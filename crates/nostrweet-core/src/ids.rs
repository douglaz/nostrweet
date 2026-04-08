use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::fmt;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TweetId(String);

impl TweetId {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "Tweet ID cannot be empty");

        if trimmed.chars().all(|c| c.is_ascii_digit()) {
            return Ok(Self(trimmed.to_string()));
        }

        if let Ok(parsed_url) = Url::parse(trimmed)
            && parsed_url
                .host_str()
                .is_some_and(|h| h.contains("twitter.com") || h.contains("x.com"))
        {
            let path_segments: Vec<&str> = parsed_url
                .path_segments()
                .map_or(Vec::new(), |s| s.collect());
            if path_segments.len() >= 3 && path_segments[1] == "status" {
                return Self::from_digits(path_segments[2]);
            }
        }

        let re = regex::Regex::new(r"(?:twitter\.com|x\.com)/\w+/status/(\d+)")?;
        if let Some(captures) = re.captures(trimmed)
            && let Some(id_match) = captures.get(1)
        {
            return Self::from_digits(id_match.as_str());
        }

        bail!("Could not extract tweet ID from: {trimmed}")
    }

    pub fn from_digits(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "Tweet ID cannot be empty");
        ensure!(
            trimmed.chars().all(|c| c.is_ascii_digit()),
            "Tweet ID must be numeric"
        );
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TweetId {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        TweetId::from_digits(&value).map_err(|err| err.to_string())
    }
}

impl From<TweetId> for String {
    fn from(value: TweetId) -> Self {
        value.0
    }
}

impl fmt::Display for TweetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UserId(String);

impl UserId {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "User ID cannot be empty");
        ensure!(
            trimmed.chars().all(|c| c.is_ascii_digit()),
            "User ID must be numeric"
        );
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for UserId {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        UserId::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<UserId> for String {
    fn from(value: UserId) -> Self {
        value.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Username(String);

impl Username {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "Username cannot be empty");

        let without_at = trimmed.strip_prefix('@').unwrap_or(trimmed);
        ensure!(!without_at.is_empty(), "Username cannot be empty");
        ensure!(
            without_at
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "Username contains invalid characters"
        );
        ensure!(
            without_at.chars().count() <= 15,
            "Username exceeds 15 character limit"
        );

        Ok(Self(without_at.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn normalized(&self) -> String {
        self.0.to_lowercase()
    }
}

impl TryFrom<String> for Username {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        Username::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<Username> for String {
    fn from(value: Username) -> Self {
        value.0
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MediaKey(String);

impl MediaKey {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "Media key cannot be empty");
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn cleaned(&self) -> &str {
        self.0
            .split_once('_')
            .map(|(_, suffix)| suffix)
            .unwrap_or(&self.0)
    }
}

impl TryFrom<String> for MediaKey {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        MediaKey::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<MediaKey> for String {
    fn from(value: MediaKey) -> Self {
        value.0
    }
}

impl fmt::Display for MediaKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HttpUrl(String);

impl HttpUrl {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "URL cannot be empty");
        let url = Url::parse(trimmed)?;
        ensure!(
            url.scheme() == "http" || url.scheme() == "https",
            "URL must use http or https"
        );
        Ok(Self(url.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HttpUrl {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        HttpUrl::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<HttpUrl> for String {
    fn from(value: HttpUrl) -> Self {
        value.0
    }
}

impl fmt::Display for HttpUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelayUrl(String);

impl RelayUrl {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "Relay URL cannot be empty");
        let url = Url::parse(trimmed)?;
        let scheme = url.scheme();
        ensure!(
            matches!(scheme, "ws" | "wss" | "http" | "https"),
            "Relay URL must use ws, wss, http, or https"
        );
        Ok(Self(url.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RelayUrl {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        RelayUrl::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<RelayUrl> for String {
    fn from(value: RelayUrl) -> Self {
        value.0
    }
}

impl fmt::Display for RelayUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BlossomUrl(HttpUrl);

impl BlossomUrl {
    pub fn parse(input: &str) -> Result<Self> {
        Ok(Self(HttpUrl::parse(input)?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<String> for BlossomUrl {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        BlossomUrl::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<BlossomUrl> for String {
    fn from(value: BlossomUrl) -> Self {
        value.0.into()
    }
}

impl fmt::Display for BlossomUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NostrEventId(String);

impl NostrEventId {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "Event ID cannot be empty");
        ensure!(trimmed.len() == 64, "Event ID must be 64 hex chars");
        hex::decode(trimmed)?;
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NostrEventId {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        NostrEventId::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<NostrEventId> for String {
    fn from(value: NostrEventId) -> Self {
        value.0
    }
}

impl fmt::Display for NostrEventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NostrPubkey(String);

impl NostrPubkey {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "Pubkey cannot be empty");
        ensure!(trimmed.len() == 64, "Pubkey must be 64 hex chars");
        hex::decode(trimmed)?;
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NostrPubkey {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        NostrPubkey::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<NostrPubkey> for String {
    fn from(value: NostrPubkey) -> Self {
        value.0
    }
}

impl fmt::Display for NostrPubkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixTimestamp(u64);

impl UnixTimestamp {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for UnixTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CreatedAt(String);

impl CreatedAt {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        ensure!(!trimmed.is_empty(), "created_at cannot be empty");
        OffsetDateTime::parse(trimmed, &Rfc3339)?;
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn unix_timestamp(&self) -> Result<UnixTimestamp> {
        let parsed = OffsetDateTime::parse(&self.0, &Rfc3339)?;
        ensure!(
            parsed.unix_timestamp() >= 0,
            "Timestamp must be non-negative"
        );
        Ok(UnixTimestamp::new(parsed.unix_timestamp() as u64))
    }
}

impl TryFrom<String> for CreatedAt {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        CreatedAt::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<CreatedAt> for String {
    fn from(value: CreatedAt) -> Self {
        value.0
    }
}

impl fmt::Display for CreatedAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tweet_id_variants() -> Result<()> {
        let id = TweetId::parse("1234567890")?;
        assert_eq!(id.as_str(), "1234567890");

        let id = TweetId::parse("https://twitter.com/user/status/1234567890")?;
        assert_eq!(id.as_str(), "1234567890");

        let id = TweetId::parse("https://x.com/user/status/1234567890?s=20")?;
        assert_eq!(id.as_str(), "1234567890");

        assert!(TweetId::parse("not-a-url").is_err());
        assert!(TweetId::parse("").is_err());
        Ok(())
    }

    #[test]
    fn parse_username_rules() -> Result<()> {
        let username = Username::parse("@test_user")?;
        assert_eq!(username.as_str(), "test_user");
        assert_eq!(username.normalized(), "test_user");

        assert!(Username::parse("bad name").is_err());
        assert!(Username::parse("").is_err());
        Ok(())
    }

    #[test]
    fn parse_http_url_rules() -> Result<()> {
        let url = HttpUrl::parse("https://example.com/path")?;
        assert_eq!(url.as_str(), "https://example.com/path");
        assert!(HttpUrl::parse("ftp://example.com").is_err());
        Ok(())
    }

    #[test]
    fn created_at_parses_rfc3339() -> Result<()> {
        let created = CreatedAt::parse("2023-01-01T00:00:00Z")?;
        assert_eq!(created.as_str(), "2023-01-01T00:00:00Z");
        let timestamp = created.unix_timestamp()?;
        assert!(timestamp.value() > 0);
        Ok(())
    }
}
