#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use nostrweet_core::{
    Attachments, CreatedAt, Entities, Hashtag, Includes, Media, MediaKind, MediaVariant, Mention,
    NoteTweet, ReferenceKind, ReferencedTweet, TweetText, UrlEntity, UserEntities, UserId,
    UserUrlEntity,
};
use nostrweet_core::{
    Clock, Tweet, TweetId, TwitterPort, UnixTimestamp, User, UserTweetsQuery, Username,
};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, instrument};

#[derive(Debug, Clone)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixTimestamp {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        UnixTimestamp::new(now.as_secs())
    }
}

#[derive(Debug, Clone)]
pub enum TwitterAdapterError {
    TweetNotFound { tweet_id: String },
    UserNotFound { username: String },
}

impl fmt::Display for TwitterAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TwitterAdapterError::TweetNotFound { tweet_id } => {
                write!(f, "Tweet not found: {tweet_id}")
            }
            TwitterAdapterError::UserNotFound { username } => {
                write!(f, "User not found: {username}")
            }
        }
    }
}

impl std::error::Error for TwitterAdapterError {}

#[derive(Debug, Clone)]
pub struct InMemoryTwitter<C: Clock> {
    clock: C,
    tweets_by_id: HashMap<String, Tweet>,
    users_by_username: HashMap<String, User>,
    users_by_id: HashMap<String, User>,
}

impl InMemoryTwitter<SystemClock> {
    pub fn new() -> Self {
        Self::with_clock(SystemClock)
    }
}

impl Default for InMemoryTwitter<SystemClock> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Clock> InMemoryTwitter<C> {
    pub fn with_clock(clock: C) -> Self {
        Self {
            clock,
            tweets_by_id: HashMap::new(),
            users_by_username: HashMap::new(),
            users_by_id: HashMap::new(),
        }
    }

    pub fn insert_user(&mut self, user: User) {
        let username_key = user.username.normalized();
        let user_id_key = user.id.as_str().to_string();
        self.users_by_username.insert(username_key, user.clone());
        self.users_by_id.insert(user_id_key, user);
    }

    pub fn insert_tweet(&mut self, tweet: Tweet) {
        let id_key = tweet.id.as_str().to_string();
        let author = tweet.author.clone();
        self.tweets_by_id.insert(id_key, tweet);
        if !self
            .users_by_username
            .contains_key(&author.username.normalized())
        {
            self.insert_user(author);
        }
    }

    fn normalize_username(username: &Username) -> String {
        username.normalized()
    }

    fn tweet_id_is_newer(candidate: &TweetId, since: &TweetId) -> bool {
        let candidate_str = candidate.as_str();
        let since_str = since.as_str();
        if candidate_str.len() != since_str.len() {
            return candidate_str.len() > since_str.len();
        }
        candidate_str > since_str
    }

    fn user_not_found(username: &Username) -> anyhow::Error {
        user_not_found(username)
    }

    fn tweet_not_found(tweet_id: &TweetId) -> anyhow::Error {
        tweet_not_found(tweet_id)
    }
}

fn user_not_found(username: &Username) -> anyhow::Error {
    anyhow::Error::new(TwitterAdapterError::UserNotFound {
        username: username.as_str().to_string(),
    })
}

fn tweet_not_found(tweet_id: &TweetId) -> anyhow::Error {
    anyhow::Error::new(TwitterAdapterError::TweetNotFound {
        tweet_id: tweet_id.as_str().to_string(),
    })
}

#[allow(clippy::large_enum_variant)]
impl<C: Clock + Send + Sync> TwitterPort for InMemoryTwitter<C> {
    #[instrument(name = "twitter.fetch_tweet", skip(self))]
    async fn fetch_tweet(&self, id: &TweetId) -> Result<Tweet> {
        let tweet = self
            .tweets_by_id
            .get(id.as_str())
            .cloned()
            .with_context(|| format!("Tweet {id} not found"))
            .map_err(|_| Self::tweet_not_found(id))?;
        Ok(tweet)
    }

    #[instrument(name = "twitter.fetch_user_profile", skip(self))]
    async fn fetch_user_profile(&self, username: &Username) -> Result<User> {
        let key = Self::normalize_username(username);
        let user = self
            .users_by_username
            .get(&key)
            .cloned()
            .with_context(|| format!("User {username} not found"))
            .map_err(|_| Self::user_not_found(username))?;
        Ok(user)
    }

