use crate::datetime_utils::{format_date_only, is_within_days, parse_rfc3339};
use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info};

use crate::media;
use crate::profile_collector;
use crate::storage;
use crate::twitter;

/// Fetch recent tweets from a user's timeline
///
/// # Arguments
/// * `username` - Twitter username (with or without @ symbol)
/// * `output_dir` - Directory to save tweets and media
/// * `max_results` - Maximum number of tweets to fetch (default: 10)
/// * `days` - Only fetch tweets from the last N days
pub async fn execute(
    username: &str,
    output_dir: &Path,
    max_results: Option<u32>,
    days: Option<u32>,
    skip_profiles: bool,
) -> Result<()> {
    // Clean username (remove @ if present)
    let username = username.trim_start_matches('@');

    info!("Fetching recent tweets for user @{username}");

    // Create Twitter client
    let client =
        twitter::TwitterClient::new(output_dir).context("Failed to initialize Twitter client")?;

    // Fetch tweets from user's timeline
    let tweets = client
        .get_user_timeline(username, max_results)
        .await
        .context("Failed to fetch user timeline")?;

    if tweets.is_empty() {
        info!("No tweets found for user @{username}");
        return Ok(());
    }

    info!(
        "Found {tweets_count} tweets from @{username}",
        tweets_count = tweets.len()
    );

    // Process each tweet (save data and download media)
    let mut processed_count = 0;
    let mut skipped_count = 0;
    let mut filtered_count = 0;
    let mut media_files_count = 0;
    let mut processed_tweets = Vec::new();

    if let Some(d) = days {
        info!("Filtering tweets from the last {d} days");
    }

    // Process each tweet: enrich references, download media, and save JSON if new
    for tweet in tweets {
        // Apply date filtering if requested
        if let Some(d) = days {
            if let Ok(tweet_date) = parse_rfc3339(&tweet.created_at) {
                if !is_within_days(&tweet_date, d) {
                    filtered_count += 1;
                    debug!(
                        "Skipping tweet {id} from {date} (older than filter)",
                        id = tweet.id,
                        date = format_date_only(&tweet_date)
                    );
                    continue;
                }
            } else {
                debug!(
                    "Could not parse date for tweet {id}: {date}",
                    id = tweet.id,
                    date = tweet.created_at
                );
            }
        }

        let tweet_id = &tweet.id;
        let mut tweet_to_save = tweet.clone();

        // Fill missing author info
        if tweet_to_save.author.username.is_empty() {
            debug!("Adding missing author information for tweet {tweet_id}");
            tweet_to_save.author.username = username.to_string();
            if tweet_to_save.author.id.is_empty() {
                if let Some(author_id) = &tweet_to_save.author_id {
                    tweet_to_save.author.id = author_id.clone();
                }
            }
        }

        // Enrich referenced tweets (always)
        if let Err(e) = client
            .enrich_referenced_tweets(&mut tweet_to_save, Some(output_dir))
            .await
        {
            debug!("Failed to enrich referenced tweets for {tweet_id}: {e}");
        }

        // Download media for tweet and its references
        let media_results = media::download_media(&tweet_to_save, output_dir)
            .await
            .with_context(|| format!("Failed to download media for tweet {tweet_id}"))?;
        let new_media = media_results.iter().filter(|r| !r.from_cache).count();
        let cached_media = media_results.len() - new_media;
        if new_media > 0 {
            debug!("Downloaded {new_media} new media files for tweet {tweet_id}");
        }
        if cached_media > 0 {
            debug!("Used {cached_media} cached media files for tweet {tweet_id}");
        }

        let expected_media_count = tweet_to_save
            .includes
            .as_ref()
            .map_or(0, |i| i.media.as_ref().map_or(0, |m| m.len()));
        let actual_media_count = media_results.len();

        if expected_media_count == 0 {
            debug!("Tweet {tweet_id}: No media items found/expected.");
        } else if actual_media_count == expected_media_count {
            debug!("Tweet {tweet_id}: All {actual_media_count} media item(s) processed.");
        } else {
            debug!(
                "Tweet {tweet_id}: {actual_media_count} out of {expected_media_count} media item(s) processed."
            );
        }

        media_files_count += actual_media_count;

        // Save main tweet JSON if missing
        if let Some(path) = storage::find_existing_tweet_json(tweet_id, output_dir) {
            debug!(
                "Tweet {tweet_id} already exists: {path}",
                path = path.display()
            );
            skipped_count += 1;
        } else {
            let saved_path = storage::save_tweet(&tweet_to_save, output_dir)
                .with_context(|| format!("Failed to save tweet data for tweet {tweet_id}"))?;
            debug!("Saved tweet data to {path}", path = saved_path.display());
            processed_count += 1;
        }

        // Store the processed tweet for profile collection
        processed_tweets.push(tweet_to_save);
    }

    // Summary
    info!("Processed {processed_count} new tweets for @{username}");

    if skipped_count > 0 {
        info!("Skipped {skipped_count} tweets that were already in the cache");
    }

    if filtered_count > 0 {
        info!("Filtered out {filtered_count} tweets that were older than the specified date range");
    }

    if media_files_count > 0 {
        info!("Processed {media_files_count} media files in total");
    }

    // Report if we couldn't get the exact number of tweets requested
    if let Some(requested) = max_results {
        let total_tweets = processed_count + skipped_count;
        if total_tweets < requested as usize {
            info!("Note: Requested {requested} tweets but could only retrieve {total_tweets}");
        }
    }

    info!(
        "All tweets and media for @{username} successfully processed in {path}",
        path = output_dir.display()
    );

    // Download profiles for all referenced users across all tweets (unless skipped)
    if !skip_profiles {
        let all_usernames = profile_collector::collect_usernames_from_tweets(&processed_tweets);
        if !all_usernames.is_empty() {
            debug!(
                "Found {} unique referenced users across all tweets",
                all_usernames.len()
            );

            let username_vec: Vec<String> = all_usernames.into_iter().collect();
            match client
                .download_user_profiles(&username_vec, output_dir)
                .await
            {
                Ok(profiles) => {
                    if !profiles.is_empty() {
                        info!(
                            "Downloaded {profile_count} new user profiles",
                            profile_count = profiles.len()
                        );
                    }
                }
                Err(e) => {
                    debug!("Failed to download some user profiles: {e}");
                    // Continue even if profile downloads fail
                }
            }
        }
    } else {
        debug!("Skipping profile downloads (--skip-profiles flag set)");
    }

    Ok(())
}
