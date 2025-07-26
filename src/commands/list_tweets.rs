use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tracing::{info, warn};

use crate::datetime_utils::{format_for_display, from_unix_timestamp};
use crate::twitter;

/// List all downloaded tweets in the cache
pub async fn execute(output_dir: &PathBuf) -> Result<()> {
    let mut entries = fs::read_dir(output_dir)
        .await
        .context("Failed to read output directory")?;

    let mut tweet_files = Vec::new();

    // Collect all JSON files
    while let Some(entry) = entries
        .next_entry()
        .await
        .context("Failed to read directory entry")?
    {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            tweet_files.push(path);
        }
    }

    if tweet_files.is_empty() {
        info!("No tweets found in {path}", path = output_dir.display());
        return Ok(());
    }

    // Sort by modification time (newest first)
    let mut file_times = HashMap::new();
    for path in &tweet_files {
        if let Ok(metadata) = fs::metadata(path).await {
            if let Ok(modified) = metadata.modified() {
                file_times.insert(path.clone(), modified);
            }
        }
    }

    // Clone tweet_files before sorting to avoid borrow checker issues
    let mut sorted_files = tweet_files.clone();
    sorted_files.sort_by(|a, b| {
        let time_a = file_times
            .get(a)
            .unwrap_or(&std::time::SystemTime::UNIX_EPOCH);
        let time_b = file_times
            .get(b)
            .unwrap_or(&std::time::SystemTime::UNIX_EPOCH);
        time_b.cmp(time_a) // newest first
    });

    // Display the tweets
    println!(
        "Found {} tweets in {}",
        sorted_files.len(),
        output_dir.display()
    );
    println!("{:-^80}", "");

    for path in sorted_files {
        let content = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read file: {path}", path = path.display()))?;

        let tweet: twitter::Tweet = match serde_json::from_str(&content) {
            Ok(tweet) => tweet,
            Err(e) => {
                warn!(
                    "Failed to parse tweet JSON from {path}: {e}",
                    path = path.display()
                );
                continue;
            }
        };

        // Format modified time as string using datetime utilities
        let time_str = file_times
            .get(&path)
            .map(|t| {
                use std::time::{Duration, UNIX_EPOCH};
                let secs = t
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::from_secs(0))
                    .as_secs();

                format_for_display(&from_unix_timestamp(secs as i64))
            })
            .unwrap_or_else(|| "Unknown".to_string());

        let filename = path.file_name().unwrap_or_default().to_string_lossy();

        println!("ID: {tweet_id}", tweet_id = tweet.id);
        // Handle cases where the author information might be missing or empty
        let author_display = if !tweet.author.username.is_empty() {
            let name_part = tweet.author.name.as_deref().filter(|n| !n.is_empty());
            match name_part {
                Some(name) => format!("{name} (@{username})", username = tweet.author.username),
                None => format!("@{username}", username = tweet.author.username),
            }
        } else if let Some(author_id) = &tweet.author_id {
            format!("ID: {author_id}")
        } else {
            "Unknown".to_string()
        };

        println!("  │ Author: {author_display}");
        println!(
            "Text: {text}",
            text = tweet.text.lines().next().unwrap_or_default()
        );
        println!("Date: {time_str}");
        println!("File: {filename}");
        println!("{:-^80}", "");
    }

    Ok(())
}