    #[instrument(name = "twitter.fetch_user_tweets", skip(self))]
    async fn fetch_user_tweets(
        &self,
        username: &Username,
        query: UserTweetsQuery,
    ) -> Result<Vec<Tweet>> {
        let key = Self::normalize_username(username);
        let mut tweets: Vec<Tweet> = self
            .tweets_by_id
            .values()
            .filter(|tweet| tweet.author.username.normalized() == key)
            .cloned()
            .collect();

        if let Some(since_id) = &query.since_id {
            tweets.retain(|tweet| Self::tweet_id_is_newer(&tweet.id, since_id));
        }

        if let Some(days) = query.days {
            let now = self.clock.now().value();
            let window = u64::from(days).saturating_mul(86_400);
            let cutoff = now.saturating_sub(window);

            let mut filtered = Vec::new();
            for tweet in tweets {
                let timestamp = tweet
                    .created_at
                    .unix_timestamp()
                    .context("Failed to parse tweet created_at")?;
                if timestamp.value() >= cutoff {
                    filtered.push(tweet);
                }
            }
            tweets = filtered;
        }

        let mut with_ts = Vec::with_capacity(tweets.len());
        for tweet in tweets {
            let timestamp = tweet
                .created_at
                .unix_timestamp()
                .context("Failed to parse tweet created_at")?;
            with_ts.push((tweet, timestamp.value()));
        }

        with_ts.sort_by(|(a, a_ts), (b, b_ts)| {
            b_ts.cmp(a_ts)
                .then_with(|| b.id.as_str().cmp(a.id.as_str()))
        });

        let mut sorted: Vec<Tweet> = with_ts.into_iter().map(|(tweet, _)| tweet).collect();

        let requested = query.count as usize;
        if requested < sorted.len() {
            sorted.truncate(requested);
        }

        Ok(sorted)
    }

    #[instrument(name = "twitter.enrich_referenced_tweets", skip(self, tweet))]
    async fn enrich_referenced_tweets(&self, tweet: &mut Tweet) -> Result<()> {
        for reference in &mut tweet.referenced_tweets {
            if reference.data.is_some() {
                continue;
            }

            if let Some(referenced) = self.tweets_by_id.get(reference.id.as_str()) {
                reference.data = Some(Box::new(referenced.clone()));
                continue;
            }

            debug!(
                "Referenced tweet {} not found in fixture store",
                reference.id.as_str()
            );
        }

        Ok(())
    }
}

const TWITTER_API_BASE: &str = "https://api.twitter.com/2";

#[derive(Debug, Clone)]
pub struct TwitterClient {
    client: Client,
    bearer_token: String,
}

