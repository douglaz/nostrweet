use anyhow::{Context, Result};
use nostr_sdk::Event;
use nostr_sdk::prelude::*;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

use crate::test_runner::TestContext;

/// Test daemon mode functionality
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing daemon mode functionality");

    // Username to monitor (repository owner)
    let username = "douglaz";

    // First, fetch some tweets for the daemon to post
    info!("Pre-fetching tweets for daemon test");
    ctx.run_nostrweet(&["user-tweets", username])
        .await
        .context("Failed to fetch user tweets")?;

    // Start daemon process
    info!("Starting daemon for @{username}");
    let mut daemon = start_daemon(ctx, username).await?;

    // Give daemon time to start and post initial tweets
    info!("Waiting for daemon to process tweets...");
    sleep(Duration::from_secs(10)).await;

    // Query relay for events
    info!("Checking for posted events");
    let keys = Keys::parse(&ctx.private_key)?;
    let client = Client::new(keys.clone());
    client.add_relay(&ctx.relay_url).await?;
    client.connect().await;

    // Query for all text events (not filtered by author since we use mnemonic-based key derivation)
    let filter = Filter::new().kind(Kind::TextNote).limit(20);

    let events = client.fetch_events(filter, Duration::from_secs(5)).await?;

    let event_vec: Vec<Event> = events.into_iter().collect();
    if event_vec.is_empty() {
        // Try to get daemon output for debugging
        if let Err(e) = daemon.kill().await {
            warn!("Failed to kill daemon: {e}");
        }
        anyhow::bail!("No events found after daemon run");
    }

    info!(
        "Found {count} events posted by daemon",
        count = event_vec.len()
    );

    // Stop daemon
    info!("Stopping daemon");
    stop_daemon(daemon).await?;

    info!("✅ Daemon mode test completed successfully");

    Ok(())
}

async fn start_daemon(ctx: &TestContext, username: &str) -> Result<Child> {
    let mut cmd = Command::new(&ctx.nostrweet_binary);

    cmd.env("NOSTRWEET_OUTPUT_DIR", &ctx.output_dir)
        .env("NOSTRWEET_PRIVATE_KEY", &ctx.private_key)
        .env("NOSTRWEET_MNEMONIC", &ctx.mnemonic)
        .arg("daemon")
        .arg("--user")
        .arg(username)
        .arg("--relay")
        .arg(&ctx.relay_url)
        .arg("--poll-interval")
        .arg("60") // 60 seconds poll interval
        .arg("--verbose")
        .kill_on_drop(true);

    debug!("Starting daemon with command: {cmd:?}");
    let child = cmd.spawn().context("Failed to start daemon")?;

    Ok(child)
}

async fn stop_daemon(mut daemon: Child) -> Result<()> {
    // Try graceful shutdown first
    daemon.kill().await.ok();

    // Wait for process to exit
    match timeout(Duration::from_secs(5), daemon.wait()).await {
        Ok(_) => info!("Daemon stopped gracefully"),
        Err(_) => {
            warn!("Daemon did not stop gracefully");
            // kill_on_drop will handle force kill
        }
    }

    Ok(())
}
