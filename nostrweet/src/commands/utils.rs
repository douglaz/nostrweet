use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use std::fs;
use tracing::info;

#[allow(clippy::too_many_arguments)]
pub async fn query_events(
    relays: Vec<String>,
    kind: Option<u32>,
    author: Option<String>,
    limit: usize,
    since: Option<u64>,
    until: Option<u64>,
    format: String,
    output: Option<String>,
) -> Result<()> {
    info!("Querying events from {} relay(s)", relays.len());

    // Initialize Nostr client without keys (read-only)
    let client = Client::default();

    // Add relays
    for relay in &relays {
        client
            .add_relay(relay)
            .await
            .with_context(|| format!("Failed to add relay: {relay}"))?;
    }

    // Connect to relays
    client.connect().await;

    // Build filter
    let mut filter = Filter::new();

    // Add kind filter if specified
    if let Some(k) = kind {
        let event_kind = Kind::from(k as u16);
        filter = filter.kind(event_kind);
    }

    // Add author filter if specified
    if let Some(author_str) = author {
        // Try to parse as npub first, then as hex
        let pubkey = if author_str.starts_with("npub") {
            PublicKey::from_bech32(&author_str)
                .with_context(|| format!("Invalid npub format: {author_str}"))?
        } else {
            PublicKey::from_hex(&author_str)
                .with_context(|| format!("Invalid hex public key: {author_str}"))?
        };
        filter = filter.author(pubkey);
    }

    // Add time filters
    if let Some(since_timestamp) = since {
        filter = filter.since(Timestamp::from(since_timestamp));
    }

    if let Some(until_timestamp) = until {
        filter = filter.until(Timestamp::from(until_timestamp));
    }

    // Set limit
    filter = filter.limit(limit);

    // Query events
    info!("Querying events with filter: {:?}", filter);

    // Fetch events from relays
    let timeout = std::time::Duration::from_secs(10);
    let events = client
        .fetch_events(filter, timeout)
        .await
        .context("Failed to fetch events from relays")?;

    info!("Retrieved {} events", events.len());

    // Format output
    let output_str = match format.as_str() {
        "json" => {
            // JSON format - array of full event objects
            let json_events: Vec<String> = events.iter().map(|e| e.as_json()).collect();
            // Combine into a JSON array string
            let json_array = format!("[{}]", json_events.join(","));
            // Pretty print the JSON
            let parsed: serde_json::Value =
                serde_json::from_str(&json_array).context("Failed to parse JSON array")?;
            serde_json::to_string_pretty(&parsed).context("Failed to serialize events to JSON")?
        }
        _ => {
            // Pretty format - human-readable summary
            let mut output = String::new();
            output.push_str(&format!("Found {} events\n", events.len()));
            output.push_str("═".repeat(80).as_str());
            output.push('\n');

            for (i, event) in events.iter().enumerate() {
                output.push_str(&format!("\n📝 Event {} of {}\n", i + 1, events.len()));
                output.push_str("─".repeat(40).as_str());
                output.push('\n');

                // Event ID
                output.push_str(&format!("ID: {}\n", event.id));

                // Author
                output.push_str(&format!("Author: {}\n", event.pubkey));

                // Kind
                let kind_desc = match event.kind {
                    Kind::Metadata => "Metadata (0)",
                    Kind::TextNote => "Text Note (1)",
                    Kind::ContactList => "Contact List (3)",
                    Kind::Repost => "Repost (6)",
                    Kind::Reaction => "Reaction (7)",
                    _ => &format!("Kind {}", event.kind.as_u16()),
                };
                output.push_str(&format!("Type: {kind_desc}\n"));

                // Timestamp
                let timestamp = event.created_at;
                let datetime =
                    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp.as_u64() as i64, 0)
                        .unwrap_or_default();
                output.push_str(&format!(
                    "Created: {} ({})\n",
                    datetime.format("%Y-%m-%d %H:%M:%S UTC"),
                    timestamp.as_u64()
                ));

                // Tags summary
                let tags = &event.tags;
                if !tags.is_empty() {
                    output.push_str(&format!("Tags: {} tag(s)\n", tags.len()));
                    for tag in tags.iter().take(3) {
                        output.push_str(&format!("  - {tag:?}\n"));
                    }
                    if tags.len() > 3 {
                        output.push_str(&format!("  ... and {} more\n", tags.len() - 3));
                    }
                }

                // Content preview
                output.push_str("\nContent:\n");
                let content = &event.content;

                // Special handling for metadata events
                if event.kind == Kind::Metadata {
                    // Try to parse and pretty-print metadata
                    if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(content) {
                        if let Ok(pretty) = serde_json::to_string_pretty(&metadata) {
                            output.push_str(&pretty);
                        } else {
                            output.push_str(content);
                        }
                    } else {
                        output.push_str(content);
                    }
                } else {
                    // For other events, show first 500 chars
                    if content.len() > 500 {
                        output.push_str(&content[..500]);
                        output
                            .push_str(&format!("\n... ({} more characters)", content.len() - 500));
                    } else {
                        output.push_str(content);
                    }
                }
                output.push('\n');
            }

            output.push_str("═".repeat(80).as_str());
            output.push('\n');
            output
        }
    };

    // Save to file or print to stdout
    if let Some(output_path) = output {
        fs::write(&output_path, &output_str)
            .with_context(|| format!("Failed to write output to {output_path}"))?;
        info!("Output saved to {output_path}");
    } else {
        println!("{output_str}");
    }

    // Disconnect from relays
    client.disconnect().await;

    Ok(())
}
