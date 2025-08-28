use crate::datetime_utils::parse_compact_datetime;
use crate::error_utils::{
    parse_json_from_reader_with_context, parse_json_with_context, serialize_to_json_with_context,
};
use crate::filename_utils::{
    nostr_event_filename, not_found_filename, sanitized_file_path, tweet_filename,
    user_profile_filename,
};
use crate::twitter::{Tweet, User};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Find an existing tweet JSON file in the output directory
pub fn find_existing_tweet_json(tweet_id: &str, output_dir: &Path) -> Option<PathBuf> {
    if let Ok(entries) = fs::read_dir(output_dir) {
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

/// Search for a tweet JSON file across all likely directories
/// This is useful for finding referenced tweets that might be in different locations
pub fn find_tweet_in_all_directories(
    tweet_id: &str,
    output_dir: &Path,
    cache_dir: Option<&Path>,
) -> Option<PathBuf> {
    // List of directories to search in priority order
    let search_dirs = vec![
        // 1. Primary output directory
        Some(output_dir.to_path_buf()),
        // 2. Optional cache directory if different
        cache_dir.map(PathBuf::from),
    ];

    // Search in each directory
    for dir_option in search_dirs.into_iter().flatten() {
        if dir_option.exists() {
            if let Some(path) = find_existing_tweet_json(tweet_id, &dir_option) {
                debug!(
                    "Found tweet {tweet_id} in {dir}",
                    dir = dir_option.display()
                );
                return Some(path);
            }
        }
    }

    None
}

/// Saves tweet data to a JSON file in the specified directory
pub fn save_tweet(tweet: &Tweet, output_dir: &Path) -> Result<PathBuf> {
    let tweet_id = &tweet.id;

    // Check if we already have a JSON file for this tweet
    if let Some(existing_path) = find_existing_tweet_json(tweet_id, output_dir) {
        info!(
            "Tweet JSON already exists, skipping save: {path}",
            path = existing_path.display()
        );
        return Ok(existing_path);
    }

    // Create a sanitized filename based on tweet ID, creation date, and author
    // Create filename using the tweet's date instead of current time
    let filename = tweet_filename(&tweet.created_at, &tweet.author.username, tweet_id)?;
    let file_path = sanitized_file_path(output_dir, &filename);

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

/// Checks if a tweet ID has been marked as "not found" in the cache.
pub fn is_tweet_not_found(tweet_id: &str, cache_dir: &Path) -> bool {
    let filename = not_found_filename(tweet_id);
    let file_path = sanitized_file_path(cache_dir, &filename);
    let exists = file_path.exists();
    if exists {
        debug!(
            "Tweet {tweet_id} .not_found marker found at {path}",
            path = file_path.display()
        );
    }
    exists
}

/// Marks a tweet ID as "not found" in the cache by creating a .not_found file.
/// Assumes cache_dir exists.
pub fn load_user_from_file(path: &Path) -> Result<User> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let user = parse_json_from_reader_with_context(reader, "user profile")?;
    Ok(user)
}

/// Find the latest (highest) tweet ID for a specific user in the cache
pub fn find_latest_tweet_id_for_user(username: &str, cache_dir: &Path) -> Result<Option<String>> {
    let cache_dir_str = cache_dir
        .to_str()
        .context("Cache directory path contains invalid UTF-8")?;

    // Pattern to match tweet files for this user
    // Format: YYYYMMDD_HHMMSS_username_tweetid.json
    let glob_pattern = format!("{cache_dir_str}/*_{username}_*.json");

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

pub fn find_latest_user_profile(username: &str, cache_dir: &Path) -> Result<Option<PathBuf>> {
    let cache_dir_str = cache_dir
        .to_str()
        .context("Cache directory path contains invalid UTF-8")?;
    let glob_pattern = format!("{cache_dir_str}/??????????????_{username}_*.json");

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
pub fn save_user_profile(user: &User, output_dir: &Path) -> Result<PathBuf> {
    let user_id = &user.id;
    let username = &user.username;

    // Create a sanitized filename based on username and current date
    // Create filename using the current date, username, and user_id
    let filename = user_profile_filename(username, user_id);
    let file_path = sanitized_file_path(output_dir, &filename);

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
pub fn save_nostr_event(event: &nostr_sdk::Event, output_dir: &Path) -> Result<PathBuf> {
    let event_id = event.id.to_hex();
    let filename = nostr_event_filename(&event_id);

    let nostr_events_dir = output_dir.join("nostr_events");
    if !nostr_events_dir.exists() {
        fs::create_dir_all(&nostr_events_dir).context("Failed to create nostr_events directory")?;
    }

    let file_path = sanitized_file_path(&nostr_events_dir, &filename);

    let json = serialize_to_json_with_context(event, "Nostr event")?;
    fs::write(&file_path, json).context("Failed to write Nostr event to file")?;

    debug!("Saved Nostr event to {path}", path = file_path.display());

    Ok(file_path)
}

pub fn mark_tweet_as_not_found(tweet_id: &str, cache_dir: &Path) -> Result<()> {
    let filename = not_found_filename(tweet_id);
    let file_path = sanitized_file_path(cache_dir, &filename);

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
}
