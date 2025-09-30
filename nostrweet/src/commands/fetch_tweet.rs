use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info};

use crate::media;
use crate::profile_collector;
use crate::storage;
use crate::twitter;

/// Fetch a single tweet and its media
pub async fn execute(
    tweet_url_or_id: &str,
    data_dir: &Path,
    skip_profiles: bool,
    bearer_token: &str,
) -> Result<()> {
    // Extract tweet ID from URL or use as is
    let tweet_id = twitter::parse_tweet_id(tweet_url_or_id).context("Failed to parse tweet ID")?;

    // Use the new helper function that handles loading from cache or fetching from API
    // with automatic enrichment of referenced tweets
    let tweet = storage::load_or_fetch_tweet(&tweet_id, data_dir, Some(bearer_token))
        .await
        .with_context(|| format!("Failed to load or fetch tweet {tweet_id}"))?;

    info!("Successfully retrieved tweet data");

    // Download media
    let media_results = media::download_media(&tweet, data_dir, Some(bearer_token))
        .await
        .context("Failed to download media")?;

    // Log detailed information about media files
    for result in &media_results {
        if result.from_cache {
            debug!(
                "Used cached media: {path}",
                path = result.file_path.display()
            );
        } else {
            debug!(
                "Downloaded new media: {path}",
                path = result.file_path.display()
            );
        }
    }

    let expected_media_count = tweet
        .includes
        .as_ref()
        .map_or(0, |i| i.media.as_ref().map_or(0, |m| m.len()));
    let actual_media_count = media_results.len();

    if expected_media_count == 0 {
        info!(
            "Tweet processed (no media items found/expected) in {path}",
            path = data_dir.display()
        );
    } else if actual_media_count == expected_media_count {
        info!(
            "Tweet and all {actual_media_count} media item(s) successfully processed in {path}",
            path = data_dir.display()
        );
    } else if actual_media_count > 0 {
        info!(
            "Tweet processed with {actual_media_count} out of {expected_media_count} media item(s) successfully processed in {path}",
            path = data_dir.display()
        );
    } else {
        info!(
            "Tweet processed, but all {expected_media_count} media item(s) failed to download, in {path}",
            path = data_dir.display()
        );
    }

    // Download profiles for all referenced users (unless skipped)
    if !skip_profiles {
        let usernames = profile_collector::collect_usernames_from_tweet(&tweet);
        if !usernames.is_empty() {
            debug!(
                "Found {user_count} referenced users in tweet",
                user_count = usernames.len()
            );

            let username_vec: Vec<String> = usernames.into_iter().collect();
            let client = twitter::TwitterClient::new(data_dir, bearer_token)
                .context("Failed to initialize Twitter client for profile downloads")?;

            match client.download_user_profiles(&username_vec, data_dir).await {
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
