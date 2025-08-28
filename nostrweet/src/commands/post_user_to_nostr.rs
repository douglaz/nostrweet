use anyhow::{Context, Result, ensure};
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

/// Options for filtering tweets when posting to Nostr
#[derive(Debug, Clone, Default)]
pub struct PostUserOptions {
    pub force: bool,
    pub skip_profiles: bool,
    pub since_date: Option<String>,
    pub until_date: Option<String>,
    pub filter_keywords: Option<Vec<String>>,
    pub exclude_keywords: Option<Vec<String>>,
    pub dry_run: bool,
}

/// Post all cached tweets for a user to Nostr relays with filtering options
pub async fn execute(
    username: &str,
    relays: &[String],
    blossom_servers: &[String],
    output_dir: &Path,
    force: bool,
    skip_profiles: bool,
    mnemonic: Option<&str>,
) -> Result<()> {
    let options = PostUserOptions {
        force,
        skip_profiles,
        ..Default::default()
    };
    execute_with_options(
        username,
        relays,
        blossom_servers,
        output_dir,
        options,
        mnemonic,
    )
    .await
}

/// Post all cached tweets for a user to Nostr relays with advanced filtering options
pub async fn execute_with_options(
    username: &str,
    relays: &[String],
    blossom_servers: &[String],
    output_dir: &Path,
    options: PostUserOptions,
    mnemonic: Option<&str>,
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

        // Load the tweet to apply filters
        let tweet = match storage::load_tweet_from_file(tweet_file) {
            Ok(tweet) => tweet,
            Err(e) => {
                debug!(
                    "Failed to load tweet from {path}: {error}",
                    path = tweet_file.display(),
                    error = e
                );
                error_count += 1;
                continue;
            }
        };

        // Apply date filters
        if let Some(since_date) = &options.since_date {
            if let (Ok(since), Ok(tweet_date)) = (
                chrono::DateTime::parse_from_rfc3339(since_date),
                chrono::DateTime::parse_from_rfc3339(&tweet.created_at),
            ) {
                if tweet_date < since {
                    debug!("Skipping tweet {tweet_id}: before since_date");
                    skip_count += 1;
                    continue;
                }
            }
        }

        if let Some(until_date) = &options.until_date {
            if let (Ok(until), Ok(tweet_date)) = (
                chrono::DateTime::parse_from_rfc3339(until_date),
                chrono::DateTime::parse_from_rfc3339(&tweet.created_at),
            ) {
                if tweet_date > until {
                    debug!("Skipping tweet {tweet_id}: after until_date");
                    skip_count += 1;
                    continue;
                }
            }
        }

        // Apply keyword filters
        if let Some(filter_keywords) = &options.filter_keywords {
            let tweet_text = tweet.text.to_lowercase();
            let has_required_keyword = filter_keywords
                .iter()
                .any(|keyword| tweet_text.contains(&keyword.to_lowercase()));
            if !has_required_keyword {
                debug!("Skipping tweet {tweet_id}: doesn't contain required keywords");
                skip_count += 1;
                continue;
            }
        }

        if let Some(exclude_keywords) = &options.exclude_keywords {
            let tweet_text = tweet.text.to_lowercase();
            let has_excluded_keyword = exclude_keywords
                .iter()
                .any(|keyword| tweet_text.contains(&keyword.to_lowercase()));
            if has_excluded_keyword {
                debug!("Skipping tweet {tweet_id}: contains excluded keywords");
                skip_count += 1;
                continue;
            }
        }

        // If dry run, just count and continue
        if options.dry_run {
            info!("DRY RUN: Would post tweet {tweet_id}");
            success_count += 1;
            continue;
        }

        // Collect referenced users from this tweet if we're posting profiles
        if !options.skip_profiles {
            let usernames = profile_collector::collect_usernames_from_tweet(&tweet);
            all_referenced_users.extend(usernames);
        }

        // Post the tweet to Nostr (with skip_profiles=true to avoid duplicate profile posting)
        match post_tweet_to_nostr::execute(
            &tweet_id,
            relays,
            blossom_servers,
            output_dir,
            options.force,
            true, // Always skip profiles here, we'll post them all at once at the end
            mnemonic,
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
    if options.dry_run {
        info!(
            "DRY RUN completed for @{username}: {success_count} tweets would be posted, {skip_count} filtered out, {error_count} errors"
        );
    } else {
        info!(
            "Completed posting tweets for @{username} to Nostr: {success_count} posted, {skip_count} skipped, {error_count} failed"
        );
    }

    // Print filtering summary if filters were applied
    if options.since_date.is_some()
        || options.until_date.is_some()
        || options.filter_keywords.is_some()
        || options.exclude_keywords.is_some()
    {
        info!("Applied filters:");
        if let Some(since) = &options.since_date {
            info!("  - Since date: {since}");
        }
        if let Some(until) = &options.until_date {
            info!("  - Until date: {until}");
        }
        if let Some(keywords) = &options.filter_keywords {
            info!("  - Required keywords: {keywords:?}");
        }
        if let Some(keywords) = &options.exclude_keywords {
            info!("  - Excluded keywords: {keywords:?}");
        }
    }

    // Post profiles for all referenced users (unless skipped)
    if !options.skip_profiles && !all_referenced_users.is_empty() && !options.dry_run {
        info!(
            "Found {user_count} unique referenced users across all tweets",
            user_count = all_referenced_users.len()
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
            let keys = crate::keys::get_keys_for_tweet(&uid, mnemonic)?;
            let client = nostr::initialize_nostr_client(&keys, relays).await?;

            // Filter profiles that need to be posted
            let profiles_to_post = nostr_profile::filter_profiles_to_post(
                all_referenced_users,
                &client,
                output_dir,
                options.force,
                mnemonic,
            )
            .await?;

            if !profiles_to_post.is_empty() {
                // Post the profiles
                let posted_count = nostr_profile::post_referenced_profiles(
                    &profiles_to_post,
                    &client,
                    output_dir,
                    mnemonic,
                )
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
    } else if options.skip_profiles {
        debug!("Skipping profile posting (--skip-profiles flag set)");
    } else if options.dry_run {
        debug!("Skipping profile posting (dry run mode)");
    }

    Ok(())
}
