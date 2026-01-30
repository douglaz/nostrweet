use anyhow::{Context, Result};
use nostr_sdk::Event;
use nostr_sdk::prelude::*;
use nostrweet_core::{
    CreatedAt, Entities, HttpUrl, ReferenceKind, ReferencedTweet, Tweet, TweetId, TweetText,
    UrlEntity, User, UserId, Username,
};
use std::fs;
use tracing::{debug, info};

use crate::test_runner::TestContext;

/// Test posting various types of tweets to Nostr
pub async fn run(ctx: &TestContext) -> Result<()> {
    info!("Testing Nostr posting functionality");

    let author = build_test_user()?;

    let base_tweet = Tweet::builder()
        .id(TweetId::parse("123456789")?)
        .text(TweetText::parse(
            "This is a test tweet with a link https://example.com",
        )?)
        .author(author.clone())
        .created_at(CreatedAt::parse("2024-01-01T00:00:00Z")?)
        .entities(Entities {
            urls: vec![UrlEntity {
                url: HttpUrl::parse("https://t.co/abc123")?,
                expanded_url: Some(HttpUrl::parse("https://example.com")?),
                display_url: "example.com".to_string(),
            }],
            mentions: Vec::new(),
            hashtags: Vec::new(),
        })
        .author_id(author.id.clone())
        .build();

    write_tweet(ctx, &base_tweet, "20240101_000000_testuser_123456789.json")?;

    // Test 1: Post regular tweet
    info!("Testing regular tweet post");
    ctx.run_nostrweet(&["post-tweet-to-nostr", "--force", "123456789"])
        .await
        .context("Failed to post test tweet")?;

    // Verify on relay
    // The actual posting uses mnemonic-based key derivation, so we can't use ctx.private_key
    // Instead, we need to query without filtering by author, or skip this verification
    // For now, let's query all events and verify the content exists
    let keys = Keys::parse(&ctx.private_key)?;
    let client = Client::new(keys.clone());
    client.add_relay(&ctx.relay_url).await?;
    client.connect().await;

    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // Query for all text note events (not filtered by author since we don't know the derived key)
    let filter = Filter::new().kind(Kind::TextNote).limit(10);

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

    // Media check is optional as not all tweets have media
    if event.content.contains(".jpg")
        || event.content.contains(".png")
        || event.content.contains(".mp4")
    {
        info!("  ✓ Media URL found in event");
    } else {
        debug!("  Note: No media URL in event (tweet may not have media)");
    }

    info!("✅ Regular tweet posted successfully");

    // Test 2: Create and post a reply tweet
    let reply_tweet = Tweet::builder()
        .id(TweetId::parse("987654321")?)
        .text(TweetText::parse("This is a reply to the previous tweet")?)
        .author(author.clone())
        .created_at(CreatedAt::parse("2024-01-01T00:05:00Z")?)
        .referenced_tweets(vec![ReferencedTweet {
            id: base_tweet.id.clone(),
            kind: ReferenceKind::RepliedTo,
            data: Some(Box::new(base_tweet.clone())),
        }])
        .author_id(author.id.clone())
        .build();

    write_tweet(ctx, &reply_tweet, "20240101_000500_testuser_987654321.json")?;

    info!("Testing reply tweet post");
    ctx.run_nostrweet(&["post-tweet-to-nostr", "--force", "987654321"])
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

    // Check for reply marker (using ↩️ emoji or "Reply to" text)
    if !reply_event.content.contains("Reply to") && !reply_event.content.contains("↩️") {
        anyhow::bail!("Reply marker not found in event");
    }

    info!("✅ Reply tweet posted successfully");

    // Test 3: Test quote tweet
    let quote_tweet = Tweet::builder()
        .id(TweetId::parse("555444333")?)
        .text(TweetText::parse("Check out this tweet!")?)
        .author(author)
        .created_at(CreatedAt::parse("2024-01-01T00:10:00Z")?)
        .referenced_tweets(vec![ReferencedTweet {
            id: base_tweet.id.clone(),
            kind: ReferenceKind::Quoted,
            data: Some(Box::new(base_tweet)),
        }])
        .author_id(UserId::parse("987654321")?)
        .build();

    write_tweet(ctx, &quote_tweet, "20240101_001000_testuser_555444333.json")?;

    info!("Testing quote tweet post");
    ctx.run_nostrweet(&["post-tweet-to-nostr", "--force", "555444333"])
        .await
        .context("Failed to post quote tweet")?;

    info!("✅ All Nostr posting tests completed successfully");

    Ok(())
}

fn build_test_user() -> Result<User> {
    Ok(User {
        id: UserId::parse("987654321")?,
        name: Some("Test User".to_string()),
        username: Username::parse("testuser")?,
        profile_image_url: Some(HttpUrl::parse("https://example.com/avatar.jpg")?),
        description: None,
        url: None,
        entities: None,
    })
}

fn write_tweet(ctx: &TestContext, tweet: &Tweet, filename: &str) -> Result<()> {
    let tweet_file = ctx.output_dir.join(filename);
    fs::write(&tweet_file, serde_json::to_string_pretty(tweet)?)
        .context("Failed to write test tweet file")?;
    Ok(())
}
