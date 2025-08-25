use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use nostr_sdk::Event;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test fetching a tweet and posting it to Nostr
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing tweet fetch and post functionality");

    // Tweet ID to test with (Twitter's first tweet)
    let tweet_id = "20";

    // Step 1: Fetch the tweet
    info!("Fetching tweet {tweet_id}");
    ctx.run_nostrweet(&["fetch-tweet", tweet_id])
        .await
        .context("Failed to fetch tweet")?;

    // Step 2: Verify tweet was downloaded
    let tweet_files: Vec<_> = std::fs::read_dir(&ctx.output_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(&format!("_{tweet_id}.json"))
        })
        .collect();

    if tweet_files.is_empty() {
        anyhow::bail!("Tweet file not found after download");
    }

    info!("Tweet downloaded successfully");

    // Step 3: Post tweet to Nostr
    info!("Posting tweet to Nostr");
    ctx.run_nostrweet(&[
        "post-tweet-to-nostr",
        "--force", // Force posting even if already posted
        tweet_id,
    ])
    .await
    .context("Failed to post tweet to Nostr")?;

    // Step 4: Verify event on relay
    info!("Verifying event on Nostr relay");

    // Create client to query relay
    let keys = Keys::parse(&ctx.private_key)?;
    let client = Client::new(keys.clone());
    client.add_relay(&ctx.relay_url).await?;
    client.connect().await;

    // Wait a moment for event to propagate
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // Query for events from our pubkey
    let filter = Filter::new().author(keys.public_key()).kind(Kind::TextNote);

    let events = client
        .fetch_events(filter, std::time::Duration::from_secs(5))
        .await?;

    let event_vec: Vec<Event> = events.into_iter().collect();
    if event_vec.is_empty() {
        anyhow::bail!("No events found on relay after posting");
    }

    // Verify event content
    let event = &event_vec[0];
    debug!("Event content: {}", event.content);

    // Check for expected content (Twitter's first tweet)
    if !event.content.contains("just setting up my twttr") {
        anyhow::bail!("Event content does not match expected tweet");
    }

    info!("✅ Tweet successfully fetched and posted to Nostr");

    Ok(())
}
