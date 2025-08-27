use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use nostr_sdk::Event;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test fetching a profile and posting it to Nostr
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing profile fetch and post functionality");

    // Username to test with (repository owner)
    let username = "douglaz";

    // Step 1: Fetch the profile
    info!("Fetching profile for @{username}");
    ctx.run_nostrweet(&["fetch-profile", username])
        .await
        .context("Failed to fetch profile")?;

    // Step 2: Verify profile was downloaded (look for any file with username in it)
    let profile_files: Vec<_> = std::fs::read_dir(&ctx.output_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(&format!("_{username}_"))
        })
        .collect();

    if profile_files.is_empty() {
        anyhow::bail!("Profile file not found after download");
    }

    info!("Profile downloaded successfully");

    // Step 3: Post profile to Nostr
    info!("Posting profile to Nostr");
    ctx.run_nostrweet(&["post-profile-to-nostr", username])
        .await
        .context("Failed to post profile to Nostr")?;

    // Step 4: Verify metadata event on relay
    info!("Verifying metadata event on Nostr relay");

    // Create client to query relay
    let keys = Keys::parse(&ctx.private_key)?;
    let client = Client::new(keys.clone());
    client.add_relay(&ctx.relay_url).await?;
    client.connect().await;

    // Wait a moment for event to propagate
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // Query for all metadata events (not filtered by author since we use mnemonic-based key derivation)
    let filter = Filter::new().kind(Kind::Metadata).limit(10);

    let events = client
        .fetch_events(filter, std::time::Duration::from_secs(5))
        .await?;

    let event_vec: Vec<Event> = events.into_iter().collect();
    if event_vec.is_empty() {
        anyhow::bail!("No metadata events found on relay after posting");
    }

    // Verify event content
    let event = &event_vec[0];
    debug!("Metadata content: {content}", content = event.content);

    // Parse metadata
    let metadata: serde_json::Value =
        serde_json::from_str(&event.content).context("Failed to parse metadata JSON")?;

    // Verify expected fields
    if metadata.get("name").is_none() {
        anyhow::bail!("Metadata missing 'name' field");
    }

    if metadata.get("about").is_none() {
        anyhow::bail!("Metadata missing 'about' field");
    }

    info!("✅ Profile successfully fetched and posted to Nostr");

    Ok(())
}
