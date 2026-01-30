#![allow(async_fn_in_trait)]

use anyhow::Result;

use crate::ids::{HttpUrl, RelayUrl, TweetId, UnixTimestamp, Username};
use crate::nostr::{NostrEventDraft, NostrEventInfo, NostrEventResult};
use crate::twitter::{Tweet, User};

pub trait Clock {
    fn now(&self) -> UnixTimestamp;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserTweetsQuery {
    pub count: u32,
    pub days: Option<u32>,
    pub since_id: Option<TweetId>,
}

impl UserTweetsQuery {
    pub fn new(count: u32) -> Self {
        Self {
            count,
            days: None,
            since_id: None,
        }
    }
}

pub struct StoredTweet {
    pub id: TweetId,
    pub location: String,
    pub tweet: Tweet,
}

pub struct StoredTweetSummary {
    pub tweet: Tweet,
    pub file_name: String,
    pub modified_at: UnixTimestamp,
}

pub struct MediaAsset {
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

pub trait TwitterPort {
    async fn fetch_tweet(&self, id: &TweetId) -> Result<Tweet>;
    async fn fetch_user_profile(&self, username: &Username) -> Result<User>;
    async fn fetch_user_tweets(
        &self,
        username: &Username,
        query: UserTweetsQuery,
    ) -> Result<Vec<Tweet>>;
    async fn enrich_referenced_tweets(&self, tweet: &mut Tweet) -> Result<()>;
}

pub trait StoragePort {
    async fn save_tweet(&self, tweet: &Tweet) -> Result<StoredTweet>;
    async fn load_tweet(&self, id: &TweetId) -> Result<Option<Tweet>>;
    async fn list_tweets(&self) -> Result<Vec<StoredTweetSummary>>;
    async fn save_user_profile(&self, user: &User) -> Result<String>;
    async fn load_latest_user_profile(&self, username: &Username) -> Result<Option<User>>;
    async fn mark_tweet_not_found(&self, id: &TweetId) -> Result<()>;
    async fn is_tweet_not_found(&self, id: &TweetId) -> Result<bool>;
    async fn find_latest_tweet_id_for_user(&self, username: &Username) -> Result<Option<TweetId>>;
    async fn save_nostr_event_info(&self, info: &NostrEventInfo) -> Result<String>;
    async fn load_nostr_event_info(&self, tweet_id: &TweetId) -> Result<Option<NostrEventInfo>>;
}

pub trait NostrPort {
    async fn publish_event(&self, event: NostrEventDraft) -> Result<NostrEventResult>;
    async fn update_relay_list(&self, relays: &[RelayUrl]) -> Result<()>;
    async fn find_event_by_tweet(&self, tweet_id: &TweetId) -> Result<Option<NostrEventResult>>;
}

pub trait BlossomPort {
    async fn upload_media(&self, assets: &[MediaAsset]) -> Result<Vec<HttpUrl>>;
}
