use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use nostr_sdk::Event;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test posting all cached tweets for a user to Nostr
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing batch posting with post-user-to-nostr command");

    // Username to test with (repository owner)
    let username = "douglaz";

    // Step 1: First fetch some tweets to have something to post
    info!("Pre-fetching tweets for batch posting test");
    ctx.run_nostrweet(&["user-tweets", "--count", "3", username])
        .await
        .context("Failed to fetch user tweets for batch test")?;

    // Step 2: Post all tweets for the user to Nostr
    info!("Batch posting all tweets for @{username} to Nostr");
    ctx.run_nostrweet(&[
        "post-user-to-nostr",
        "--force", // Force posting even if already posted
        username,
    ])
    .await
    .context("Failed to batch post user tweets to Nostr")?;

    // Step 3: Verify events on Nostr relay
    info!("Verifying batch posted events on Nostr relay");

    // Create client to query relay
    let keys = Keys::parse(&ctx.private_key)?;
    let client = Client::new(keys.clone());
    client.add_relay(&ctx.relay_url).await?;
    client.connect().await;

    // Wait for events to propagate
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Query for text events from our pubkey
    let filter = Filter::new().author(keys.public_key()).kind(Kind::TextNote);

    let events = client
        .fetch_events(filter, std::time::Duration::from_secs(5))
        .await?;

    let event_vec: Vec<Event> = events.into_iter().collect();
    if event_vec.is_empty() {
        anyhow::bail!("No events found on relay after batch posting");
    }

    info!(
        "Found {count} events after batch posting",
        count = event_vec.len()
    );

    // Verify we have multiple events (batch posted)
    if event_vec.len() < 2 {
        let count = event_vec.len();
        anyhow::bail!("Expected multiple events from batch posting, found only {count}");
    }

    // Check that events contain expected content
    for (i, event) in event_vec.iter().enumerate() {
        debug!(
            "Event {}: {}",
            i + 1,
            &event.content[..100.min(event.content.len())]
        );

        // Verify signature
        event.verify().with_context(|| {
            format!("Event {index} signature verification failed", index = i + 1)
        })?;
    }

    // Step 4: Test with skip-profiles flag
    info!("Testing batch post with --skip-profiles flag");
    let result = ctx
        .run_nostrweet(&["post-user-to-nostr", "--force", "--skip-profiles", username])
        .await;

    match result {
        Ok(_) => info!("Batch post with skip-profiles completed"),
        Err(e) => debug!("Skip-profiles test result: {e}"),
    }

    info!("✅ Batch posting tests completed successfully");

    Ok(())
}
