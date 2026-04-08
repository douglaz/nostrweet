use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::ids::{HttpUrl, NostrEventId, NostrPubkey, RelayUrl, TweetId, UnixTimestamp};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NostrEventInfo {
    pub tweet_id: TweetId,
    pub event_id: NostrEventId,
    pub pubkey: NostrPubkey,
    pub created_at: UnixTimestamp,
    pub media_urls: Vec<HttpUrl>,
    pub relays: Vec<RelayUrl>,
    pub event_json: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NostrEventDraft {
    pub content: String,
    #[serde(default)]
    pub tags: Vec<NostrTag>,
    pub created_at: Option<UnixTimestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NostrEventResult {
    pub event_id: NostrEventId,
    pub event_json: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NostrTag {
    pub name: String,
    #[serde(default)]
    pub values: Vec<String>,
}

impl NostrTag {
    pub fn new(name: impl Into<String>, values: Vec<String>) -> Result<Self> {
        let name = name.into();
        ensure!(!name.trim().is_empty(), "Tag name cannot be empty");
        Ok(Self { name, values })
    }

    pub fn r(url: &HttpUrl) -> Self {
        Self {
            name: "r".to_string(),
            values: vec![url.as_str().to_string()],
        }
    }

    pub fn p(pubkey: &NostrPubkey) -> Self {
        Self {
            name: "p".to_string(),
            values: vec![pubkey.as_str().to_string()],
        }
    }

    pub fn media(url: &HttpUrl) -> Self {
        Self {
            name: "media".to_string(),
            values: vec![url.as_str().to_string()],
        }
    }

    pub fn source(url: &HttpUrl) -> Self {
        Self {
            name: "source".to_string(),
            values: vec![url.as_str().to_string()],
        }
    }

    pub fn client() -> Self {
        Self {
            name: "client".to_string(),
            values: vec!["nostrweet".to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CreatedAt, HttpUrl, TweetId, UserId, Username};
    use crate::twitter::{Tweet, TweetText, User};

    #[test]
    fn nostr_tag_constructors() -> Result<()> {
        let url = HttpUrl::parse("https://example.com")?;
        let tag = NostrTag::r(&url);
        assert_eq!(tag.name, "r");
        assert_eq!(tag.values, vec!["https://example.com/".to_string()]);
        Ok(())
    }

    #[test]
    fn nostr_event_info_serializes() -> Result<()> {
        let tweet = Tweet::new(
            TweetId::parse("123")?,
            TweetText::parse("hello")?,
            User::new(UserId::parse("1")?, Username::parse("tester")?),
            CreatedAt::parse("2023-01-01T00:00:00Z")?,
        );

        let info = NostrEventInfo {
            tweet_id: tweet.id.clone(),
            event_id: NostrEventId::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )?,
            pubkey: NostrPubkey::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )?,
            created_at: tweet.created_at.unix_timestamp()?,
            media_urls: Vec::new(),
            relays: Vec::new(),
            event_json: None,
        };

        let serialized = serde_json::to_string(&info)?;
        assert!(serialized.contains("event_id"));
        Ok(())
    }
}
