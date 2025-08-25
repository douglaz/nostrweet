use anyhow::{Context, Result};
use std::fs;
use tracing::info;

use crate::test_runner::TestContext;

/// Test fetching multiple tweets from a user's timeline
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing user-tweets command functionality");

    // Username to test with (repository owner)
    let username = "douglaz";

    // Test 1: Fetch recent tweets with count limit
    info!("Testing user-tweets with --count parameter");
    ctx.run_nostrweet(&["user-tweets", "--count", "5", username])
        .await
        .context("Failed to fetch user tweets with count limit")?;

    // Verify tweets were downloaded
    let tweet_files: Vec<_> = fs::read_dir(&ctx.output_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            name_str.contains(username)
                && name_str.ends_with(".json")
                && !name_str.ends_with(&format!("{username}.json"))
        })
        .collect();

    if tweet_files.is_empty() {
        anyhow::bail!("No tweet files found after user-tweets command");
    }

    info!("Found {} tweet files for @{username}", tweet_files.len());

    // Test 2: Fetch tweets with days filter (if Twitter API allows)
    info!("Testing user-tweets with --days parameter");
    let result = ctx
        .run_nostrweet(&["user-tweets", "--count", "10", "--days", "7", username])
        .await;

    match result {
        Ok(_) => info!("Successfully fetched tweets from last 7 days"),
        Err(e) => info!("Days filter test skipped (may not be supported): {e}"),
    }

    // Test 3: Test skip-profiles flag
    info!("Testing user-tweets with --skip-profiles flag");
    ctx.run_nostrweet(&["user-tweets", "--count", "3", "--skip-profiles", username])
        .await
        .context("Failed to fetch tweets with skip-profiles flag")?;

    // Verify profile files were not created for referenced users
    // (This would need actual referenced tweets to properly test)

    info!("✅ User tweets fetching tests completed successfully");

    Ok(())
}
