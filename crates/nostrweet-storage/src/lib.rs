#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use nostrweet_core::{
    Clock, CreatedAt, NostrEventInfo, StoragePort, StoredTweet, StoredTweetSummary, Tweet, TweetId,
    UnixTimestamp, User, Username,
};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::warn;

const NOSTR_INFO_DIR: &str = "nostr";

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixTimestamp {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        UnixTimestamp::new(now.as_secs())
    }
}

pub struct FileStorage<C> {
    data_dir: PathBuf,
    clock: C,
}

impl FileStorage<SystemClock> {
    pub fn new(data_dir: impl Into<PathBuf>) -> Result<Self> {
        Self::with_clock(data_dir, SystemClock)
    }
}

impl<C: Clock> FileStorage<C> {
    pub fn with_clock(data_dir: impl Into<PathBuf>, clock: C) -> Result<Self> {
        let data_dir = data_dir.into();
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("Failed to create data directory at {data_dir:?}"))?;
        Ok(Self { data_dir, clock })
    }

    fn sanitize_filename(filename: &str) -> String {
        filename.replace(['\\', '/'], "_")
    }

    fn sanitized_path(&self, filename: &str) -> PathBuf {
        let sanitized = Self::sanitize_filename(filename);
        self.data_dir.join(sanitized)
    }

    fn format_tweet_timestamp(created_at: &CreatedAt) -> Result<String> {
        let parsed = OffsetDateTime::parse(created_at.as_str(), &Rfc3339)
            .with_context(|| format!("Failed to parse tweet created_at {created_at}"))?;
        let format = time::format_description::parse("[year][month][day]_[hour][minute][second]")?;
        parsed
            .format(&format)
            .context("Failed to format tweet timestamp")
    }

    fn format_profile_timestamp(&self) -> Result<String> {
        let now = self.clock.now();
        let raw = i64::try_from(now.value())
            .with_context(|| format!("Timestamp {now} does not fit in i64"))?;
        let parsed = OffsetDateTime::from_unix_timestamp(raw)
            .with_context(|| format!("Failed to parse timestamp {now}"))?;
        let format = time::format_description::parse("[year][month][day][hour][minute][second]")?;
        parsed
            .format(&format)
            .context("Failed to format profile timestamp")
    }

    fn tweet_filename(&self, tweet: &Tweet) -> Result<String> {
        let timestamp = Self::format_tweet_timestamp(&tweet.created_at)?;
        let username = tweet.author.username.as_str();
        let tweet_id = tweet.id.as_str();
        Ok(format!("{timestamp}_{username}_{tweet_id}.json"))
    }

    fn user_profile_filename(&self, user: &User) -> Result<String> {
        let timestamp = self.format_profile_timestamp()?;
        Ok(format!(
            "{timestamp}_{username}_{user_id}.json",
            username = user.username.as_str(),
            user_id = user.id.as_str()
        ))
    }

    fn not_found_filename(tweet_id: &str) -> String {
        format!("{tweet_id}.not_found")
    }

    fn find_existing_tweet_path(&self, id: &TweetId) -> Result<Option<PathBuf>> {
        let id_str = id.as_str();
        for entry in fs::read_dir(&self.data_dir).with_context(|| {
            format!(
                "Failed to read data directory {data_dir:?}",
                data_dir = self.data_dir
            )
        })? {
            let entry = entry.context("Failed to read directory entry")?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            if let Some(filename) = path.file_name().and_then(|name| name.to_str())
                && filename.contains(id_str)
            {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    fn tweet_id_from_filename(filename: &str, username: &Username) -> Option<TweetId> {
        if filename.ends_with("_profile.json") {
            return None;
        }
        if !filename.contains(username.as_str()) {
            return None;
        }

        let stem = filename.strip_suffix(".json")?;
        let (date_part, rest) = stem.split_once('_')?;
        if date_part.len() != 8 || !date_part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let (time_part, _) = rest.split_once('_')?;
        if time_part.len() != 6 || !time_part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }

        let tweet_id = stem.rsplit('_').next()?;
        TweetId::from_digits(tweet_id).ok()
    }

    fn nostr_info_path(&self, tweet_id: &TweetId) -> PathBuf {
        self.data_dir
            .join(NOSTR_INFO_DIR)
            .join(format!("{}.json", tweet_id.as_str()))
    }

    fn read_json<T: serde::de::DeserializeOwned>(&self, path: &Path, desc: &str) -> Result<T> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {desc} file at {path:?}"))?;
        serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse {desc} JSON at {path:?}"))
    }

    fn write_json<T: serde::Serialize>(&self, path: &Path, value: &T, desc: &str) -> Result<()> {
        let json = serde_json::to_string_pretty(value)
            .with_context(|| format!("Failed to serialize {desc} to JSON"))?;
        fs::write(path, json)
            .with_context(|| format!("Failed to write {desc} JSON to {path:?}"))?;
        Ok(())
    }

    fn file_modified_timestamp(metadata: &fs::Metadata, path: &Path) -> Result<UnixTimestamp> {
        let modified = metadata
            .modified()
            .with_context(|| format!("Failed to read modified time for {path:?}"))?;
        let duration = modified
            .duration_since(UNIX_EPOCH)
            .with_context(|| format!("Modified time is before UNIX_EPOCH for {path:?}"))?;
        Ok(UnixTimestamp::new(duration.as_secs()))
    }

    fn best_user_profile_path(&self, username: &Username) -> Result<Option<PathBuf>> {
        let mut latest: Option<(String, PathBuf)> = None;

        for entry in fs::read_dir(&self.data_dir).with_context(|| {
            format!(
                "Failed to read data directory {data_dir:?}",
                data_dir = self.data_dir
            )
        })? {
            let entry = entry.context("Failed to read directory entry")?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let filename = match path.file_name().and_then(|name| name.to_str()) {
                Some(name) => name,
                None => continue,
            };

            let (timestamp, rest) = match filename.split_once('_') {
                Some(parts) => parts,
                None => continue,
            };

            if timestamp.len() != 14 || !timestamp.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            let expected = format!("{username}_", username = username.as_str());
            if !rest.starts_with(&expected) {
                continue;
            }

            if latest
                .as_ref()
                .is_none_or(|(latest_ts, _)| timestamp > latest_ts.as_str())
            {
                latest = Some((timestamp.to_string(), path));
            }
        }

        Ok(latest.map(|(_, path)| path))
    }
}

