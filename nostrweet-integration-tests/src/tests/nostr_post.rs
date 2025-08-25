use anyhow::{Context, Result};
use nostr_sdk::prelude::*;
use nostr_sdk::Event;
use serde_json::json;
use std::fs;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test posting various types of tweets to Nostr
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing Nostr posting functionality");

    // Create a test tweet JSON file
    let test_tweet = json!({
        "id": "test123",
        "text": "This is a test tweet with a link https://example.com",
        "author": {
            "username": "testuser",
            "name": "Test User",
            "profile_image_url": "https://example.com/avatar.jpg"
        },
        "created_at": "2024-01-01T00:00:00Z",
        "metrics": {
            "like_count": 10,
            "retweet_count": 5,
            "reply_count": 2
        },
        "media": [
            {
                "type": "photo",
                "url": "https://example.com/image.jpg",
                "width": 1024,
                "height": 768
            }
        ],
        "urls": [
            {
                "display_url": "example.com",
                "expanded_url": "https://example.com",
                "url": "https://t.co/abc123"
            }
        ]
    });

    // Save test tweet to file
    let tweet_file = ctx.output_dir.join("20240101_000000_testuser_test123.json");
    fs::write(&tweet_file, serde_json::to_string_pretty(&test_tweet)?)
        .context("Failed to write test tweet file")?;

    // Test 1: Post regular tweet
    info!("Testing regular tweet post");
    ctx.run_nostrweet(&["post-tweet-to-nostr", "--force", "test123"])
        .await
        .context("Failed to post test tweet")?;

    // Verify on relay
    let keys = Keys::parse(&ctx.private_key)?;
    let client = Client::new(keys.clone());
    client.add_relay(&ctx.relay_url).await?;
    client.connect().await;

    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    let filter = Filter::new().author(keys.public_key()).kind(Kind::TextNote);

    let events = client
        .fetch_events(filter.clone(), std::time::Duration::from_secs(5))
        .await?;

    let event_vec: Vec<Event> = events.into_iter().collect();
    if event_vec.is_empty() {
        anyhow::bail!("No events found after posting test tweet");
    }

    // Verify event content
    let event = &event_vec[0];
    debug!("Posted event content: {content}", content = event.content);

    // Check for expected content
    if !event.content.contains("This is a test tweet") {
        anyhow::bail!("Event content does not match test tweet");
    }

    // Check for URL expansion
    if !event.content.contains("https://example.com") {
        anyhow::bail!("URL was not properly expanded in event");
    }

    // Check for media reference
    if !event.content.contains("image.jpg") {
        anyhow::bail!("Media URL not found in event");
    }

    info!("✅ Regular tweet posted successfully");

    // Test 2: Create and post a reply tweet
    let reply_tweet = json!({
        "id": "reply456",
        "text": "This is a reply to the previous tweet",
        "author": {
            "username": "testuser",
            "name": "Test User",
            "profile_image_url": "https://example.com/avatar.jpg"
        },
        "created_at": "2024-01-01T00:05:00Z",
        "referenced_tweets": [
            {
                "type": "replied_to",
                "id": "test123"
            }
        ],
        "metrics": {
            "like_count": 2,
            "retweet_count": 0,
            "reply_count": 0
        }
    });

    let reply_file = ctx
        .output_dir
        .join("20240101_000500_testuser_reply456.json");
    fs::write(&reply_file, serde_json::to_string_pretty(&reply_tweet)?)
        .context("Failed to write reply tweet file")?;

    info!("Testing reply tweet post");
    ctx.run_nostrweet(&["post-tweet-to-nostr", "--force", "reply456"])
        .await
        .context("Failed to post reply tweet")?;

    // Wait and verify reply
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    let reply_events = client
        .fetch_events(filter, std::time::Duration::from_secs(5))
        .await?;

    // Find the reply event
    let reply_event_vec: Vec<Event> = reply_events.into_iter().collect();
    let reply_event = reply_event_vec
        .iter()
        .find(|e| e.content.contains("This is a reply"))
        .context("Reply event not found")?;

    debug!(
        "Reply event content: {content}",
        content = reply_event.content
    );

    // Check for reply marker
    if !reply_event.content.contains("replied to") {
        anyhow::bail!("Reply marker not found in event");
    }

    info!("✅ Reply tweet posted successfully");

    // Test 3: Test quote tweet
    let quote_tweet = json!({
        "id": "quote789",
        "text": "Check out this tweet!",
        "author": {
            "username": "testuser",
            "name": "Test User",
            "profile_image_url": "https://example.com/avatar.jpg"
        },
        "created_at": "2024-01-01T00:10:00Z",
        "referenced_tweets": [
            {
                "type": "quoted",
                "id": "test123",
                "text": "This is a test tweet with a link https://example.com",
                "author": {
                    "username": "testuser",
                    "name": "Test User"
                }
            }
        ],
        "metrics": {
            "like_count": 5,
            "retweet_count": 2,
            "reply_count": 1
        }
    });

    let quote_file = ctx
        .output_dir
        .join("20240101_001000_testuser_quote789.json");
    fs::write(&quote_file, serde_json::to_string_pretty(&quote_tweet)?)
        .context("Failed to write quote tweet file")?;

    info!("Testing quote tweet post");
    ctx.run_nostrweet(&["post-tweet-to-nostr", "--force", "quote789"])
        .await
        .context("Failed to post quote tweet")?;

    info!("✅ All Nostr posting tests completed successfully");

    Ok(())
}
