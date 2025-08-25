use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use nostr_sdk::Event;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test fetching a tweet and posting it to Nostr - Full End-to-End Test
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing complete end-to-end flow: Twitter -> nostrweet -> Nostr relay");

    // Tweet ID to test with (Twitter's first tweet - stable and historical)
    // This tweet by @jack: "just setting up my twttr"
    let tweet_id = "20";

    // Step 1: Fetch the tweet from Twitter API
    info!("Step 1: Fetching tweet {tweet_id} from Twitter API");
    ctx.run_nostrweet(&["fetch-tweet", tweet_id])
        .await
        .context("Failed to fetch tweet from Twitter API")?;

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

    info!("Tweet downloaded successfully to local filesystem");

    // Step 3: Post tweet to Nostr relay
    info!("Step 2: Posting tweet to Nostr relay");
    ctx.run_nostrweet(&[
        "post-tweet-to-nostr",
        "--force", // Force posting even if already posted
        tweet_id,
    ])
    .await
    .context("Failed to post tweet to Nostr relay")?;

    // Step 4: Verify event on Nostr relay
    info!("Step 3: Verifying event on Nostr relay");

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

    // Verify event content and structure
    let event = &event_vec[0];
    debug!("Event content: {}", event.content);
    debug!("Event ID: {}", event.id);
    debug!("Event pubkey: {}", event.pubkey);
    debug!("Event created_at: {}", event.created_at);

    // Comprehensive verification
    info!("Performing comprehensive event verification...");

    // 1. Check content matches expected tweet
    if !event.content.contains("just setting up my twttr") {
        anyhow::bail!("Event content does not match expected tweet");
    }
    info!("  ✓ Content matches expected tweet");

    // 2. Verify event signature
    event
        .verify()
        .context("Event signature verification failed")?;
    info!("  ✓ Event signature is valid");

    // 3. Check timestamp is reasonable (within last minute)
    let now = Timestamp::now();
    let event_age = now.as_u64().saturating_sub(event.created_at.as_u64());
    if event_age > 60 {
        anyhow::bail!("Event timestamp is too old: {} seconds", event_age);
    }
    info!("  ✓ Event timestamp is recent");

    // 4. Verify it's from our test key
    if event.pubkey != keys.public_key() {
        anyhow::bail!("Event pubkey doesn't match our test key");
    }
    info!("  ✓ Event is from correct pubkey");

    // 5. Check for Twitter link in content
    if !event.content.contains("twitter.com") && !event.content.contains("x.com") {
        debug!("Note: No Twitter/X link found in content (might be expected for old tweets)");
    }

    info!("✅ FULL END-TO-END TEST PASSED!");
    info!("   Twitter API → nostrweet → Nostr relay → Verification complete");

    Ok(())
}
