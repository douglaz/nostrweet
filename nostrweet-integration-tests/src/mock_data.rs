use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use std::fs;
use std::path::Path;

/// Create mock tweet files to avoid hitting Twitter API during tests
pub fn create_mock_tweets(output_dir: &Path) -> Result<()> {
    // Use current timestamp for all tweets
    let now = Utc::now();
    let timestamp = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let filename_timestamp = now.format("%Y%m%d_%H%M%S").to_string();

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
        "created_at": timestamp,
        "entities": null,
        "attachments": null,
        "referenced_tweets": null,
        "includes": null
    });

    let filename1 = format!("{filename_timestamp}_Twitter_1453856044928933893.json");
    fs::write(
        output_dir.join(filename1),
        serde_json::to_string_pretty(&tweet1)?,
    )?;

    // Create mock tweets for douglaz user
    let douglaz_tweet1 = json!({
        "id": "1000000000000000001",
        "text": "Test tweet from douglaz #1 with an image",
        "author": {
            "id": "123456789",
            "username": "douglaz",
            "name": "Douglas Augusto",
            "profile_image_url": "https://example.com/avatar.jpg"
        },
        "author_id": "123456789",
        "created_at": timestamp.clone(),
        "entities": null,
        "attachments": {
            "media_keys": ["3_1000000000000000001"]
        },
        "referenced_tweets": null,
        "includes": {
            "media": [{
                "media_key": "3_1000000000000000001",
                "type": "photo",
                "url": "https://pbs.twimg.com/media/example_image.jpg"
            }]
        }
    });

    let filename2 = format!("{filename_timestamp}_douglaz_1000000000000000001.json");
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
        "created_at": timestamp.clone(),
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

    let filename3 = format!("{filename_timestamp}_douglaz_1000000000000000002.json");
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

    let profile_filename = format!("{filename_timestamp}_douglaz_profile.json");
    fs::write(
        output_dir.join(profile_filename),
        serde_json::to_string_pretty(&profile)?,
    )?;

    // Create test tweet for nostr_post test
    let test_tweet = json!({
        "id": "123456789",
        "text": "This is a test tweet with a link https://example.com",
        "author": {
            "id": "987654321",
            "username": "testuser",
            "name": "Test User",
            "profile_image_url": "https://example.com/avatar.jpg"
        },
        "author_id": "987654321",
        "created_at": timestamp.clone(),
        "entities": {
            "urls": [{
                "display_url": "example.com",
                "expanded_url": "https://example.com",
                "url": "https://t.co/abc123"
            }]
        },
        "attachments": null,
        "referenced_tweets": null,
        "includes": null
    });

    let test_filename = format!("{filename_timestamp}_testuser_123456789.json");
    fs::write(
        output_dir.join(test_filename),
        serde_json::to_string_pretty(&test_tweet)?,
    )?;

    // Create a few more tweets for the batch/user-tweets tests
    for i in 3..6 {
        let tweet = json!({
            "id": format!("100000000000000000{}", i),
            "text": format!("Test tweet #{} from douglaz", i),
            "author": {
                "id": "123456789",
                "username": "douglaz",
                "name": "Douglas Augusto",
                "profile_image_url": "https://example.com/avatar.jpg"
            },
            "author_id": "123456789",
            "created_at": timestamp.clone(),
            "entities": null,
            "attachments": null,
            "referenced_tweets": null,
            "includes": null
        });

        let filename = format!("{filename_timestamp}_douglaz_100000000000000000{i}.json");
        fs::write(
            output_dir.join(filename),
            serde_json::to_string_pretty(&tweet)?,
        )?;
    }

    Ok(())
}
