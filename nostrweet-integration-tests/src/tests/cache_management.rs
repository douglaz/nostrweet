use anyhow::{Context, Result};
use std::fs;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test cache management commands (list-tweets and clear-cache)
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing cache management commands");

    // Step 1: Fetch some tweets to populate the cache
    info!("Populating cache with test data");
    ctx.run_nostrweet(&["fetch-tweet", "1453856044928933893"])
        .await
        .context("Failed to fetch tweet for cache test")?;

    ctx.run_nostrweet(&["user-tweets", "--count", "2", "douglaz"])
        .await
        .context("Failed to fetch user tweets for cache test")?;

    // Step 2: Test list-tweets command
    info!("Testing list-tweets command");
    let output = ctx
        .run_nostrweet_with_output(&["list-tweets"])
        .await
        .context("Failed to list tweets")?;

    // Verify output contains tweet information
    if output.is_empty() {
        anyhow::bail!("list-tweets returned empty output");
    }

    debug!(
        "list-tweets output length: {length} bytes",
        length = output.len()
    );

    // Check that output mentions cached tweets
    if !output.contains("tweet") && !output.contains("Tweet") {
        anyhow::bail!("list-tweets output doesn't appear to contain tweet information");
    }

    info!("list-tweets command successful");

    // Step 3: Count files before clearing cache
    let files_before: Vec<_> = fs::read_dir(&ctx.output_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            name_str.ends_with(".json") || name_str.ends_with(".jpg") || name_str.ends_with(".mp4")
        })
        .collect();

    let count_before = files_before.len();
    info!("Files in cache before clear: {count_before}");

    // Step 4: Test clear-cache command with force flag
    info!("Testing clear-cache command with --force");
    ctx.run_nostrweet(&["clear-cache", "--force"])
        .await
        .context("Failed to clear cache")?;

    // Step 5: Verify cache was cleared
    let files_after: Vec<_> = fs::read_dir(&ctx.output_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            name_str.ends_with(".json") || name_str.ends_with(".jpg") || name_str.ends_with(".mp4")
        })
        .collect();

    let count_after = files_after.len();
    info!("Files in cache after clear: {count_after}");

    if count_after >= count_before {
        anyhow::bail!(
            "Cache was not cleared properly. Files before: {count_before}, after: {count_after}"
        );
    }

    // Step 6: Test list-tweets on empty cache
    info!("Testing list-tweets on empty cache");
    let empty_output = ctx
        .run_nostrweet_with_output(&["list-tweets"])
        .await
        .context("Failed to list tweets on empty cache")?;

    debug!("Empty cache list-tweets output: {empty_output}");

    info!("✅ Cache management tests completed successfully");

    Ok(())
}