impl TwitterClient {
    pub fn new(bearer_token: impl Into<String>) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to build Twitter HTTP client")?;
        Ok(Self {
            client,
            bearer_token: bearer_token.into(),
        })
    }

    fn request(&self, url: &str) -> reqwest::RequestBuilder {
        self.client.get(url).bearer_auth(&self.bearer_token)
    }

    async fn get_user_by_username(&self, username: &Username) -> Result<User> {
        let url = format!(
            "{TWITTER_API_BASE}/users/by/username/{username}?user.fields=name,username,profile_image_url,description,url,entities"
        );
        let response = self
            .request(&url)
            .send()
            .await
            .context("Twitter user lookup failed")?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(user_not_found(username));
        }

        let body: ApiUserResponse = response
            .json()
            .await
            .context("Failed to parse Twitter user response")?;
        convert_user(&body.data)
    }

    async fn fetch_tweet_by_id(&self, tweet_id: &TweetId) -> Result<ApiTweetResponse> {
        let url = format!(
            "{TWITTER_API_BASE}/tweets/{id}?tweet.fields=created_at,entities,referenced_tweets,author_id,note_tweet&expansions=attachments.media_keys,referenced_tweets.id,author_id,referenced_tweets.id.attachments.media_keys&user.fields=name,username,profile_image_url,description,url,entities&media.fields=url,preview_image_url,alt_text,variants,media_key,type",
            id = tweet_id.as_str()
        );
        let response = self
            .request(&url)
            .send()
            .await
            .context("Twitter tweet lookup failed")?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(tweet_not_found(tweet_id));
        }

        response
            .json::<ApiTweetResponse>()
            .await
            .context("Failed to parse Twitter tweet response")
    }

    async fn fetch_timeline_page(
        &self,
        user_id: &UserId,
        max_results: usize,
        pagination_token: Option<&str>,
        since_id: Option<&TweetId>,
    ) -> Result<ApiTimelineResponse> {
        let mut url = format!(
            "{TWITTER_API_BASE}/users/{id}/tweets?tweet.fields=created_at,entities,referenced_tweets,author_id,note_tweet&expansions=attachments.media_keys,referenced_tweets.id,author_id,referenced_tweets.id.attachments.media_keys&user.fields=name,username,profile_image_url,description,url,entities&media.fields=url,preview_image_url,alt_text,variants,media_key,type&max_results={max_results}",
            id = user_id.as_str(),
            max_results = max_results.clamp(5, 100)
        );
        if let Some(token) = pagination_token {
            url.push_str("&pagination_token=");
            url.push_str(token);
        }
        if let Some(since_id) = since_id {
            url.push_str("&since_id=");
            url.push_str(since_id.as_str());
        }

        let response = self
            .request(&url)
            .send()
            .await
            .context("Twitter timeline request failed")?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow::anyhow!("User timeline not found for id {user_id}"));
        }

        response
            .json::<ApiTimelineResponse>()
            .await
            .context("Failed to parse Twitter timeline response")
    }
}

#[derive(Debug, Deserialize)]
struct ApiUserResponse {
    data: ApiUser,
}

#[derive(Debug, Deserialize)]
struct ApiTweetResponse {
    data: ApiTweet,
    #[serde(default)]
    includes: Option<ApiIncludes>,
}

#[derive(Debug, Deserialize)]
struct ApiTimelineResponse {
    #[serde(default)]
    data: Option<Vec<ApiTweet>>,
    #[serde(default)]
    includes: Option<ApiIncludes>,
    #[serde(default)]
    meta: Option<ApiTimelineMeta>,
}

