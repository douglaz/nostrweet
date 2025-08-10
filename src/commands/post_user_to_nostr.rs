use anyhow::{ensure, Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use crate::commands::post_tweet_to_nostr;
use crate::nostr;
use crate::nostr_profile;
use crate::profile_collector;
use crate::storage;

/// Find all tweet JSON files for a specific user in the output directory
async fn find_user_tweets(username: &str, output_dir: &Path) -> Result<Vec<PathBuf>> {
    ensure!(
        output_dir.exists(),
        "Output directory does not exist: {path}",
        path = output_dir.display()
    );

    // Normalize the username for consistent matching
    let normalized_username = username.trim_start_matches('@').to_lowercase();

    // Read all entries in the directory
    let entries = fs::read_dir(output_dir).with_context(|| {
        format!(
            "Failed to read output directory: {path}",
            path = output_dir.display()
        )
    })?;

    // Collect all matching tweet files
    let mut user_tweets: Vec<PathBuf> = entries
        .filter_map(|entry| {
            // Flatten Result to skip errors
            let entry = entry.ok()?;
            let path = entry.path();

            // Only consider JSON files
            if !path.is_file() || path.extension()?.to_str()? != "json" {
                return None;
            }

            // Check if filename contains the username
            let filename = path.file_name()?.to_str()?;
            if filename.to_lowercase().contains(&normalized_username) {
                debug!(
                    "Found tweet for user {username}: {path}",
                    path = path.display()
                );
                Some(path)
            } else {
                None
            }
        })
        .collect();

    info!(
        "Found {count} cached tweets for user @{username}",
        count = user_tweets.len()
    );

    // Sort tweets by filename (which includes date) to process in chronological order
    user_tweets.sort();

    Ok(user_tweets)
}

/// Post all cached tweets for a user to Nostr relays
pub async fn execute(
    username: &str,
    relays: &[String],
    blossom_servers: &[String],
    output_dir: &Path,
    force: bool,
    skip_profiles: bool,
) -> Result<()> {
    // Clean username (remove @ if present)
    let username = username.trim_start_matches('@');

    info!("Finding cached tweets for user @{username}");

    // Find all tweets for this user
    let tweet_files = find_user_tweets(username, output_dir).await?;

    ensure!(
        !tweet_files.is_empty(),
        "No cached tweets found for user @{username}. Please fetch tweets first using the 'user-tweets' command."
    );

    info!(
        "Found {count} cached tweets for user @{username}, posting to Nostr...",
        count = tweet_files.len()
    );

    let mut success_count = 0;
    let mut skip_count = 0;
    let mut error_count = 0;
    let mut all_referenced_users = HashSet::new();

    // Process each tweet
    for tweet_file in &tweet_files {
        // Extract tweet ID from filename
        let filename = tweet_file
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        // Try to extract tweet ID from filename
        // Format is typically: YYYYMMDD_HHMMSS_username_tweetid.json
        let tweet_id = if let Some(idx) = filename.rfind('_') {
            let end_idx = filename.rfind('.').unwrap_or(filename.len());
            filename[(idx + 1)..end_idx].to_string()
        } else {
            // If we can't parse the filename format, load the tweet to get the ID
            match storage::load_tweet_from_file(tweet_file) {
                Ok(tweet) => tweet.id.clone(),
                Err(e) => {
                    debug!(
                        "Failed to load tweet from {path}: {error}",
                        path = tweet_file.display(),
                        error = e
                    );
                    error_count += 1;
                    continue;
                }
            }
        };

        debug!("Processing tweet ID: {tweet_id}");

        // Collect referenced users from this tweet if we're posting profiles
        if !skip_profiles {
            if let Ok(tweet) = storage::load_tweet_from_file(tweet_file) {
                let usernames = profile_collector::collect_usernames_from_tweet(&tweet);
                all_referenced_users.extend(usernames);
            }
        }

        // Post the tweet to Nostr (with skip_profiles=true to avoid duplicate profile posting)
        match post_tweet_to_nostr::execute(
            &tweet_id,
            relays,
            blossom_servers,
            output_dir,
            force,
            true, // Always skip profiles here, we'll post them all at once at the end
        )
        .await
        {
            Ok(()) => {
                info!("Successfully posted tweet {tweet_id} to Nostr");
                success_count += 1;
            }
            Err(e) => {
                // Don't halt on error, just log and continue
                if e.to_string().contains("already posted") {
                    debug!("Tweet {tweet_id} already posted to Nostr, skipping");
                    skip_count += 1;
                } else {
                    debug!(
                        "Failed to post tweet {tweet_id} to Nostr: {error}",
                        error = e
                    );
                    error_count += 1;
                }
            }
        }
    }

    // Print summary
    info!(
        "Completed posting tweets for @{username} to Nostr: {success} posted, {skip} skipped, {error} failed",
        success = success_count,
        skip = skip_count,
        error = error_count
    );

    // Post profiles for all referenced users (unless skipped)
    if !skip_profiles && !all_referenced_users.is_empty() {
        info!(
            "Found {} unique referenced users across all tweets",
            all_referenced_users.len()
        );

        // We need to get the Nostr keys for the main user
        // Try to load any tweet to get the author ID
        let user_id = if let Some(tweet_file) = tweet_files.first() {
            storage::load_tweet_from_file(tweet_file)
                .ok()
                .and_then(|t| {
                    if !t.author.id.is_empty() {
                        Some(t.author.id)
                    } else {
                        None
                    }
                })
        } else {
            None
        };

        // Only proceed if we have a user ID
        if let Some(uid) = user_id {
            // Initialize Nostr client with the user's keys
            let keys = crate::keys::get_keys_for_tweet(&uid)?;
            let client = nostr::initialize_nostr_client(&keys, relays).await?;

            // Filter profiles that need to be posted
            let profiles_to_post = nostr_profile::filter_profiles_to_post(
                all_referenced_users,
                &client,
                output_dir,
                force,
            )
            .await?;

            if !profiles_to_post.is_empty() {
                // Post the profiles
                let posted_count =
                    nostr_profile::post_referenced_profiles(&profiles_to_post, &client, output_dir)
                        .await?;

                if posted_count > 0 {
                    info!("Posted {posted_count} referenced user profiles to Nostr");
                }
            } else {
                debug!("All referenced user profiles already posted or not available");
            }
        } else {
            debug!("Could not determine user ID for profile posting");
        }
    } else if skip_profiles {
        debug!("Skipping profile posting (--skip-profiles flag set)");
    }

    Ok(())
}
