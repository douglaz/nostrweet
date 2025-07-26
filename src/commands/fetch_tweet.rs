use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, info};

use crate::media;
use crate::storage;
use crate::twitter;

/// Fetch a single tweet and its media
pub async fn execute(tweet_url_or_id: &str, output_dir: &Path) -> Result<()> {
    // Extract tweet ID from URL or use as is
    let tweet_id = twitter::parse_tweet_id(tweet_url_or_id).context("Failed to parse tweet ID")?;

    // Check if we already have the tweet data locally
    let tweet = if let Some(existing_path) =
        storage::find_existing_tweet_json(&tweet_id, output_dir)
    {
        // Use existing tweet data
        debug!(
            "Found existing tweet data: {path}",
            path = existing_path.display()
        );
        storage::load_tweet_from_file(&existing_path).context("Failed to load local tweet data")?
    } else {
        // Download the tweet data from the API
        info!("Downloading tweet {tweet_id}");

        // Download the tweet and its media
        let client = twitter::TwitterClient::new(output_dir)
            .context("Failed to initialize Twitter client")?;

        // Download the tweet
        let mut downloaded_tweet = client
            .get_tweet(&tweet_id)
            .await
            .context("Failed to download tweet")?;

        // Enrich the tweet with referenced tweet data
        if let Err(e) = client
            .enrich_referenced_tweets(&mut downloaded_tweet, Some(output_dir))
            .await
        {
            debug!("Failed to enrich referenced tweets: {e}");
            // Continue with the basic tweet even if enrichment fails
        }

        info!("Successfully retrieved tweet data");

        // Save tweet data
        let saved_path = storage::save_tweet(&downloaded_tweet, output_dir)
            .context("Failed to save tweet data")?;
        debug!("Saved tweet data to {path}", path = saved_path.display());

        downloaded_tweet
    };

    // Download media
    let media_results = media::download_media(&tweet, output_dir)
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
            path = output_dir.display()
        );
    } else if actual_media_count == expected_media_count {
        info!(
            "Tweet and all {actual_media_count} media item(s) successfully processed in {path}",
            path = output_dir.display()
        );
    } else if actual_media_count > 0 {
        info!(
            "Tweet processed with {actual_media_count} out of {expected_media_count} media item(s) successfully processed in {path}",
            path = output_dir.display()
        );
    } else {
        info!(
            "Tweet processed, but all {expected_media_count} media item(s) failed to download, in {path}",
            path = output_dir.display()
        );
    }

    Ok(())
}