#[derive(Debug, Deserialize)]
struct ApiTimelineMeta {
    #[serde(default)]
    next_token: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiUser {
    id: String,
    username: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    profile_image_url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    entities: Option<ApiUserEntities>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiUserEntities {
    #[serde(default)]
    url: Option<ApiUserUrlEntity>,
    #[serde(default)]
    description: Option<ApiEntities>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiUserUrlEntity {
    #[serde(default)]
    urls: Vec<ApiUrlEntity>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiTweet {
    id: String,
    text: String,
    #[serde(default)]
    author_id: Option<String>,
    #[serde(default)]
    referenced_tweets: Option<Vec<ApiReferencedTweet>>,
    #[serde(default)]
    attachments: Option<ApiAttachments>,
    created_at: String,
    #[serde(default)]
    entities: Option<ApiEntities>,
    #[serde(default)]
    note_tweet: Option<ApiNoteTweet>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiNoteTweet {
    text: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiReferencedTweet {
    id: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiAttachments {
    #[serde(default)]
    media_keys: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiEntities {
    #[serde(default)]
    urls: Vec<ApiUrlEntity>,
    #[serde(default)]
    mentions: Vec<ApiMention>,
    #[serde(default)]
    hashtags: Vec<ApiHashtag>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiUrlEntity {
    url: String,
    #[serde(default)]
    expanded_url: Option<String>,
    #[serde(default)]
    display_url: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiMention {
    username: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiHashtag {
    tag: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiIncludes {
    #[serde(default)]
    media: Vec<ApiMedia>,
    #[serde(default)]
    users: Vec<ApiUser>,
    #[serde(default)]
    tweets: Vec<ApiTweet>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiMedia {
    media_key: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    preview_image_url: Option<String>,
    #[serde(default)]
    alt_text: Option<String>,
    #[serde(default)]
    variants: Vec<ApiMediaVariant>,
}

#[derive(Debug, Deserialize, Clone)]
struct ApiMediaVariant {
    #[serde(default)]
    bit_rate: Option<u64>,
    content_type: String,
    url: String,
}

fn convert_user(user: &ApiUser) -> Result<User> {
    let id = UserId::parse(&user.id)?;
    let username = Username::parse(&user.username)?;
    let profile_image_url = match &user.profile_image_url {
        Some(url) => Some(nostrweet_core::HttpUrl::parse(url)?),
        None => None,
    };
    let url = match &user.url {
        Some(url) => Some(nostrweet_core::HttpUrl::parse(url)?),
        None => None,
    };
    let entities = match &user.entities {
        Some(entities) => Some(convert_user_entities(entities)?),
        None => None,
    };

    Ok(User {
        id,
        name: user.name.clone(),
        username,
        profile_image_url,
        description: user.description.clone(),
        url,
        entities,
    })
}

fn convert_user_entities(entities: &ApiUserEntities) -> Result<UserEntities> {
    let url = match &entities.url {
        Some(urls) => Some(UserUrlEntity {
            urls: urls
                .urls
                .iter()
                .map(convert_url_entity)
                .collect::<Result<Vec<_>>>()?,
        }),
        None => None,
    };
    let description = match &entities.description {
        Some(description) => Some(convert_entities(description)?),
        None => None,
    };
    Ok(UserEntities { url, description })
}

fn convert_entities(entities: &ApiEntities) -> Result<Entities> {
    let urls = entities
        .urls
        .iter()
        .map(convert_url_entity)
        .collect::<Result<Vec<_>>>()?;
    let mentions = entities
        .mentions
        .iter()
        .map(|mention| {
            Ok(Mention {
                username: Username::parse(&mention.username)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let hashtags = entities
        .hashtags
        .iter()
        .map(|tag| {
            Ok(Hashtag {
                tag: tag.tag.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Entities {
        urls,
        mentions,
        hashtags,
    })
}

fn convert_url_entity(entity: &ApiUrlEntity) -> Result<UrlEntity> {
    Ok(UrlEntity {
        url: nostrweet_core::HttpUrl::parse(&entity.url)?,
        expanded_url: match &entity.expanded_url {
            Some(url) => Some(nostrweet_core::HttpUrl::parse(url)?),
            None => None,
        },
        display_url: entity.display_url.clone(),
    })
}

fn convert_media(media: &ApiMedia) -> Result<Media> {
    let kind = match media.kind.as_str() {
        "photo" => MediaKind::Photo,
        "video" => MediaKind::Video,
        "animated_gif" => MediaKind::AnimatedGif,
        other => MediaKind::Other(other.to_string()),
    };
    let variants = media
        .variants
        .iter()
        .map(|variant| {
            Ok(MediaVariant {
                bit_rate: variant.bit_rate,
                content_type: variant.content_type.clone(),
                url: nostrweet_core::HttpUrl::parse(&variant.url)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Media {
        media_key: nostrweet_core::MediaKey::parse(&media.media_key)?,
        kind,
        url: match &media.url {
            Some(url) => Some(nostrweet_core::HttpUrl::parse(url)?),
            None => None,
        },
        preview_image_url: match &media.preview_image_url {
            Some(url) => Some(nostrweet_core::HttpUrl::parse(url)?),
            None => None,
        },
        alt_text: media.alt_text.clone(),
        variants,
    })
}

fn convert_includes(includes: &ApiIncludes) -> Result<Includes> {
    let media = includes
        .media
        .iter()
        .map(convert_media)
        .collect::<Result<Vec<_>>>()?;
    let users = includes
        .users
        .iter()
        .map(convert_user)
        .collect::<Result<Vec<_>>>()?;
    let user_map: HashMap<String, User> = users
        .iter()
        .cloned()
        .map(|user| (user.id.as_str().to_string(), user))
        .collect();
    let tweets = includes
        .tweets
        .iter()
        .map(|tweet| convert_tweet(tweet, &user_map, None, None))
        .collect::<Result<Vec<_>>>()?;

    Ok(Includes {
        media,
        users,
        tweets,
    })
}

fn convert_tweet(
    tweet: &ApiTweet,
    user_map: &HashMap<String, User>,
    includes: Option<&ApiIncludes>,
    fallback_user: Option<&User>,
) -> Result<Tweet> {
    let author_id = tweet
        .author_id
        .as_deref()
        .context("Tweet missing author_id")?;
    let author = if let Some(user) = user_map.get(author_id) {
        user.clone()
    } else if let Some(fallback) = fallback_user {
        fallback.clone()
    } else {
        User::new(UserId::parse(author_id)?, Username::parse("unknown")?)
    };

    let id = TweetId::parse(&tweet.id)?;
    let text = TweetText::parse(&tweet.text)?;
    let created_at = CreatedAt::parse(&tweet.created_at)?;

    let mut converted = Tweet::new(id.clone(), text, author, created_at);
    converted.author_id = Some(UserId::parse(author_id)?);
    converted.entities = match &tweet.entities {
        Some(entities) => Some(convert_entities(entities)?),
        None => None,
    };
    converted.note_tweet = match &tweet.note_tweet {
        Some(note) => Some(NoteTweet {
            text: TweetText::parse(&note.text)?,
        }),
        None => None,
    };
    converted.attachments = tweet.attachments.as_ref().map(|attachments| Attachments {
        media_keys: attachments
            .media_keys
            .iter()
            .filter_map(|key| nostrweet_core::MediaKey::parse(key).ok())
            .collect(),
    });
    converted.referenced_tweets = tweet
        .referenced_tweets
        .as_ref()
        .map(|refs| {
            refs.iter()
                .map(|reference| {
                    let kind = match reference.kind.as_str() {
                        "replied_to" => ReferenceKind::RepliedTo,
                        "retweeted" => ReferenceKind::Retweeted,
                        "quoted" => ReferenceKind::Quoted,
                        other => ReferenceKind::Unknown(other.to_string()),
                    };
                    Ok(ReferencedTweet {
                        id: TweetId::parse(&reference.id)?,
                        kind,
                        data: None,
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();

    if let Some(includes) = includes {
        converted.includes = Some(convert_includes(includes)?);
        if let Some(includes) = converted.includes.as_ref() {
            for reference in &mut converted.referenced_tweets {
                if let Some(found) = includes
                    .tweets
                    .iter()
                    .find(|candidate| candidate.id == reference.id)
                {
                    reference.data = Some(Box::new(found.clone()));
                }
            }
        }
    }

    Ok(converted)
}

impl TwitterPort for TwitterClient {
    #[instrument(name = "twitter.fetch_tweet", skip(self))]
    async fn fetch_tweet(&self, id: &TweetId) -> Result<Tweet> {
        let response = self.fetch_tweet_by_id(id).await?;
        let includes = response.includes.as_ref();
        let user_map = includes
            .map(|inc| {
                inc.users
                    .iter()
                    .map(convert_user)
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|user| (user.id.as_str().to_string(), user))
            .collect::<HashMap<_, _>>();
        convert_tweet(&response.data, &user_map, includes, None)
    }

    #[instrument(name = "twitter.fetch_user_profile", skip(self))]
    async fn fetch_user_profile(&self, username: &Username) -> Result<User> {
        self.get_user_by_username(username).await
    }

    #[instrument(name = "twitter.fetch_user_tweets", skip(self))]
    async fn fetch_user_tweets(
        &self,
        username: &Username,
        query: UserTweetsQuery,
    ) -> Result<Vec<Tweet>> {
        let user = self.get_user_by_username(username).await?;
        let user_id = user.id.clone();
        let mut tweets = Vec::new();
        let mut next_token = None;

        loop {
            let response = self
                .fetch_timeline_page(
                    &user_id,
                    query.count as usize,
                    next_token.as_deref(),
                    query.since_id.as_ref(),
                )
                .await?;
            let includes = response.includes.as_ref();
            let user_map = includes
                .map(|inc| {
                    inc.users
                        .iter()
                        .map(convert_user)
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .map(|user| (user.id.as_str().to_string(), user))
                .collect::<HashMap<_, _>>();

            let page = response.data.unwrap_or_default();
            for api_tweet in page {
                let converted = convert_tweet(&api_tweet, &user_map, includes, Some(&user))?;
                tweets.push(converted);
                if tweets.len() >= query.count as usize {
                    break;
                }
            }

            if tweets.len() >= query.count as usize {
                break;
            }

            next_token = response.meta.and_then(|meta| meta.next_token);
            if next_token.is_none() {
                break;
            }
        }

        if let Some(days) = query.days {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            let cutoff = now.as_secs().saturating_sub(u64::from(days) * 86_400);
            let mut filtered = Vec::new();
            for tweet in tweets {
                let timestamp = tweet.created_at.unix_timestamp()?;
                if timestamp.value() >= cutoff {
                    filtered.push(tweet);
                }
            }
            tweets = filtered;
        }

        Ok(tweets)
    }

    #[instrument(name = "twitter.enrich_referenced_tweets", skip(self, tweet))]
    async fn enrich_referenced_tweets(&self, tweet: &mut Tweet) -> Result<()> {
        for reference in &mut tweet.referenced_tweets {
            if reference.data.is_some() {
                continue;
            }
            let fetched = self.fetch_tweet(&reference.id).await?;
            reference.data = Some(Box::new(fetched));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostrweet_core::{CreatedAt, ReferenceKind, ReferencedTweet, TweetText, UserId, Username};

    #[derive(Clone)]
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
    async fn fetch_user_profile_returns_user() -> Result<()> {
        let mut client = InMemoryTwitter::new();
        let user = sample_user("tester", "1")?;
        client.insert_user(user.clone());

        let fetched = client
            .fetch_user_profile(&Username::parse("tester")?)
            .await?;
        assert_eq!(fetched, user);
        Ok(())
    }

    #[tokio::test]
    async fn fetch_tweet_returns_error_when_missing() -> Result<()> {
        let client = InMemoryTwitter::new();
        let err = client
            .fetch_tweet(&TweetId::parse("123")?)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Tweet not found"));
        Ok(())
    }

    #[tokio::test]
    async fn fetch_user_tweets_applies_days_and_count() -> Result<()> {
        let clock = FixedClock::new(172_800);
        let mut client = InMemoryTwitter::with_clock(clock);

        let recent = sample_tweet("200", "tester", "1970-01-02T00:00:00Z")?;
        let old = sample_tweet("100", "tester", "1970-01-01T00:00:00Z")?;
        client.insert_tweet(recent.clone());
        client.insert_tweet(old);

        let query = UserTweetsQuery {
            count: 1,
            days: Some(1),
            since_id: None,
        };

        let tweets = client
            .fetch_user_tweets(&Username::parse("tester")?, query)
            .await?;
        assert_eq!(tweets.len(), 1);
        assert_eq!(tweets[0].id.as_str(), recent.id.as_str());
        Ok(())
    }

    #[tokio::test]
    async fn fetch_user_tweets_applies_since_id() -> Result<()> {
        let mut client = InMemoryTwitter::new();
        client.insert_tweet(sample_tweet("100", "tester", "1970-01-01T00:00:00Z")?);
        client.insert_tweet(sample_tweet("200", "tester", "1970-01-01T00:00:00Z")?);

        let query = UserTweetsQuery {
            count: 10,
            days: None,
            since_id: Some(TweetId::parse("150")?),
        };

        let tweets = client
            .fetch_user_tweets(&Username::parse("tester")?, query)
            .await?;

        assert_eq!(tweets.len(), 1);
        assert_eq!(tweets[0].id.as_str(), "200");
        Ok(())
    }

    #[tokio::test]
    async fn enrich_referenced_tweets_fills_data() -> Result<()> {
        let mut client = InMemoryTwitter::new();
        let referenced = sample_tweet("222", "ref", "1970-01-01T00:00:00Z")?;
        client.insert_tweet(referenced.clone());

        let mut tweet = sample_tweet("111", "tester", "1970-01-01T00:00:00Z")?;
        tweet.referenced_tweets = vec![ReferencedTweet {
            id: TweetId::parse("222")?,
            kind: ReferenceKind::Quoted,
            data: None,
        }];

        client.enrich_referenced_tweets(&mut tweet).await?;
        assert!(tweet.referenced_tweets[0].data.is_some());
        assert_eq!(
            tweet.referenced_tweets[0]
                .data
                .as_ref()
                .unwrap()
                .id
                .as_str(),
            referenced.id.as_str()
        );
        Ok(())
    }
}