impl<C: Clock> StoragePort for FileStorage<C> {
    async fn save_tweet(&self, tweet: &Tweet) -> Result<StoredTweet> {
        if let Some(existing) = self.find_existing_tweet_path(&tweet.id)? {
            return Ok(StoredTweet {
                id: tweet.id.clone(),
                location: existing.to_string_lossy().to_string(),
                tweet: tweet.clone(),
            });
        }

        let filename = self.tweet_filename(tweet)?;
        let path = self.sanitized_path(&filename);
        self.write_json(&path, tweet, "tweet")?;
        Ok(StoredTweet {
            id: tweet.id.clone(),
            location: path.to_string_lossy().to_string(),
            tweet: tweet.clone(),
        })
    }

    async fn load_tweet(&self, id: &TweetId) -> Result<Option<Tweet>> {
        let path = match self.find_existing_tweet_path(id)? {
            Some(path) => path,
            None => return Ok(None),
        };
        let tweet = self.read_json(&path, "tweet")?;
        Ok(Some(tweet))
    }

    async fn list_tweets(&self) -> Result<Vec<StoredTweetSummary>> {
        let mut summaries = Vec::new();
        for entry in fs::read_dir(&self.data_dir).with_context(|| {
            format!(
                "Failed to read data directory {data_dir:?}",
                data_dir = self.data_dir
            )
        })? {
            let entry = entry.context("Failed to read directory entry")?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }

            let tweet: Tweet = match self.read_json(&path, "tweet") {
                Ok(tweet) => tweet,
                Err(error) => {
                    warn!("Skipping non-tweet JSON at {path:?}: {error}");
                    continue;
                }
            };

            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            let metadata = fs::metadata(&path)
                .with_context(|| format!("Failed to read metadata for {path:?}"))?;
            let modified_at = Self::file_modified_timestamp(&metadata, &path)?;

            summaries.push(StoredTweetSummary {
                tweet,
                file_name: filename,
                modified_at,
            });
        }

        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.modified_at.value()));
        Ok(summaries)
    }

    async fn save_user_profile(&self, user: &User) -> Result<String> {
        let filename = self.user_profile_filename(user)?;
        let path = self.sanitized_path(&filename);
        self.write_json(&path, user, "user profile")?;
        Ok(path.to_string_lossy().to_string())
    }

    async fn load_latest_user_profile(&self, username: &Username) -> Result<Option<User>> {
        let path = match self.best_user_profile_path(username)? {
            Some(path) => path,
            None => return Ok(None),
        };
        let user = self.read_json(&path, "user profile")?;
        Ok(Some(user))
    }

    async fn mark_tweet_not_found(&self, id: &TweetId) -> Result<()> {
        let filename = Self::not_found_filename(id.as_str());
        let path = self.sanitized_path(&filename);
        fs::write(&path, "")
            .with_context(|| format!("Failed to create not-found marker at {path:?}"))?;
        Ok(())
    }

    async fn is_tweet_not_found(&self, id: &TweetId) -> Result<bool> {
        let filename = Self::not_found_filename(id.as_str());
        let path = self.sanitized_path(&filename);
        Ok(path.exists())
    }

    async fn find_latest_tweet_id_for_user(&self, username: &Username) -> Result<Option<TweetId>> {
        let mut latest: Option<TweetId> = None;

        for entry in fs::read_dir(&self.data_dir).with_context(|| {
            format!(
                "Failed to read data directory {data_dir:?}",
                data_dir = self.data_dir
            )
        })? {
            let entry = entry.context("Failed to read directory entry")?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let filename = match path.file_name().and_then(|name| name.to_str()) {
                Some(name) => name,
                None => continue,
            };
            let Some(tweet_id) = Self::tweet_id_from_filename(filename, username) else {
                continue;
            };

            if latest.as_ref().is_none_or(|current| {
                tweet_id.as_str().len() > current.as_str().len()
                    || (tweet_id.as_str().len() == current.as_str().len()
                        && tweet_id.as_str() > current.as_str())
            }) {
                latest = Some(tweet_id);
            }
        }

        Ok(latest)
    }

    async fn save_nostr_event_info(&self, info: &NostrEventInfo) -> Result<String> {
        let dir = self.data_dir.join(NOSTR_INFO_DIR);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create nostr info dir at {dir:?}"))?;
        let path = self.nostr_info_path(&info.tweet_id);
        self.write_json(&path, info, "nostr event info")?;
        Ok(path.to_string_lossy().to_string())
    }

    async fn load_nostr_event_info(&self, tweet_id: &TweetId) -> Result<Option<NostrEventInfo>> {
        let path = self.nostr_info_path(tweet_id);
        if !path.exists() {
            return Ok(None);
        }
        let info = self.read_json(&path, "nostr event info")?;
        Ok(Some(info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostrweet_core::{CreatedAt, NostrEventId, NostrPubkey, TweetText, UserId, Username};
    use tempfile::TempDir;
    use tokio::time::{Duration, sleep};

    struct FixedClock {
        now: UnixTimestamp,
    }

    impl FixedClock {
        fn new(value: u64) -> Self {
            Self {
                now: UnixTimestamp::new(value),
            }
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> UnixTimestamp {
            self.now
        }
    }

    fn sample_user(username: &str, id: &str) -> Result<User> {
        Ok(User::new(UserId::parse(id)?, Username::parse(username)?))
    }

    fn sample_tweet(id: &str, username: &str, created_at: &str) -> Result<Tweet> {
        let user = sample_user(username, "42")?;
        Ok(Tweet::new(
            TweetId::parse(id)?,
            TweetText::parse("hello")?,
            user,
            CreatedAt::parse(created_at)?,
        ))
    }

    #[tokio::test]
    async fn save_and_load_tweet_roundtrip() -> Result<()> {
        let temp = TempDir::new()?;
        let storage = FileStorage::with_clock(temp.path(), FixedClock::new(1))?;
        let tweet = sample_tweet("123", "tester", "2023-01-01T00:00:00Z")?;

        let stored = storage.save_tweet(&tweet).await?;
        assert!(stored.location.contains("20230101_000000_tester_123.json"));

        let loaded = storage.load_tweet(&tweet.id).await?;
        assert_eq!(loaded, Some(tweet));
        Ok(())
    }

    #[tokio::test]
    async fn save_tweet_skips_existing() -> Result<()> {
        let temp = TempDir::new()?;
        let storage = FileStorage::with_clock(temp.path(), FixedClock::new(1))?;
        let tweet = sample_tweet("555", "tester", "2023-01-01T00:00:00Z")?;

        let first = storage.save_tweet(&tweet).await?;
        let second = storage.save_tweet(&tweet).await?;

        assert_eq!(first.location, second.location);
        Ok(())
    }

    #[tokio::test]
    async fn list_tweets_sorts_by_mtime() -> Result<()> {
        let temp = TempDir::new()?;
        let storage = FileStorage::with_clock(temp.path(), FixedClock::new(1))?;
        let first = sample_tweet("100", "tester", "2023-01-01T00:00:00Z")?;
        let second = sample_tweet("200", "tester", "2023-01-02T00:00:00Z")?;

        storage.save_tweet(&first).await?;
        sleep(Duration::from_millis(1100)).await;
        storage.save_tweet(&second).await?;

        let list = storage.list_tweets().await?;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].tweet.id.as_str(), "200");
        assert_eq!(list[1].tweet.id.as_str(), "100");
        Ok(())
    }

    #[tokio::test]
    async fn save_and_load_user_profile() -> Result<()> {
        let temp = TempDir::new()?;
        let clock = FixedClock::new(1_700_000_000);
        let storage = FileStorage::with_clock(temp.path(), clock)?;
        let user = sample_user("tester", "99")?;

        let path = storage.save_user_profile(&user).await?;
        assert!(path.contains("_tester_99.json"));

        let loaded = storage
            .load_latest_user_profile(&Username::parse("tester")?)
            .await?;
        assert_eq!(loaded, Some(user));
        Ok(())
    }

    #[tokio::test]
    async fn mark_and_check_not_found() -> Result<()> {
        let temp = TempDir::new()?;
        let storage = FileStorage::with_clock(temp.path(), FixedClock::new(1))?;
        let id = TweetId::parse("123")?;

        assert!(!storage.is_tweet_not_found(&id).await?);
        storage.mark_tweet_not_found(&id).await?;
        assert!(storage.is_tweet_not_found(&id).await?);
        Ok(())
    }

    #[tokio::test]
    async fn find_latest_tweet_id_for_user_ignores_profiles() -> Result<()> {
        let temp = TempDir::new()?;
        let storage = FileStorage::with_clock(temp.path(), FixedClock::new(1))?;

        let filename = "20230101_000000_tester_100.json";
        fs::write(temp.path().join(filename), "{}")?;
        let filename = "20230102_000000_tester_300.json";
        fs::write(temp.path().join(filename), "{}")?;
        let filename = "20230103_000000_tester_200.json";
        fs::write(temp.path().join(filename), "{}")?;
        let filename = "20230104_000000_tester_profile.json";
        fs::write(temp.path().join(filename), "{}")?;

        let latest = storage
            .find_latest_tweet_id_for_user(&Username::parse("tester")?)
            .await?;
        assert_eq!(
            latest.map(|id| id.as_str().to_string()),
            Some("300".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn save_and_load_nostr_event_info() -> Result<()> {
        let temp = TempDir::new()?;
        let storage = FileStorage::with_clock(temp.path(), FixedClock::new(1))?;
        let info = NostrEventInfo {
            tweet_id: TweetId::parse("123")?,
            event_id: NostrEventId::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )?,
            pubkey: NostrPubkey::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )?,
            created_at: UnixTimestamp::new(1_700_000_000),
            media_urls: Vec::new(),
            relays: Vec::new(),
            event_json: None,
        };

        let path = storage.save_nostr_event_info(&info).await?;
        assert!(path.contains("/nostr/123.json"));

        let loaded = storage
            .load_nostr_event_info(&TweetId::parse("123")?)
            .await?;
        assert_eq!(loaded, Some(info));
        Ok(())
    }

    #[tokio::test]
    async fn load_latest_user_profile_uses_timestamp_prefix() -> Result<()> {
        let temp = TempDir::new()?;
        let storage = FileStorage::with_clock(temp.path(), FixedClock::new(1))?;
        let username = Username::parse("tester")?;

        let early = sample_user("tester", "1")?;
        let late = sample_user("tester", "2")?;

        let early_path = temp.path().join("20230101120000_tester_1.json");
        let late_path = temp.path().join("20230102120000_tester_2.json");

        storage.write_json(&early_path, &early, "user profile")?;
        storage.write_json(&late_path, &late, "user profile")?;

        let loaded = storage.load_latest_user_profile(&username).await?;
        assert_eq!(loaded, Some(late));
        Ok(())
    }
}
