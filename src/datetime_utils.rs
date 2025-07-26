use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};

/// Common date/time formats used throughout the application
pub mod formats {
    /// Format for filenames with separator: "20240120_153000"
    pub const FILENAME_WITH_SEPARATOR: &str = "%Y%m%d_%H%M%S";

    /// Compact format for filenames: "20240120153000"
    pub const FILENAME_COMPACT: &str = "%Y%m%d%H%M%S";

    /// Human-readable format for display: "2024-01-20 15:30:00"
    pub const DISPLAY_FULL: &str = "%Y-%m-%d %H:%M:%S";

    /// Date-only format for display: "2024-01-20"
    pub const DISPLAY_DATE: &str = "%Y-%m-%d";
}

/// Parse an RFC3339/ISO 8601 datetime string (e.g., from Twitter API)
pub fn parse_rfc3339(date_str: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(date_str)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("Failed to parse RFC3339 date: {date_str}"))
}

/// Parse a datetime string in compact format (e.g., "20240120153000")
pub fn parse_compact_datetime(date_str: &str) -> Result<NaiveDateTime> {
    NaiveDateTime::parse_from_str(date_str, formats::FILENAME_COMPACT)
        .with_context(|| format!("Failed to parse compact datetime: {date_str}"))
}

/// Format a datetime for use in filenames with separator
pub fn format_for_filename(datetime: &DateTime<Utc>) -> String {
    datetime
        .format(formats::FILENAME_WITH_SEPARATOR)
        .to_string()
}

/// Format a datetime in compact format (no separators)
pub fn format_compact(datetime: &DateTime<Utc>) -> String {
    datetime.format(formats::FILENAME_COMPACT).to_string()
}

/// Format a datetime for human-readable display
pub fn format_for_display(datetime: &DateTime<Utc>) -> String {
    datetime.format(formats::DISPLAY_FULL).to_string()
}

/// Format a date (without time) for display
pub fn format_date_only(datetime: &DateTime<Utc>) -> String {
    datetime.format(formats::DISPLAY_DATE).to_string()
}

/// Get current UTC timestamp
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

/// Get current Unix timestamp (seconds since epoch)
#[allow(dead_code)]
pub fn unix_timestamp_now() -> i64 {
    Utc::now().timestamp()
}

/// Convert Unix timestamp to DateTime
pub fn from_unix_timestamp(timestamp: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(timestamp, 0).unwrap_or_else(|| {
        // If the provided timestamp is invalid, return Unix epoch
        DateTime::from_timestamp(0, 0).expect("Unix epoch timestamp should always be valid")
    })
}

/// Parse a tweet date and format it for use in filenames
pub fn parse_and_format_tweet_date(tweet_date: &str) -> Result<String> {
    let parsed = parse_rfc3339(tweet_date)?;
    Ok(format_for_filename(&parsed))
}

/// Check if a date is within the last N days
pub fn is_within_days(datetime: &DateTime<Utc>, days: u32) -> bool {
    let now = Utc::now();
    let duration = now.signed_duration_since(datetime);
    duration.num_days() <= days as i64 && duration.num_days() >= 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rfc3339() {
        let date_str = "2024-01-20T15:30:00Z";
        let parsed = parse_rfc3339(date_str).unwrap();
        assert_eq!(parsed.timestamp(), 1705764600);

        // Test with timezone offset
        let date_str_tz = "2024-01-20T15:30:00+00:00";
        let parsed_tz = parse_rfc3339(date_str_tz).unwrap();
        assert_eq!(parsed_tz.timestamp(), 1705764600);
    }

    #[test]
    fn test_parse_compact_datetime() {
        let date_str = "20240120153000";
        let parsed = parse_compact_datetime(date_str).unwrap();
        assert_eq!(
            parsed.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-01-20 15:30:00"
        );
    }

    #[test]
    fn test_format_for_filename() {
        let dt = from_unix_timestamp(1705764600); // 2024-01-20 15:30:00 UTC
        assert_eq!(format_for_filename(&dt), "20240120_153000");
    }

    #[test]
    fn test_format_compact() {
        let dt = from_unix_timestamp(1705764600); // 2024-01-20 15:30:00 UTC
        assert_eq!(format_compact(&dt), "20240120153000");
    }

    #[test]
    fn test_format_for_display() {
        let dt = from_unix_timestamp(1705764600); // 2024-01-20 15:30:00 UTC
        assert_eq!(format_for_display(&dt), "2024-01-20 15:30:00");
    }

    #[test]
    fn test_format_date_only() {
        let dt = from_unix_timestamp(1705764600); // 2024-01-20 15:30:00 UTC
        assert_eq!(format_date_only(&dt), "2024-01-20");
    }

    #[test]
    fn test_parse_and_format_tweet_date() {
        let tweet_date = "2024-01-20T15:30:00Z";
        let formatted = parse_and_format_tweet_date(tweet_date).unwrap();
        assert_eq!(formatted, "20240120_153000");
    }

    #[test]
    fn test_is_within_days() {
        let now = Utc::now();
        let yesterday = now - chrono::Duration::days(1);
        let week_ago = now - chrono::Duration::days(7);
        let month_ago = now - chrono::Duration::days(30);

        assert!(is_within_days(&yesterday, 2));
        assert!(!is_within_days(&yesterday, 0));
        assert!(is_within_days(&week_ago, 7));
        assert!(!is_within_days(&week_ago, 6));
        assert!(!is_within_days(&month_ago, 7));
    }

    #[test]
    fn test_from_unix_timestamp() {
        let timestamp = 1705764600;
        let dt = from_unix_timestamp(timestamp);
        assert_eq!(dt.timestamp(), timestamp);

        // Test edge case with 0
        let dt_zero = from_unix_timestamp(0);
        assert_eq!(dt_zero.timestamp(), 0);
    }
}
