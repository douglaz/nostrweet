use anyhow::Result;
use serde_json::json;
use std::fs;
use std::path::Path;

/// Create mock tweet files to avoid hitting Twitter API during tests
pub fn create_mock_tweets(output_dir: &Path) -> Result<()> {
    // Create a mock tweet for ID 1453856044928933893 (Twitter's "Hello literally everyone")
    let tweet1 = json!({
        "id": "1453856044928933893",
        "text": "Hello literally everyone",
        "author": {
            "id": "783214",
            "username": "Twitter",
            "name": "Twitter",
            "profile_image_url": "https://pbs.twimg.com/profile_images/1683325380441128960/yRsRRjGO_400x400.jpg"
        },
        "author_id": "783214",
        "created_at": "2021-10-29T00:00:00Z",
        "entities": null,
        "attachments": null,
        "referenced_tweets": null,
        "includes": null
    });

    let filename1 = "20211029_000000_Twitter_1453856044928933893.json";
    fs::write(
        output_dir.join(filename1),
        serde_json::to_string_pretty(&tweet1)?,
    )?;

    // Create mock tweets for douglaz user
    let douglaz_tweet1 = json!({
        "id": "1000000000000000001",
        "text": "Test tweet from douglaz #1",
        "author": {
            "id": "123456789",
            "username": "douglaz",
            "name": "Douglas Augusto",
            "profile_image_url": "https://example.com/avatar.jpg"
        },
        "author_id": "123456789",
        "created_at": "2024-01-01T12:00:00Z",
        "entities": null,
        "attachments": null,
        "referenced_tweets": null,
        "includes": null
    });

    let filename2 = "20240101_120000_douglaz_1000000000000000001.json";
    fs::write(
        output_dir.join(filename2),
        serde_json::to_string_pretty(&douglaz_tweet1)?,
    )?;

    let douglaz_tweet2 = json!({
        "id": "1000000000000000002",
        "text": "Test tweet from douglaz #2 with link https://github.com/douglaz/nostrweet",
        "author": {
            "id": "123456789",
            "username": "douglaz",
            "name": "Douglas Augusto",
            "profile_image_url": "https://example.com/avatar.jpg"
        },
        "author_id": "123456789",
        "created_at": "2024-01-01T13:00:00Z",
        "entities": {
            "urls": [{
                "display_url": "github.com/douglaz/nostrweet",
                "expanded_url": "https://github.com/douglaz/nostrweet",
                "url": "https://t.co/abc123"
            }]
        },
        "attachments": null,
        "referenced_tweets": null,
        "includes": null
    });

    let filename3 = "20240101_130000_douglaz_1000000000000000002.json";
    fs::write(
        output_dir.join(filename3),
        serde_json::to_string_pretty(&douglaz_tweet2)?,
    )?;

    // Create mock profile for douglaz
    let profile = json!({
        "id": "123456789",
        "username": "douglaz",
        "name": "Douglas Augusto",
        "description": "Software engineer. Creator of nostrweet.",
        "profile_image_url": "https://example.com/avatar.jpg",
        "url": "https://github.com/douglaz",
        "created_at": "2010-01-01T00:00:00Z",
        "public_metrics": {
            "followers_count": 100,
            "following_count": 50,
            "tweet_count": 1000,
            "listed_count": 5,
            "like_count": 500
        }
    });

    let profile_filename = "20240101_000000_douglaz_profile.json";
    fs::write(
        output_dir.join(profile_filename),
        serde_json::to_string_pretty(&profile)?,
    )?;

    Ok(())
}
