use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use serde_json::Value;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test the utils query-events command
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing utils query-events command");

    // Step 1: Post some test events to have something to query
    info!("Creating test events for querying");

    // Post a profile (metadata event)
    ctx.run_nostrweet(&["fetch-profile", "douglaz"])
        .await
        .context("Failed to fetch profile")?;

    ctx.run_nostrweet(&["post-profile-to-nostr", "douglaz"])
        .await
        .context("Failed to post profile")?;

    // Post a tweet (text note event)
    ctx.run_nostrweet(&["fetch-tweet", "1628832338187636737"])
        .await
        .context("Failed to fetch tweet")?;

    ctx.run_nostrweet(&["post-tweet-to-nostr", "--force", "1628832338187636737"])
        .await
        .context("Failed to post tweet")?;

    // Wait for events to propagate
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Step 2: Test basic query without filters
    info!("Testing basic query-events");
    let output = ctx
        .run_nostrweet_with_output(&["utils", "query-events", "--limit", "5"])
        .await
        .context("Failed to query events")?;

    if output.is_empty() {
        anyhow::bail!("query-events returned empty output");
    }

    debug!(
        "Basic query output length: {length} bytes",
        length = output.len()
    );

    // Step 3: Test query with kind filter for metadata
    info!("Testing query-events with kind filter (metadata)");
    let metadata_output = ctx
        .run_nostrweet_with_output(&[
            "utils",
            "query-events",
            "--kind",
            "0", // Kind 0 = metadata
            "--limit",
            "5",
        ])
        .await
        .context("Failed to query metadata events")?;

    debug!(
        "Metadata query output: {output}",
        output = &metadata_output[..200.min(metadata_output.len())]
    );

    // Step 4: Test query with kind filter for text notes
    info!("Testing query-events with kind filter (text notes)");
    let text_output = ctx
        .run_nostrweet_with_output(&[
            "utils",
            "query-events",
            "--kind",
            "1", // Kind 1 = text note
            "--limit",
            "5",
        ])
        .await
        .context("Failed to query text note events")?;

    debug!(
        "Text note query output: {output}",
        output = &text_output[..200.min(text_output.len())]
    );

    // Step 5: Test JSON format output
    info!("Testing query-events with JSON format");
    let json_output = ctx
        .run_nostrweet_with_output(&["utils", "query-events", "--format", "json", "--limit", "2"])
        .await
        .context("Failed to query events with JSON format")?;

    // Verify JSON is valid
    let parsed: Result<Value, _> = serde_json::from_str(&json_output);
    if parsed.is_err() {
        anyhow::bail!("query-events JSON output is not valid JSON");
    }

    info!("JSON format output is valid");

    // Step 6: Test query with author filter
    info!("Testing query-events with author filter");

    // Get our test key's public key
    let keys = Keys::parse(&ctx.private_key)?;
    let npub = keys.public_key().to_bech32()?;

    let author_output = ctx
        .run_nostrweet_with_output(&["utils", "query-events", "--author", &npub, "--limit", "10"])
        .await
        .context("Failed to query events by author")?;

    if author_output.is_empty() {
        anyhow::bail!("No events found for our test author");
    }

    debug!("Found events for author {author}", author = &npub[..20]);

    // Step 7: Test time range filters
    info!("Testing query-events with time range filters");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let since = now - 3600; // 1 hour ago
    let until = now + 3600; // 1 hour from now

    let time_output = ctx
        .run_nostrweet_with_output(&[
            "utils",
            "query-events",
            "--since",
            &since.to_string(),
            "--until",
            &until.to_string(),
            "--limit",
            "10",
        ])
        .await
        .context("Failed to query events with time range")?;

    debug!(
        "Time range query returned {length} bytes",
        length = time_output.len()
    );

    // Step 8: Test output to file
    info!("Testing query-events with file output");
    let output_file = ctx.output_dir.join("query_results.json");

    ctx.run_nostrweet(&[
        "utils",
        "query-events",
        "--format",
        "json",
        "--limit",
        "3",
        "--output",
        output_file.to_str().unwrap(),
    ])
    .await
    .context("Failed to query events with file output")?;

    // Verify file was created
    if !output_file.exists() {
        anyhow::bail!("Output file was not created");
    }

    // Verify file contains valid JSON
    let file_content = std::fs::read_to_string(&output_file)?;
    let _: Value =
        serde_json::from_str(&file_content).context("Output file does not contain valid JSON")?;

    info!("✅ Utils query-events tests completed successfully");

    Ok(())
}
