use crate::datetime_utils::{format_compact, now_utc, parse_and_format_tweet_date};
use anyhow::Result;
use sanitize_filename::sanitize;
use std::path::{Path, PathBuf};

/// Utility functions for generating consistent filenames across the application
///
/// Generate a filename for a tweet JSON file
/// Format: YYYYMMDD_HHMMSS_username_tweetid.json
pub fn tweet_filename(tweet_date: &str, username: &str, tweet_id: &str) -> Result<String> {
    let parsed_date = parse_and_format_tweet_date(tweet_date)?;
    Ok(format!("{parsed_date}_{username}_{tweet_id}.json"))
}

/// Generate a filename for a user profile JSON file
/// Format: YYYYMMDDHHMISS_username_userid.json
pub fn user_profile_filename(username: &str, user_id: &str) -> String {
    let current_date = format_compact(&now_utc());
    format!("{current_date}_{username}_{user_id}.json")
}

/// Generate a filename for media files
/// Format: username_mediakey.extension
pub fn media_filename(username: &str, media_key: &str, file_extension: &str) -> String {
    // Clean media key by removing prefixes like "3_" or "7_"
    let clean_media_key = if let Some(underscore_pos) = media_key.find('_') {
        &media_key[underscore_pos + 1..]
    } else {
        media_key
    };

    format!("{username}_{clean_media_key}.{file_extension}")
}

/// Sanitize and create full file path
pub fn sanitized_file_path(output_dir: &Path, filename: &str) -> PathBuf {
    let sanitized_filename = sanitize(filename);
    output_dir.join(sanitized_filename)
}

/// Generate a filename for Nostr event JSON files
/// Format: eventid.json
pub fn nostr_event_filename(event_id: &str) -> String {
    format!("{event_id}.json")
}

/// Generate a not-found marker filename
/// Format: tweetid.not_found
pub fn not_found_filename(tweet_id: &str) -> String {
    format!("{tweet_id}.not_found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_tweet_filename() {
        let filename = tweet_filename("2023-01-15T10:30:00.000Z", "testuser", "123456789").unwrap();
        assert_eq!(filename, "20230115_103000_testuser_123456789.json");
    }

    #[test]
    fn test_tweet_filename_invalid_date() {
        let result = tweet_filename("invalid-date", "testuser", "123456789");
        assert!(result.is_err());
    }

    #[test]
    fn test_user_profile_filename() {
        let filename = user_profile_filename("testuser", "987654321");
        // Should contain the pattern but date will vary
        assert!(filename.contains("_testuser_987654321.json"));
        assert!(filename.len() > 25); // Should have timestamp prefix
    }

    #[test]
    fn test_media_filename() {
        let filename = media_filename("testuser", "3_1234567890", "jpg");
        assert_eq!(filename, "testuser_1234567890.jpg");

        let filename = media_filename("testuser", "1234567890", "mp4");
        assert_eq!(filename, "testuser_1234567890.mp4");
    }

    #[test]
    fn test_sanitized_file_path() {
        let temp_dir = TempDir::new().unwrap();
        let path = sanitized_file_path(temp_dir.path(), "test/file\\name.json");

        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(!filename.contains('/'));
        assert!(!filename.contains('\\'));
    }

    #[test]
    fn test_nostr_event_filename() {
        let filename = nostr_event_filename("abc123def456");
        assert_eq!(filename, "abc123def456.json");
    }

    #[test]
    fn test_not_found_filename() {
        let filename = not_found_filename("123456789");
        assert_eq!(filename, "123456789.not_found");
    }
}
