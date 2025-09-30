use crate::datetime_utils::parse_compact_datetime;
use crate::error_utils::{
    parse_json_from_reader_with_context, parse_json_with_context, serialize_to_json_with_context,
};
use crate::filename_utils::{
    nostr_event_filename, not_found_filename, sanitized_file_path, tweet_filename,
    user_profile_filename,
};
#[cfg(test)]
use crate::twitter::{NoteTweet, ReferencedTweet};
use crate::twitter::{Tweet, User};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Find an existing tweet JSON file in the output directory
pub fn find_existing_tweet_json(tweet_id: &str, data_dir: &Path) -> Option<PathBuf> {
    if let Ok(entries) = fs::read_dir(data_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(extension) = path.extension() {
                if extension == "json" {
                    if let Some(filename) = path.file_name() {
                        let filename = filename.to_string_lossy();
                        // Check if this JSON file contains our tweet ID
                        if filename.contains(tweet_id) && path.is_file() {
                            // Found an existing JSON file for this tweet
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Saves tweet data to a JSON file in the specified directory
pub fn save_tweet(tweet: &Tweet, data_dir: &Path) -> Result<PathBuf> {
    let tweet_id = &tweet.id;

    // Check if we already have a JSON file for this tweet
    if let Some(existing_path) = find_existing_tweet_json(tweet_id, data_dir) {
        info!(
            "Tweet JSON already exists, skipping save: {path}",
            path = existing_path.display()
        );
        return Ok(existing_path);
    }

    // Create a sanitized filename based on tweet ID, creation date, and author
    // Create filename using the tweet's date instead of current time
    let filename = tweet_filename(&tweet.created_at, &tweet.author.username, tweet_id)?;
    let file_path = sanitized_file_path(data_dir, &filename);

    // Serialize tweet to JSON and write to file
    let json = serialize_to_json_with_context(tweet, "tweet")?;
    fs::write(&file_path, json).context("Failed to write tweet JSON to file")?;

    info!("Saved tweet data to {path}", path = file_path.display());

    Ok(file_path)
}

/// Load a tweet from a local JSON file
pub fn load_tweet_from_file(file_path: &Path) -> Result<Tweet> {
    let json_content = fs::read_to_string(file_path).context("Failed to read tweet JSON file")?;

    let tweet: Tweet = parse_json_with_context(&json_content, "tweet data")?;

    info!(
        "Loaded tweet data from local file: {path}",
        path = file_path.display()
    );

    Ok(tweet)
}

/// Checks if a tweet ID has been marked as "not found" in the data directory.
pub fn is_tweet_not_found(tweet_id: &str, data_dir: &Path) -> bool {
    let filename = not_found_filename(tweet_id);
    let file_path = sanitized_file_path(data_dir, &filename);
    let exists = file_path.exists();
    if exists {
        debug!(
            "Tweet {tweet_id} .not_found marker found at {path}",
            path = file_path.display()
        );
    }
    exists
}

/// Load a user profile from a JSON file.
pub fn load_user_from_file(path: &Path) -> Result<User> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let user = parse_json_from_reader_with_context(reader, "user profile")?;
    Ok(user)
}

/// Find the latest (highest) tweet ID for a specific user in the data directory
pub fn find_latest_tweet_id_for_user(username: &str, data_dir: &Path) -> Result<Option<String>> {
    let data_dir_str = data_dir
        .to_str()
        .context("Data directory path contains invalid UTF-8")?;

    // Pattern to match tweet files for this user
    // Format: YYYYMMDD_HHMMSS_username_tweetid.json
    let glob_pattern = format!("{data_dir_str}/*_{username}_*.json");

    let mut latest_tweet_id: Option<String> = None;

    for path in glob::glob(&glob_pattern)?.flatten() {
        if let Some(filename) = path.file_stem() {
            let filename_str = filename.to_string_lossy();

            // Skip profile files (they end with _profile)
            if filename_str.ends_with("_profile") {
                continue;
            }

            // Extract tweet ID (last part after the last underscore)
            if let Some(tweet_id) = filename_str.rsplit('_').next() {
                // Twitter IDs are snowflake IDs that increase over time
                // So we can compare them as strings to find the latest
                if latest_tweet_id
                    .as_ref()
                    .is_none_or(|latest| tweet_id > latest.as_str())
                {
                    latest_tweet_id = Some(tweet_id.to_string());
                }
            }
        }
    }

    if let Some(ref id) = latest_tweet_id {
        debug!("Found latest tweet ID for @{username}: {id}");
    } else {
        debug!("No cached tweets found for @{username}");
    }

    Ok(latest_tweet_id)
}

pub fn find_latest_user_profile(username: &str, data_dir: &Path) -> Result<Option<PathBuf>> {
    let data_dir_str = data_dir
        .to_str()
        .context("Data directory path contains invalid UTF-8")?;
    let glob_pattern = format!("{data_dir_str}/??????????????_{username}_*.json");

    let mut latest_file: Option<(chrono::NaiveDateTime, PathBuf)> = None;

    for path in glob::glob(&glob_pattern)?.flatten() {
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(timestamp_str) = filename.split('_').next() {
                if let Ok(timestamp) = parse_compact_datetime(timestamp_str) {
                    if let Some((latest_timestamp, _)) = &latest_file {
                        if timestamp > *latest_timestamp {
                            latest_file = Some((timestamp, path));
                        }
                    } else {
                        latest_file = Some((timestamp, path));
                    }
                }
            }
        }
    }

    Ok(latest_file.map(|(_, path)| path))
}

/// Saves user profile data to a JSON file in the specified directory
pub fn save_user_profile(user: &User, data_dir: &Path) -> Result<PathBuf> {
    let user_id = &user.id;
    let username = &user.username;

    // Create a sanitized filename based on username and current date
    // Create filename using the current date, username, and user_id
    let filename = user_profile_filename(username, user_id);
    let file_path = sanitized_file_path(data_dir, &filename);

    // Serialize user to JSON and write to file
    let json = serialize_to_json_with_context(user, "user profile")?;
    fs::write(&file_path, json).context("Failed to write user profile JSON to file")?;

    info!(
        "Saved user profile data to {path}",
        path = file_path.display()
    );

    Ok(file_path)
}

/// Saves a Nostr event to a JSON file.
pub fn save_nostr_event(event: &nostr_sdk::Event, data_dir: &Path) -> Result<PathBuf> {
    let event_id = event.id.to_hex();
    let filename = nostr_event_filename(&event_id);

    let nostr_events_dir = data_dir.join("nostr_events");
    if !nostr_events_dir.exists() {
        fs::create_dir_all(&nostr_events_dir).context("Failed to create nostr_events directory")?;
    }

    let file_path = sanitized_file_path(&nostr_events_dir, &filename);

    let json = serialize_to_json_with_context(event, "Nostr event")?;
    fs::write(&file_path, json).context("Failed to write Nostr event to file")?;

    debug!("Saved Nostr event to {path}", path = file_path.display());

    Ok(file_path)
}

pub fn mark_tweet_as_not_found(tweet_id: &str, data_dir: &Path) -> Result<()> {
    let filename = not_found_filename(tweet_id);
    let file_path = sanitized_file_path(data_dir, &filename);

    // Create an empty file. fs::write will create or truncate.
    fs::write(&file_path, "").with_context(|| {
        format!(
            "Failed to create/write .not_found file for tweet {tweet_id} at {path}",
            path = file_path.display()
        )
    })?;
    debug!(
        "Marked tweet {tweet_id} as not found at {path}",
        path = file_path.display()
    );
    Ok(())
}

// ============================================================================
// Helper Functions for Common Tweet Loading and Enrichment Pattern
// ============================================================================

/// Load a tweet from cache or fetch from API with automatic enrichment of referenced tweets
///
/// This function provides a consistent way to load tweets across all commands:
/// 1. First checks the local cache for the tweet
/// 2. If found in cache, ensures referenced tweets are enriched
/// 3. If not in cache, fetches from Twitter API
/// 4. Always enriches referenced tweets to get full content (including note_tweet)
/// 5. Saves the enriched tweet to cache
///
/// This prevents issues with truncated referenced tweets (retweets/quotes) that
/// occur when the API's initial expansion doesn't include note_tweet fields.
pub async fn load_or_fetch_tweet(
    tweet_id: &str,
    data_dir: &Path,
    bearer_token: Option<&str>,
) -> Result<Tweet> {
    // Step 1: Check if we have the tweet in cache
    if let Some(existing_path) = find_existing_tweet_json(tweet_id, data_dir) {
        debug!(
            "Found existing tweet data at {path}",
            path = existing_path.display()
        );
        let mut tweet = load_tweet_from_file(&existing_path)
            .with_context(|| format!("Failed to load existing tweet data for {tweet_id}"))?;

        // Step 2: Ensure referenced tweets are enriched (may need API call)
        ensure_tweet_enriched(&mut tweet, data_dir, bearer_token)
            .await
            .with_context(|| format!("Failed to enrich referenced tweets for {tweet_id}"))?;

        return Ok(tweet);
    }

    // Step 3: Not in cache - need to fetch from Twitter API
    debug!("Tweet {tweet_id} not found locally, downloading from Twitter API");

    let bearer =
        bearer_token.ok_or_else(|| anyhow::anyhow!("Bearer token required to download tweet"))?;

    let client = crate::twitter::TwitterClient::new(data_dir, bearer)
        .context("Failed to initialize Twitter client")?;

    let mut tweet = client
        .get_tweet(tweet_id)
        .await
        .with_context(|| format!("Failed to download tweet {tweet_id}"))?;

    // Step 4: Always enrich referenced tweets for complete data
    if let Err(e) = client
        .enrich_referenced_tweets(&mut tweet, Some(data_dir))
        .await
    {
        debug!("Failed to enrich referenced tweets: {e}");
        // Continue with the tweet even if enrichment fails
    }

    // Step 5: Save the enriched tweet to cache
    let saved_path = save_tweet(&tweet, data_dir)
        .with_context(|| format!("Failed to save tweet data for {tweet_id}"))?;
    debug!("Saved tweet data to {path}", path = saved_path.display());

    Ok(tweet)
}

/// Ensure a tweet has all its referenced tweets fully enriched with complete data
///
/// This function checks if a tweet has unenriched referenced tweets (missing data
/// or missing note_tweet for long content) and fetches the complete data if needed.
///
/// This is crucial for retweets and quotes to display their full content instead
/// of truncated previews.
pub async fn ensure_tweet_enriched(
    tweet: &mut Tweet,
    data_dir: &Path,
    bearer_token: Option<&str>,
) -> Result<()> {
    // Check if we have referenced tweets that need enrichment
    let has_unenriched_refs = tweet
        .referenced_tweets
        .as_ref()
        .map(|refs| refs.iter().any(|r| r.data.is_none()))
        .unwrap_or(false);

    if has_unenriched_refs && bearer_token.is_some() {
        debug!("Some referenced tweets need enrichment, fetching from Twitter API");
        let bearer = bearer_token.unwrap();
        let client = crate::twitter::TwitterClient::new(data_dir, bearer)
            .context("Failed to initialize Twitter client for enriching referenced tweets")?;

        // This will fetch full tweet data including note_tweet for long tweets
        client
            .enrich_referenced_tweets(tweet, Some(data_dir))
            .await
            .context("Failed to enrich referenced tweets from API")?;

        info!("Successfully enriched referenced tweets");
    } else if has_unenriched_refs {
        debug!("Referenced tweets need enrichment but no bearer token available");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_tweet() -> Tweet {
        Tweet {
            id: "123456789".to_string(),
            text: "Test tweet content".to_string(),
            author: User {
                id: "987654321".to_string(),
                name: Some("Test User".to_string()),
                username: "testuser".to_string(),
                profile_image_url: None,
                description: None,
                url: None,
                entities: None,
            },
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: None,
            includes: None,
            author_id: Some("987654321".to_string()),
            note_tweet: None,
        }
    }

    fn create_test_user() -> User {
        User {
            id: "987654321".to_string(),
            name: Some("Test User".to_string()),
            username: "testuser".to_string(),
            profile_image_url: Some("https://example.com/profile.jpg".to_string()),
            description: Some("Test user description".to_string()),
            url: Some("https://example.com".to_string()),
            entities: None,
        }
    }

    #[test]
    fn test_find_existing_tweet_json() {
        let temp_dir = TempDir::new().unwrap();
        let tweet_id = "123456789";

        // No file exists yet
        assert!(find_existing_tweet_json(tweet_id, temp_dir.path()).is_none());

        // Create a tweet JSON file
        let filename = format!("20230101_000000_testuser_{tweet_id}.json");
        let file_path = temp_dir.path().join(&filename);
        fs::write(&file_path, "{}").unwrap();

        // Now it should find the file
        let found = find_existing_tweet_json(tweet_id, temp_dir.path());
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().file_name().unwrap().to_str().unwrap(),
            filename
        );
    }

    #[test]
    fn test_save_and_load_tweet() {
        let temp_dir = TempDir::new().unwrap();
        let tweet = create_test_tweet();

        // Save tweet
        let saved_path = save_tweet(&tweet, temp_dir.path()).unwrap();
        assert!(saved_path.exists());

        // Load tweet back
        let loaded_tweet = load_tweet_from_file(&saved_path).unwrap();
        assert_eq!(loaded_tweet.id, tweet.id);
        assert_eq!(loaded_tweet.text, tweet.text);
        assert_eq!(loaded_tweet.author.username, tweet.author.username);
    }

    #[test]
    fn test_save_tweet_twice_returns_existing() {
        let temp_dir = TempDir::new().unwrap();
        let tweet = create_test_tweet();

        // Save tweet first time
        let first_path = save_tweet(&tweet, temp_dir.path()).unwrap();

        // Save same tweet again
        let second_path = save_tweet(&tweet, temp_dir.path()).unwrap();

        // Should return the same path
        assert_eq!(first_path, second_path);
    }

    #[test]
    fn test_is_tweet_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let tweet_id = "123456789";

        // Initially not marked as not found
        assert!(!is_tweet_not_found(tweet_id, temp_dir.path()));

        // Mark as not found
        mark_tweet_as_not_found(tweet_id, temp_dir.path()).unwrap();

        // Now it should be marked as not found
        assert!(is_tweet_not_found(tweet_id, temp_dir.path()));
    }

    #[test]
    fn test_save_and_load_user() {
        let temp_dir = TempDir::new().unwrap();
        let user = create_test_user();

        // Save user
        let saved_path = save_user_profile(&user, temp_dir.path()).unwrap();
        assert!(saved_path.exists());

        // Load user back
        let loaded_user = load_user_from_file(&saved_path).unwrap();
        assert_eq!(loaded_user.id, user.id);
        assert_eq!(loaded_user.username, user.username);
        assert_eq!(loaded_user.name, user.name);
    }

    #[test]
    fn test_find_latest_user_profile() {
        let temp_dir = TempDir::new().unwrap();
        let username = "testuser";

        // Create multiple user profile files with different timestamps
        let file1 = temp_dir
            .path()
            .join(format!("20230101120000_{username}_profile.json"));
        let file2 = temp_dir
            .path()
            .join(format!("20230102120000_{username}_profile.json"));
        let file3 = temp_dir
            .path()
            .join(format!("20230103120000_{username}_profile.json"));

        fs::write(&file1, "{}").unwrap();
        fs::write(&file2, "{}").unwrap();
        fs::write(&file3, "{}").unwrap();

        // Should find the latest one
        let latest = find_latest_user_profile(username, temp_dir.path()).unwrap();
        assert!(latest.is_some());
        assert_eq!(
            latest.unwrap().file_name().unwrap(),
            file3.file_name().unwrap()
        );
    }

    #[test]
    fn test_find_latest_user_profile_no_files() {
        let temp_dir = TempDir::new().unwrap();
        let username = "testuser";

        // No files exist
        let latest = find_latest_user_profile(username, temp_dir.path()).unwrap();
        assert!(latest.is_none());
    }

    #[test]
    fn test_find_latest_tweet_id_for_user() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let username = "testuser";

        // Initially no tweets
        let result = find_latest_tweet_id_for_user(username, temp_dir.path())?;
        assert!(result.is_none());

        // Create some tweet files with different IDs
        let file1 = temp_dir
            .path()
            .join(format!("20230101_120000_{username}_1000.json"));
        let file2 = temp_dir
            .path()
            .join(format!("20230102_120000_{username}_2000.json"));
        let file3 = temp_dir
            .path()
            .join(format!("20230103_120000_{username}_1500.json"));

        fs::write(&file1, "{}")?;
        fs::write(&file2, "{}")?;
        fs::write(&file3, "{}")?;

        // Should find the highest tweet ID (2000)
        let result = find_latest_tweet_id_for_user(username, temp_dir.path())?;
        assert_eq!(result, Some("2000".to_string()));

        // Add a profile file (should be ignored)
        let profile = temp_dir
            .path()
            .join(format!("20230104_120000_{username}_profile.json"));
        fs::write(&profile, "{}")?;

        // Should still return 2000
        let result = find_latest_tweet_id_for_user(username, temp_dir.path())?;
        assert_eq!(result, Some("2000".to_string()));

        Ok(())
    }

    #[test]
    fn test_sanitize_filename_special_chars() {
        let tweet = Tweet {
            id: "123456789".to_string(),
            text: "Test tweet with special chars: <>&*?".to_string(),
            author: User {
                id: "987654321".to_string(),
                name: Some("Test User".to_string()),
                username: "test/user\\name".to_string(),
                profile_image_url: None,
                description: None,
                url: None,
                entities: None,
            },
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: None,
            includes: None,
            author_id: Some("987654321".to_string()),
            note_tweet: None,
        };

        let temp_dir = TempDir::new().unwrap();

        // Save should succeed even with special chars in username
        let saved_path = save_tweet(&tweet, temp_dir.path()).unwrap();
        assert!(saved_path.exists());

        // Filename should be sanitized
        let filename = saved_path.file_name().unwrap().to_str().unwrap();
        assert!(!filename.contains('/'));
        assert!(!filename.contains('\\'));
    }

    #[test]
    fn test_ensure_tweet_enriched_detects_unenriched_refs() {
        // Create a tweet with unenriched referenced tweet (simulating the bug case)
        let mut tweet = Tweet {
            id: "1965148820234535067".to_string(),
            text: "RT @elonmusk: This is a test".to_string(),
            author: User {
                id: "987654321".to_string(),
                name: Some("Test User".to_string()),
                username: "testuser".to_string(),
                profile_image_url: None,
                description: None,
                url: None,
                entities: None,
            },
            referenced_tweets: Some(vec![ReferencedTweet {
                id: "1234567890".to_string(),
                type_field: "retweeted".to_string(),
                // This is the key: data is None, meaning unenriched
                data: None,
            }]),
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: None,
            includes: None,
            author_id: Some("987654321".to_string()),
            note_tweet: None,
        };

        // Check that the tweet needs enrichment
        let needs_enrichment = tweet
            .referenced_tweets
            .as_ref()
            .map(|refs| refs.iter().any(|r| r.data.is_none()))
            .unwrap_or(false);

        assert!(
            needs_enrichment,
            "Tweet with unenriched referenced tweets should be detected as needing enrichment"
        );

        // Now simulate enrichment - adding data to the referenced tweet
        if let Some(refs) = tweet.referenced_tweets.as_mut() {
            for referenced in refs.iter_mut() {
                if referenced.data.is_none() {
                    // In real code, this would be fetched from API
                    referenced.data = Some(Box::new(Tweet {
                        id: referenced.id.clone(),
                        text: "This is the full text of the referenced tweet that was previously truncated...".to_string(),
                        author: User {
                            id: "11111".to_string(),
                            name: Some("Elon Musk".to_string()),
                            username: "elonmusk".to_string(),
                            profile_image_url: None,
                            description: None,
                            url: None,
                            entities: None,
                        },
                        referenced_tweets: None,
                        attachments: None,
                        created_at: "2023-01-01T00:00:00Z".to_string(),
                        entities: None,
                        includes: None,
                        author_id: Some("11111".to_string()),
                        note_tweet: Some(NoteTweet {
                            text: "This is the extended full text of the tweet that would be longer than 280 characters and might have been cut off with ellipsis in the truncated version...".to_string(),
                        }),
                    }));
                }
            }
        }

        // After enrichment, it should not need enrichment anymore
        let needs_enrichment_after = tweet
            .referenced_tweets
            .as_ref()
            .map(|refs| refs.iter().any(|r| r.data.is_none()))
            .unwrap_or(false);

        assert!(
            !needs_enrichment_after,
            "After enrichment, tweet should not need enrichment"
        );

        // Verify the referenced tweet now has full data
        let ref_tweet_data = &tweet.referenced_tweets.as_ref().unwrap()[0].data;
        assert!(
            ref_tweet_data.is_some(),
            "Referenced tweet should have data after enrichment"
        );

        let ref_tweet = ref_tweet_data.as_ref().unwrap();
        assert!(
            ref_tweet.note_tweet.is_some(),
            "Referenced tweet should have note_tweet for long content"
        );
    }
}
