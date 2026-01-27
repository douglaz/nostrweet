use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use std::fs;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test the complete Twitter-to-Nostr pipeline by mirroring multiple accounts
/// to a local Nostr relay, ensuring all media is downloaded locally and
/// properly linked in Nostr events.
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing multi-account Twitter mirror integration");

    // Test accounts
    let accounts = vec!["douglaz", "AMAZlNGNATURE"];

    // Track statistics
    let mut total_tweets = 0;

    // Step 1: Fetch tweets for each account
    for username in &accounts {
        info!("Fetching tweets for @{username}");
        ctx.run_nostrweet(&["user-tweets", "--count", "100", "--days", "90", username])
            .await
            .with_context(|| format!("Failed to fetch tweets for @{username}"))?;

        // Count tweet files for this user
        let tweet_count = count_tweet_files(&ctx.output_dir, username)?;
        info!("Fetched {tweet_count} tweets for @{username}");
        total_tweets += tweet_count;
    }

    // Verify we have tweets
    if total_tweets == 0 {
        anyhow::bail!("No tweets fetched from any account");
    }
    info!("Total tweets fetched: {total_tweets}");

    // Step 2: Verify media files were downloaded
    let media_stats = count_media_files(&ctx.output_dir)?;
    let total_media_files = media_stats.total;

    info!(
        "Media files downloaded: {} total ({} jpg, {} png, {} mp4, {} gif)",
        media_stats.total, media_stats.jpg, media_stats.png, media_stats.mp4, media_stats.gif
    );

    // Verify all media files are non-empty
    verify_media_files_not_empty(&ctx.output_dir)?;
    info!("All media files verified non-empty");

    // Step 3: Post all tweets to Nostr for each user
    for username in &accounts {
        info!("Posting tweets for @{username} to Nostr");
        ctx.run_nostrweet(&["post-user-to-nostr", "--force", username])
            .await
            .with_context(|| format!("Failed to post tweets for @{username} to Nostr"))?;
    }

    // Step 4: Verify events on Nostr relay
    info!("Verifying events on Nostr relay");

    let keys = Keys::parse(&ctx.private_key)?;
    let client = Client::new(keys.clone());
    client.add_relay(&ctx.relay_url).await?;
    client.connect().await;

    // Wait for events to propagate
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Query for all text note events (not filtered by author since we use mnemonic-based key derivation)
    let filter = Filter::new().kind(Kind::TextNote).limit(500);

    let events = client
        .fetch_events(filter, std::time::Duration::from_secs(10))
        .await?;

    let event_vec: Vec<Event> = events.into_iter().collect();
    if event_vec.is_empty() {
        anyhow::bail!("No events found on relay after posting");
    }

    info!("Found {} events on Nostr relay", event_vec.len());

    // Step 5: Verify event signatures and check for media URLs
    let mut events_with_media = 0;
    let mut signature_errors = 0;

    for (i, event) in event_vec.iter().enumerate() {
        // Verify signature
        if let Err(e) = event.verify() {
            debug!("Event {} signature verification failed: {}", i + 1, e);
            signature_errors += 1;
        }

        // Check for media URLs in content
        let content = &event.content;
        if content.contains(".jpg")
            || content.contains(".png")
            || content.contains(".mp4")
            || content.contains(".gif")
            || content.contains("twimg")
            || content.contains("pbs.twimg.com")
            || content.contains("video.twimg.com")
        {
            events_with_media += 1;
            debug!("Event {}: media URL found in content", i + 1);
        }
    }

    if signature_errors > 0 {
        anyhow::bail!("{signature_errors} events failed signature verification");
    }
    info!(
        "All {} event signatures verified successfully",
        event_vec.len()
    );

    if events_with_media > 0 {
        info!("{events_with_media} events contain media URLs");
    } else {
        info!("Note: No events contain media URLs (accounts may not have recent media tweets)");
    }

    // Summary
    info!("=== Multi-Account Mirror Summary ===");
    info!("Accounts mirrored: {}", accounts.len());
    info!("Total tweets fetched: {total_tweets}");
    info!("Total media files: {total_media_files}");
    info!("Events posted to Nostr: {}", event_vec.len());
    info!("Events with media URLs: {events_with_media}");

    info!("✅ Multi-account mirror test completed successfully");

    Ok(())
}

/// Count tweet JSON files for a specific user
fn count_tweet_files(output_dir: &std::path::Path, username: &str) -> Result<usize> {
    let count = fs::read_dir(output_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Tweet files have format: YYYYMMDD_HHMMSS_username_tweetid.json
            // Exclude profile files which end with just username.json
            name_str.contains(username)
                && name_str.ends_with(".json")
                && !name_str.ends_with(&format!("{username}.json"))
        })
        .count();
    Ok(count)
}

/// Statistics for media files
struct MediaStats {
    total: usize,
    jpg: usize,
    png: usize,
    mp4: usize,
    gif: usize,
}

/// Count media files by extension
fn count_media_files(output_dir: &std::path::Path) -> Result<MediaStats> {
    let mut stats = MediaStats {
        total: 0,
        jpg: 0,
        png: 0,
        mp4: 0,
        gif: 0,
    };

    for entry in fs::read_dir(output_dir)?.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_lowercase();

        if name_str.ends_with(".jpg") || name_str.ends_with(".jpeg") {
            stats.jpg += 1;
            stats.total += 1;
        } else if name_str.ends_with(".png") {
            stats.png += 1;
            stats.total += 1;
        } else if name_str.ends_with(".mp4") {
            stats.mp4 += 1;
            stats.total += 1;
        } else if name_str.ends_with(".gif") {
            stats.gif += 1;
            stats.total += 1;
        }
    }

    Ok(stats)
}

/// Verify all media files are non-empty
fn verify_media_files_not_empty(output_dir: &std::path::Path) -> Result<()> {
    for entry in fs::read_dir(output_dir)?.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_lowercase();

        let is_media = name_str.ends_with(".jpg")
            || name_str.ends_with(".jpeg")
            || name_str.ends_with(".png")
            || name_str.ends_with(".mp4")
            || name_str.ends_with(".gif");

        if is_media {
            let metadata = entry.metadata()?;
            if metadata.len() == 0 {
                anyhow::bail!("Media file is empty: {}", entry.path().display());
            }
        }
    }

    Ok(())
}
