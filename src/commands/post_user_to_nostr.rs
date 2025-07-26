use anyhow::{ensure, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use crate::commands::post_tweet_to_nostr;
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
    private_key: Option<&str>,
    output_dir: &Path,
    force: bool,
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

        // Post the tweet to Nostr
        match post_tweet_to_nostr::execute(
            &tweet_id,
            relays,
            blossom_servers,
            private_key,
            output_dir,
            force,
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

    Ok(())
}
