use crate::error_utils::{
    create_http_client_with_context, get_required_env_var, parse_http_response_json,
};
use anyhow::{bail, Context, Result};
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use regex::Regex;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::{fs, future::Future, path::Path, pin::Pin, time::Duration};
use thiserror::Error;
use tracing::{debug, info};
use url::Url;

/// Twitter API specific errors with structured information
#[derive(Debug, Error)]
pub enum TwitterError {
    #[error("Rate limit exceeded (reset at {reset_time:?}, remaining: {remaining:?})")]
    RateLimit {
        reset_time: Option<u64>,
        remaining: Option<u64>,
    },

    #[error("User not found: {username}")]
    UserNotFound { username: String },

    #[error("Tweet not found: {tweet_id}")]
    TweetNotFound { tweet_id: String },

    #[error("Network error: {message}")]
    #[allow(dead_code)] // Placeholder for future network error handling
    Network { message: String },

    #[error("API error (status {status}): {message}")]
    ApiError { status: u16, message: String },

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

const TWITTER_API_BASE: &str = "https://api.twitter.com/2";

// Common URL parameters for API requests
const COMMON_MEDIA_FIELDS: &str = "url,preview_image_url,alt_text,variants,media_key,type";
const COMMON_TWEET_FIELDS: &str = "created_at,entities,referenced_tweets,author_id,note_tweet";
const COMMON_USER_FIELDS: &str = "name,username,profile_image_url,description,url,entities";
const COMMON_EXPANSIONS: &str = "attachments.media_keys,referenced_tweets.id,referenced_tweets.id.attachments.media_keys,author_id";

/// Twitter API rate limit information extracted from response headers
#[derive(Debug, Clone, Default)]
struct RateLimits {
    /// Maximum number of requests allowed in the current time window
    limit: Option<u64>,
    /// Number of requests remaining in the current time window
    remaining: Option<u64>,
    /// Unix timestamp when the rate limit resets
    reset: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Tweet {
    /// The tweet ID
    pub id: String,

    /// Tweet content text
    pub text: String,

    /// User who posted the tweet
    #[serde(default)]
    pub author: User,

    /// Original tweet if this is a retweet
    pub referenced_tweets: Option<Vec<ReferencedTweet>>,

    /// Media attachments (photos, videos)
    pub attachments: Option<Attachments>,

    /// Tweet creation date
    pub created_at: String,

    /// Additional metadata
    pub entities: Option<Entities>,

    /// Media metadata (populated during API expansion)
    pub includes: Option<Includes>,

    /// Author ID from API
    pub author_id: Option<String>,

    /// For tweets that exceed the standard character limit (X Premium long tweets).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_tweet: Option<NoteTweet>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NoteTweet {
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct User {
    pub id: String,
    pub name: Option<String>,
    pub username: String,
    pub profile_image_url: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub entities: Option<UserEntities>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserEntities {
    pub url: Option<UserUrlEntity>,
    pub description: Option<Entities>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserUrlEntity {
    pub urls: Option<Vec<UrlEntity>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReferencedTweet {
    pub id: String,
    #[serde(rename = "type")]
    pub type_field: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<Tweet>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Attachments {
    pub media_keys: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Entities {
    pub urls: Option<Vec<UrlEntity>>,
    pub mentions: Option<Vec<Mention>>,
    pub hashtags: Option<Vec<Hashtag>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UrlEntity {
    pub url: String,
    pub expanded_url: String,
    pub display_url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Mention {
    pub username: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Hashtag {
    pub tag: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Includes {
    pub media: Option<Vec<Media>>,
    pub users: Option<Vec<User>>,
    pub tweets: Option<Vec<Tweet>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Media {
    pub media_key: String,
    #[serde(rename = "type")]
    pub type_field: String,
    pub url: Option<String>,
    pub preview_image_url: Option<String>,
    pub alt_text: Option<String>,
    pub variants: Option<Vec<MediaVariant>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MediaVariant {
    pub bit_rate: Option<u64>,
    pub content_type: String,
    pub url: String,
}

/// Twitter API client for downloading tweet data
pub struct TwitterClient {
    client: Client,
    bearer_token: String,
    /// Cache directory for tweets
    cache_dir: Option<std::path::PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimelineResponse {
    pub data: Option<Vec<Tweet>>,
    pub meta: Option<TimelineMeta>,
    pub includes: Option<Includes>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimelineMeta {
    pub result_count: u32,
    pub newest_id: Option<String>,
    pub oldest_id: Option<String>,
    pub next_token: Option<String>,
    pub previous_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserProfileResponse {
    pub data: User,
}

impl TwitterClient {
    /// Parses rate limit headers from a response
    fn parse_rate_limit_headers(&self, response: &reqwest::Response) -> RateLimits {
        let remaining = response
            .headers()
            .get("x-rate-limit-remaining")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let limit = response
            .headers()
            .get("x-rate-limit-limit")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let reset = response
            .headers()
            .get("x-rate-limit-reset")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        RateLimits {
            limit,
            remaining,
            reset,
        }
    }
    /// Calculates a sleep duration with random jitter to avoid thundering herd effects
    fn calculate_sleep_duration_with_jitter(&self, base_duration: Duration) -> Duration {
        // Add 0-999ms of jitter to the base duration
        let jitter = rand::random::<u64>() % 1000;
        base_duration + Duration::from_millis(jitter)
    }

    /// Creates an exponential backoff configuration for API request retries
    fn create_backoff_config(&self) -> impl Backoff {
        ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_secs(1))
            .with_max_interval(Duration::from_secs(60))
            .with_multiplier(2.0)
            .with_max_elapsed_time(Some(Duration::from_secs(300))) // 5 minutes max
            .build()
    }

    /// Makes a Twitter API request with automatic retries for rate limiting
    async fn api_request(&self, resource_id: &str, url: &str) -> Result<reqwest::Response> {
        // Configure exponential backoff
        let mut backoff = self.create_backoff_config();

        let mut attempt = 0;
        let max_attempts = 5;

        loop {
            // Make the request
            debug!(%resource_id, %url, "Making request to Twitter API");

            // Use a match pattern to handle different types of errors
            let response = match self
                .client
                .get(url)
                .bearer_auth(&self.bearer_token)
                .timeout(Duration::from_secs(30)) // Add explicit timeout
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    attempt += 1;

                    // Check if we've hit the max retry attempts
                    if attempt >= max_attempts {
                        return Err(anyhow::Error::new(err)).with_context(|| {
                            format!(
                                "Failed to send request to Twitter API after {attempt} attempts"
                            )
                        });
                    }

                    // Check if it's a timeout or connection error
                    let is_timeout = err.is_timeout()
                        || err.is_connect()
                        || err.to_string().contains("timed out");

                    if is_timeout {
                        // Use exponential backoff for timeouts
                        let backoff_time = backoff
                            .next_backoff()
                            .unwrap_or(Duration::from_secs(5 * (attempt as u64)));

                        let sleep_duration =
                            self.calculate_sleep_duration_with_jitter(backoff_time);

                        debug!("Network timeout connecting to Twitter API for {resource_id}. Retrying in {sleep_duration:?} (attempt {attempt}/{max_attempts})");
                        tokio::time::sleep(sleep_duration).await;
                        continue;
                    } else {
                        // For other types of errors, propagate them
                        return Err(anyhow::Error::new(err)
                            .context("Failed to send request to Twitter API"));
                    }
                }
            };

            // Parse rate limit headers
            let rate_limits = self.parse_rate_limit_headers(&response);

            // Check for rate limiting
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                attempt += 1;

                if attempt >= max_attempts {
                    debug!("Maximum retry attempts ({max_attempts}) reached for {resource_id}, rate limit reset: {rate_limit_reset:?}", rate_limit_reset = rate_limits.reset);
                    return Err(TwitterError::RateLimit {
                        reset_time: rate_limits.reset,
                        remaining: rate_limits.remaining,
                    }
                    .into());
                }

                // First check Twitter-specific rate limit headers
                let now = std::time::SystemTime::now();
                let timestamp = now
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_else(|_| Duration::from_secs(0))
                    .as_secs();

                // Try to get rate limit reset time from headers
                let rate_limit_wait = rate_limits.reset.map(|reset_time| {
                    if reset_time > timestamp {
                        // Calculate seconds until reset
                        reset_time - timestamp
                    } else {
                        // If reset time is in the past, use default backoff
                        backoff
                            .next_backoff()
                            .unwrap_or(Duration::from_secs(5))
                            .as_secs()
                    }
                });

                // Try to get retry-after header as fallback
                let retry_after = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok());

                // Use the most specific timing information available
                let base_wait_secs = rate_limit_wait.or(retry_after).unwrap_or_else(|| {
                    // Default to exponential backoff if no header guidance
                    backoff
                        .next_backoff()
                        .unwrap_or(Duration::from_secs(5 * (attempt as u64)))
                        .as_secs()
                });

                let sleep_duration =
                    self.calculate_sleep_duration_with_jitter(Duration::from_secs(base_wait_secs));

                debug!("Rate limited by Twitter API for {resource_id}. Limit: {limit:?}, Remaining: {remaining:?}, Reset: {reset:?}. Retrying in {sleep_duration:?} (attempt {attempt}/{max_attempts})",
                      limit = rate_limits.limit,
                      remaining = rate_limits.remaining,
                      reset = rate_limits.reset);
                tokio::time::sleep(sleep_duration).await;
                continue;
            }

            // Check for other errors
            if response.status().is_success() {
                debug!("Received Twitter API response for {resource_id} with limits: {limit:?}/{remaining:?} until {reset:?}",
                       limit = rate_limits.limit,
                       remaining = rate_limits.remaining,
                       reset = rate_limits.reset);
            } else {
                let status_code = response.status().as_u16();
                let error_message = format!(
                    "limit: {limit:?}, remaining: {remaining:?}, reset: {reset:?}",
                    limit = rate_limits.limit,
                    remaining = rate_limits.remaining,
                    reset = rate_limits.reset
                );

                return Err(match response.status() {
                    StatusCode::NOT_FOUND => TwitterError::UserNotFound {
                        username: resource_id.to_string(),
                    },
                    StatusCode::TOO_MANY_REQUESTS => TwitterError::RateLimit {
                        reset_time: rate_limits.reset,
                        remaining: rate_limits.remaining,
                    },
                    _ => TwitterError::ApiError {
                        status: status_code,
                        message: error_message,
                    },
                }
                .into());
            }

            return Ok(response);
        }
    }

    /// Enriches a tweet by fetching data for referenced tweets, with cache checking
    pub async fn enrich_referenced_tweets(
        &self,
        tweet: &mut Tweet,
        cache_dir: Option<&std::path::Path>,
    ) -> Result<()> {
        // Check if we have referenced tweets that need enrichment
        if let Some(ref_tweets) = &mut tweet.referenced_tweets {
            for ref_tweet in ref_tweets {
                // Only fetch if we don't already have the data
                if ref_tweet.data.is_none() {
                    // First check cache if a directory is provided
                    let mut found_in_cache = false;

                    if let Some(output_dir) = cache_dir {
                        if let Some(ref_path) =
                            crate::storage::find_existing_tweet_json(&ref_tweet.id, output_dir)
                        {
                            debug!("Found cached referenced tweet {id}", id = ref_tweet.id);

                            // Try to load the referenced tweet from cache
                            match crate::storage::load_tweet_from_file(&ref_path) {
                                Ok(referenced_tweet) => {
                                    // Store the cached tweet in the data field
                                    ref_tweet.data = Some(Box::new(referenced_tweet));
                                    debug!(
                                        "Successfully loaded cached referenced tweet {id}",
                                        id = ref_tweet.id
                                    );
                                    found_in_cache = true;
                                }
                                Err(e) => {
                                    // Just log the error and continue to API fetch
                                    debug!(
                                        "Failed to load cached referenced tweet {id}: {error}",
                                        id = ref_tweet.id,
                                        error = e
                                    );
                                }
                            }
                        }
                    }

                    // If not found in cache, try API
                    if !found_in_cache {
                        debug!("Fetching data for referenced tweet {id}", id = ref_tweet.id);

                        // Try to fetch the referenced tweet
                        match self.get_tweet(&ref_tweet.id).await {
                            Ok(referenced_tweet) => {
                                // Verify author data is complete before storing
                                let username_present = !referenced_tweet.author.username.is_empty();
                                if !username_present {
                                    debug!(
                                        "Referenced tweet {id} has incomplete author data",
                                        id = ref_tweet.id
                                    );
                                    // Log author info for debugging
                                    debug!(
                                        "Author data: id={:?}, author={:?}",
                                        referenced_tweet.author_id, referenced_tweet.author
                                    );
                                }

                                // Store the fetched tweet in the data field
                                ref_tweet.data = Some(Box::new(referenced_tweet.clone()));
                                let author_status = if username_present {
                                    "complete"
                                } else {
                                    "incomplete"
                                };
                                debug!(
                                    "Successfully enriched referenced tweet {id} (author data: {author_status})",
                                    id = ref_tweet.id
                                );

                                // Save the referenced tweet to cache if cache directory is available
                                if let Some(output_dir) = cache_dir {
                                    match crate::storage::save_tweet(&referenced_tweet, output_dir)
                                    {
                                        Ok(path) => {
                                            debug!(
                                                "Saved referenced tweet {id} to cache: {path}",
                                                id = ref_tweet.id,
                                                path = path.display()
                                            );
                                        }
                                        Err(e) => {
                                            debug!(
                                                "Failed to save referenced tweet {id} to cache: {error}",
                                                id = ref_tweet.id,
                                                error = e
                                            );
                                        }
                                    }
                                }
                            }
                            Err(error) => {
                                // Just log the error and continue - don't fail the whole process
                                debug!(
                                    "Failed to fetch referenced tweet {id}: {error}",
                                    id = ref_tweet.id
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Creates a new Twitter client using the TWITTER_BEARER_TOKEN env variable
    /// and the provided cache directory.
    pub fn new(cache_dir: &Path) -> Result<Self> {
        let bearer_token = get_required_env_var("TWITTER_BEARER_TOKEN")?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        // The calling code is responsible for creating the directory, but we double-check
        if !cache_dir.exists() {
            fs::create_dir_all(cache_dir).with_context(|| {
                format!(
                    "Failed to create cache directory at {path}",
                    path = cache_dir.display()
                )
            })?;
        }

        debug!("Tweet caching enabled: {path}", path = cache_dir.display());

        Ok(Self {
            client,
            bearer_token,
            cache_dir: Some(cache_dir.to_path_buf()),
        })
    }

    /// Retrieves a tweet by its ID, including all media and referenced tweets
    pub async fn get_tweet(&self, tweet_id: &str) -> Result<Tweet> {
        // Use a new function that returns a pinned box to handle recursive async calls
        self.get_tweet_boxed(tweet_id).await
    }

    // This function returns a boxed future to handle recursive async calls properly
    fn get_tweet_boxed<'a>(
        &'a self,
        tweet_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Tweet>> + 'a>> {
        Box::pin(async move { self.get_tweet_internal(tweet_id).await })
    }

    /// Internal implementation of tweet fetching logic
    async fn get_tweet_internal(&self, tweet_id: &str) -> Result<Tweet> {
        // First check if we can find the tweet in any known directory
        if let Some(existing_path) = crate::storage::find_tweet_in_all_directories(tweet_id) {
            debug!(
                "Found tweet {tweet_id} in directory {path}, loading from disk",
                path = existing_path.parent().unwrap_or(Path::new("./")).display()
            );
            return crate::storage::load_tweet_from_file(&existing_path);
        }

        // Double-check in cache directory if it exists (for backward compatibility)
        if let Some(ref cache_path) = self.cache_dir {
            if let Some(existing_path) =
                crate::storage::find_existing_tweet_json(tweet_id, cache_path)
            {
                debug!("Found tweet {tweet_id} in cache, loading from disk");
                return crate::storage::load_tweet_from_file(&existing_path);
            }
        }

        // Check if tweet is marked as "not found" in the primary cache_dir
        if let Some(ref cache_path) = self.cache_dir {
            if cache_path.exists() {
                // Only check if cache_dir itself exists
                if crate::storage::is_tweet_not_found(tweet_id, cache_path) {
                    debug!(
                        "Tweet {tweet_id} is cached as not found in {path}, skipping API call",
                        path = cache_path.display()
                    );
                    return Err(TwitterError::TweetNotFound {
                        tweet_id: tweet_id.to_string(),
                    }
                    .into());
                }
            }
        }

        // If not in cache (neither as JSON nor as .not_found), fetch from API
        debug!("Tweet {tweet_id} not found in cache or as .not_found, fetching from API");
        let url = self.build_tweet_url(tweet_id);

        let response = self.api_request(tweet_id, &url).await?;

        let data: serde_json::Value = parse_http_response_json(response, "Twitter API").await?;

        debug!("Received Twitter API response for tweet {tweet_id}");

        // Debug: log the full JSON response for tweet
        debug!(
            "Tweet API response JSON for {tweet_id}: {json}",
            json = data
        );

        // Check if we got an error response
        if data.get("errors").is_some() && data.get("data").is_none() {
            // Extract error details for better diagnostics
            let error_detail = data["errors"][0]["detail"]
                .as_str()
                .unwrap_or("Unknown error");

            // Check if this is a 'resource_not_found' error and cache it if so
            let error_type = data["errors"][0]["type"].as_str();
            let error_title = data["errors"][0]["title"].as_str();

            if error_type == Some("https://api.twitter.com/2/problems/resource-not-found")
                || error_title == Some("Not Found Error")
            {
                debug!(
                    "Tweet {tweet_id} reported as 'not found' by API: type='{error_type:?}', title='{error_title:?}', detail='{error_detail}'"
                );
                if let Some(ref cache_path) = self.cache_dir {
                    // Ensure cache directory exists before trying to write the .not_found file
                    if !cache_path.exists() {
                        if let Err(e) = std::fs::create_dir_all(cache_path) {
                            debug!(
                                "Failed to create cache directory {path} for .not_found marker: {e}",
                                path = cache_path.display()
                            );
                            // If dir creation fails, we can't mark it, so just proceed to bail
                        }
                    }
                    // Re-check existence in case create_dir_all failed silently or due to permissions
                    if cache_path.exists() {
                        if let Err(e) =
                            crate::storage::mark_tweet_as_not_found(tweet_id, cache_path)
                        {
                            debug!("Failed to mark tweet {tweet_id} as not found in cache: {e}");
                        } else {
                            debug!(
                                "Marked tweet {tweet_id} as not found in cache {path}",
                                path = cache_path.display()
                            );
                        }
                    }
                }
            }

            // Check if this is a "not found" error
            if error_type == Some("https://api.twitter.com/2/problems/resource-not-found")
                || error_title == Some("Not Found Error")
            {
                return Err(TwitterError::TweetNotFound {
                    tweet_id: tweet_id.to_string(),
                }
                .into());
            } else {
                return Err(TwitterError::ApiError {
                    status: 400, // Default status for API errors
                    message: format!("Twitter API error: {error_detail}"),
                }
                .into());
            }
        }

        // Extract the tweet data
        let raw_data = data
            .get("data")
            .with_context(|| {
                format!("No 'data' field in Twitter API response for tweet {tweet_id}")
            })?
            .clone();

        debug!("Raw 'data' JSON for tweet {tweet_id}: {raw_data}");
        let mut tweet: Tweet = serde_json::from_value(raw_data.clone())
            .with_context(|| format!("Failed to parse tweet data for {tweet_id}: {raw_data}"))?;

        // Extract includes data (media, users, referenced tweets)
        if let Some(includes) = data.get("includes") {
            tweet.includes = serde_json::from_value(includes.clone()).ok();
        }

        // Fetch all referenced tweets, not just retweets
        if let Some(ref mut referenced_tweets) = tweet.referenced_tweets {
            for ref_tweet in referenced_tweets {
                // Fetch data for all reference types (retweet, quoted, replied_to)
                info!(
                    "Tweet references another tweet of type '{type_field}', fetching tweet {id}",
                    type_field = ref_tweet.type_field,
                    id = ref_tweet.id
                );

                // Use our boxed function to handle recursive calls properly
                match self.get_tweet_boxed(&ref_tweet.id).await {
                    Ok(referenced_tweet) => {
                        ref_tweet.data = Some(Box::new(referenced_tweet));
                        debug!(
                            "Successfully fetched referenced tweet {id}",
                            id = ref_tweet.id
                        );

                        // Save referenced tweet to cache if cache directory is available
                        if let Some(ref cache_path) = self.cache_dir {
                            if let Some(ref tweet_data) = ref_tweet.data {
                                if let Err(e) = crate::storage::save_tweet(tweet_data, cache_path) {
                                    debug!(
                                        "Failed to save referenced tweet {id} to cache: {e}",
                                        id = ref_tweet.id
                                    );
                                } else {
                                    debug!(
                                        "Saved referenced tweet {id} to cache",
                                        id = ref_tweet.id
                                    );
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!(
                            "Failed to fetch referenced tweet {id}: {e}",
                            id = ref_tweet.id
                        );
                        // Continue with the basic tweet even if fetching references fails
                    }
                }
            }
        }

        // Extract author data
        if let Some(includes) = &tweet.includes {
            if let Some(users) = &includes.users {
                if let Some(author_id) = &tweet.author_id {
                    if let Some(user) = users.iter().find(|u| u.id == *author_id) {
                        tweet.author = user.clone();
                    } else if let Some(first) = users.first() {
                        // Fallback to the first user if the author was not found
                        tweet.author = first.clone();
                    }
                } else if let Some(first) = users.first() {
                    tweet.author = first.clone();
                }
            }
        }

        // Save tweet to cache if cache directory is available
        if let Some(ref cache_path) = self.cache_dir {
            // Ensure cache directory exists
            if !cache_path.exists() {
                if let Err(e) = std::fs::create_dir_all(cache_path) {
                    debug!("Failed to create cache directory: {e}");
                }
            }

            // Save tweet to cache
            if let Err(e) = crate::storage::save_tweet(&tweet, cache_path) {
                debug!("Failed to save tweet to cache: {e}");
            } else {
                debug!("Saved tweet {tweet_id} to cache");
            }
        }

        Ok(tweet)
    }

    /// Fetches the most recent tweets from a user's timeline
    ///
    /// # Arguments
    /// * `username` - Twitter username (without the @ symbol)
    /// * `max_results` - Maximum number of tweets to fetch (default: 10)
    pub async fn get_user_timeline(
        &self,
        username: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<Tweet>> {
        self.get_user_timeline_with_since_id(username, max_results, None)
            .await
    }

    /// Get a user's timeline from Twitter with optional since_id for efficient pagination
    /// Returns tweets newer than the specified since_id
    ///
    /// # Arguments
    /// * `username` - Twitter username (without the @ symbol)
    /// * `max_results` - Maximum number of tweets to fetch (default: 10)
    /// * `since_id` - Only return tweets with IDs greater than this value
    pub async fn get_user_timeline_with_since_id(
        &self,
        username: &str,
        max_results: Option<u32>,
        since_id: Option<String>,
    ) -> Result<Vec<Tweet>> {
        // First get the user ID from the username
        let user_id = self.get_user_id(username).await?;

        // Then get the user's tweets
        let requested_count = max_results.unwrap_or(10);
        // Only enforce min limit of 5, but don't cap the max since we'll paginate
        let requested_count = requested_count.max(5);

        // Twitter API has a max of 100 tweets per request, so we'll need to paginate
        // for larger requests
        let page_size = 100; // Twitter API max per request

        // Create a vector to hold cached tweets
        let mut cached_tweets = Vec::new();

        // Try to load cached tweets for this user
        if let Some(ref cache_path) = self.cache_dir {
            if cache_path.exists() {
                self.load_cached_tweets_for_user(username, cache_path, &mut cached_tweets);
            }
        }

        // If we found enough cached tweets, we can skip the API call
        // If no specific count is requested and we have enough cached tweets, we can skip the API call
        if max_results.is_none() && cached_tweets.len() >= requested_count as usize {
            // No need to fetch from the API since we have enough tweets in cache
            debug!(
                "Found {cached_count} cached tweets for user {username}, skipping API call",
                cached_count = cached_tweets.len()
            );

            // Sort tweets by creation date (newest first)
            cached_tweets.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            // Return only the requested number
            return Ok(cached_tweets
                .into_iter()
                .take(requested_count as usize)
                .collect());
        } else if !cached_tweets.is_empty() {
            debug!("Found {cached_count} cached tweets for user {username}, but need {requested_count}",
                cached_count = cached_tweets.len());
        }

        // If we get here, we need to fetch from the API

        // Fetch remaining tweets from the API
        let timeline_response = self
            .fetch_paginated_timeline(&user_id, requested_count, page_size, since_id.as_deref())
            .await?;

        let mut tweets = timeline_response.data.unwrap_or_default();

        if tweets.is_empty() && cached_tweets.is_empty() {
            info!("No tweets found for user {username}");
            return Ok(Vec::new());
        }

        // Process includes data to populate tweet metadata
        if let Some(includes) = timeline_response.includes {
            // Add author information to each tweet
            if let Some(users) = &includes.users {
                use std::collections::HashMap;
                if !users.is_empty() {
                    // Build a lookup map id -> user for fast access
                    let user_map: HashMap<String, crate::twitter::User> =
                        users.iter().cloned().map(|u| (u.id.clone(), u)).collect();
                    for tweet in &mut tweets {
                        if let Some(author_id) = &tweet.author_id {
                            if let Some(user) = user_map.get(author_id) {
                                tweet.author = user.clone();
                            }
                        }
                    }
                }
            }

            // Add media information to each tweet
            if let Some(ref media) = includes.media {
                for tweet in &mut tweets {
                    self.add_media_to_tweet(tweet, media);
                }
            }
        }

        // Save all newly fetched tweets to cache
        if let Some(ref cache_path) = self.cache_dir {
            // Ensure cache directory exists
            if !cache_path.exists() {
                if let Err(e) = std::fs::create_dir_all(cache_path) {
                    debug!("Failed to create cache directory: {e}");
                }
            }

            // Save each tweet to cache
            for tweet in &tweets {
                if let Err(e) = crate::storage::save_tweet(tweet, cache_path) {
                    debug!("Failed to save tweet to cache: {e}");
                } else {
                    debug!("Saved tweet {tweet_id} to cache", tweet_id = tweet.id);
                }
            }
        }

        // Combine newly fetched tweets with any cached tweets we found
        if !cached_tweets.is_empty() {
            self.combine_tweets(&mut tweets, cached_tweets, requested_count);
        }

        // Log the number of tweets we're returning
        let tweet_count = tweets.len();
        match tweet_count.cmp(&(requested_count as usize)) {
            std::cmp::Ordering::Less => {
                debug!("Returning {tweet_count} tweets for user {username} (requested {requested_count})");
            }
            std::cmp::Ordering::Equal => {
                debug!(
                    "Returning exactly {requested_count} tweets for user {username} as requested"
                );
            }
            std::cmp::Ordering::Greater => {
                debug!("Fetched more tweets than requested ({tweet_count} vs {requested_count})");
                // We don't truncate the results - return all tweets we found
            }
        }
        Ok(tweets)
    }

    /// Combines newly fetched tweets with cached tweets, removing duplicates
    /// and sorting by creation date (newest first)
    fn combine_tweets(
        &self,
        tweets: &mut Vec<Tweet>,
        cached_tweets: Vec<Tweet>,
        requested_count: u32,
    ) {
        // Create a hashset of fetched tweet IDs
        let fetched_ids: std::collections::HashSet<String> =
            tweets.iter().map(|t| t.id.clone()).collect();

        // Add non-duplicate cached tweets
        for tweet in cached_tweets {
            if !fetched_ids.contains(&tweet.id) {
                tweets.push(tweet);
            }
        }

        // Sort by creation date (newest first)
        tweets.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // Limit to requested count
        if tweets.len() > requested_count as usize {
            tweets.truncate(requested_count as usize);
        }
    }

    /// Adds media items to a tweet if they match the tweet's media keys
    fn add_media_to_tweet(&self, tweet: &mut Tweet, media_items: &[Media]) {
        // Create includes for this tweet if it doesn't exist
        if tweet.includes.is_none() {
            tweet.includes = Some(Includes {
                media: None,
                users: None,
                tweets: None,
            });
        }

        // Only add media that belongs to this tweet
        if let Some(attachments) = &tweet.attachments {
            if let Some(media_keys) = &attachments.media_keys {
                // Create a Vector of Media items that match the media_keys
                let mut tweet_media = Vec::new();

                for media in media_items {
                    if media_keys.contains(&media.media_key) {
                        tweet_media.push(media.clone());
                    }
                }

                if !tweet_media.is_empty() {
                    if let Some(includes) = &mut tweet.includes {
                        includes.media = Some(tweet_media);
                    }
                }
            }
        }
    }

    /// Load cached tweets for a specific user from the cache directory
    fn load_cached_tweets_for_user(
        &self,
        username: &str,
        cache_path: &std::path::Path,
        cached_tweets: &mut Vec<Tweet>,
    ) {
        // Check if user's tweets might be cached - this is a best-effort approach
        if let Ok(entries) = std::fs::read_dir(cache_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(filename) = path.file_name() {
                    let filename = filename.to_string_lossy();
                    // Look for tweets from this user by checking filename
                    if filename.contains(&username.to_lowercase()) && path.is_file() {
                        if let Ok(tweet) = crate::storage::load_tweet_from_file(&path) {
                            if tweet.author.username.to_lowercase() == username.to_lowercase() {
                                cached_tweets.push(tweet);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Fetches tweets from a user's timeline with pagination support
    ///
    /// Returns a TimelineResponse with all tweets fetched across multiple pages
    async fn fetch_paginated_timeline(
        &self,
        user_id: &str,
        requested_count: u32,
        page_size: u32,
        since_id: Option<&str>,
    ) -> Result<TimelineResponse> {
        let mut all_tweets = Vec::new();
        let mut next_token: Option<String> = None;
        let mut remaining_count = requested_count;
        let mut last_response = TimelineResponse {
            data: None,
            meta: None,
            includes: None,
        };

        // Continue fetching pages until we have enough tweets or there are no more pages
        while remaining_count > 0 {
            // For each request, fetch the minimum of the page_size and remaining count
            // Twitter API requires at least 10 tweets to be requested
            let min_api_request = 10;
            let current_page_size =
                std::cmp::max(min_api_request, std::cmp::min(page_size, remaining_count));

            // Build the URL for user timeline request
            let url = self.build_user_timeline_url(
                user_id,
                current_page_size,
                next_token.as_deref(),
                since_id,
            );

            let response = self.api_request(user_id, &url).await?;

            last_response = parse_http_response_json(response, "Twitter API timeline").await?;

            // Process the current page
            if let Some(page_tweets) = last_response.data.clone() {
                // Track how many tweets we fetched in this page
                let page_tweet_count = page_tweets.len() as u32;
                all_tweets.extend(page_tweets);

                // Update remaining count
                remaining_count = remaining_count.saturating_sub(page_tweet_count);
            } else {
                // No tweets in this page, break the loop
                break;
            }

            // Check if there's a next page
            next_token = last_response
                .meta
                .as_ref()
                .and_then(|m| m.next_token.clone());

            // If we don't have a next token or we already have enough tweets, break the loop
            if next_token.is_none() || all_tweets.len() >= requested_count as usize {
                break;
            }

            // Small delay to avoid hitting rate limits too hard
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // Create a result with all tweets collected across pages
        let mut result = last_response;
        result.data = Some(all_tweets);
        Ok(result)
    }

    /// Builds a Twitter API URL for fetching a specific tweet with all necessary fields
    fn build_tweet_url(&self, tweet_id: &str) -> String {
        format!(
            "{TWITTER_API_BASE}/tweets/{tweet_id}?expansions={COMMON_EXPANSIONS}\
            &media.fields={COMMON_MEDIA_FIELDS}\
            &tweet.fields={COMMON_TWEET_FIELDS},attachments,public_metrics,conversation_id,context_annotations\
            &user.fields={COMMON_USER_FIELDS}"
        )
    }

    /// Builds a Twitter API URL for fetching a user's timeline
    fn build_user_timeline_url(
        &self,
        user_id: &str,
        max_results: u32,
        pagination_token: Option<&str>,
        since_id: Option<&str>,
    ) -> String {
        let base = format!("{TWITTER_API_BASE}/users/{user_id}/tweets?max_results={max_results}");

        // Build URL with common parameters
        let params = format!("&expansions={COMMON_EXPANSIONS}&media.fields={COMMON_MEDIA_FIELDS}&tweet.fields={COMMON_TWEET_FIELDS}&user.fields={COMMON_USER_FIELDS}");

        // Add pagination token if provided
        let token_param =
            pagination_token.map_or(String::new(), |token| format!("&pagination_token={token}"));

        // Add since_id if provided (for efficient pagination)
        let since_param = since_id.map_or(String::new(), |id| format!("&since_id={id}"));

        format!("{base}{params}{token_param}{since_param}")
    }

    /// Get a user's profile from their username
    pub async fn get_user_by_username(&self, username: &str) -> Result<User> {
        let url = format!(
            "{TWITTER_API_BASE}/users/by/username/{username}?user.fields={COMMON_USER_FIELDS}"
        );

        let response = self
            .api_request(&format!("user:{username}"), &url)
            .await
            .context("API request for user profile failed")?;

        let mut user = response
            .json::<UserProfileResponse>()
            .await
            .context("Failed to parse user profile response")?;

        // Resolve the shortened URL in the user's profile
        if let Some(url_entity) = &mut user.data.entities {
            if let Some(url_data) = &mut url_entity.url {
                if let Some(urls) = &mut url_data.urls {
                    for url_item in urls {
                        if let Ok(expanded_url) = resolve_shortened_url(&url_item.url).await {
                            url_item.expanded_url = expanded_url;
                        }
                    }
                }
            }
        }

        // Also update the top-level URL if it exists
        if let Some(url) = &user.data.url {
            if let Ok(expanded_url) = resolve_shortened_url(url).await {
                user.data.url = Some(expanded_url);
            }
        }

        Ok(user.data)
    }

    /// Download profiles for multiple usernames, returning successfully downloaded profiles
    /// Skips profiles that already exist in cache or fail to download
    pub async fn download_user_profiles(
        &self,
        usernames: &[String],
        output_dir: &std::path::Path,
    ) -> Result<Vec<User>> {
        use crate::profile_collector::filter_uncached_usernames;
        use crate::storage::save_user_profile;
        use std::collections::HashSet;

        if usernames.is_empty() {
            return Ok(Vec::new());
        }

        // Convert to HashSet for filtering
        let username_set: HashSet<String> = usernames.iter().cloned().collect();

        // Filter out already cached profiles
        let uncached_usernames = filter_uncached_usernames(username_set, output_dir).await?;

        if uncached_usernames.is_empty() {
            debug!("All {} profiles already cached", usernames.len());
            return Ok(Vec::new());
        }

        info!(
            "Downloading {} new profiles (out of {} referenced users)",
            uncached_usernames.len(),
            usernames.len()
        );

        let mut downloaded_profiles = Vec::new();
        let mut failed_count = 0;

        for username in &uncached_usernames {
            debug!("Downloading profile for @{username}");

            match self.get_user_by_username(username).await {
                Ok(user) => {
                    // Save the profile
                    if let Err(e) = save_user_profile(&user, output_dir) {
                        debug!("Failed to save profile for @{username}: {e}");
                    } else {
                        debug!("Successfully saved profile for @{username}");
                        downloaded_profiles.push(user);
                    }
                }
                Err(e) => {
                    debug!("Failed to download profile for @{username}: {e}");
                    failed_count += 1;
                }
            }
        }

        if failed_count > 0 {
            debug!("{failed_count} profiles failed to download");
        }

        info!(
            "Successfully downloaded and saved {} new profiles",
            downloaded_profiles.len()
        );

        Ok(downloaded_profiles)
    }

    /// Get a user's ID from their username
    async fn get_user_id(&self, username: &str) -> Result<String> {
        let url = format!("{TWITTER_API_BASE}/users/by/username/{username}");

        let response = self.api_request(username, &url).await?;

        let data: serde_json::Value = parse_http_response_json(response, "Twitter API").await?;

        let user_id = data["data"]["id"]
            .as_str()
            .context("Failed to extract user ID")?;

        Ok(user_id.to_string())
    }

    /// Fetch a tweet with extended media information to get video URLs
    pub async fn get_tweet_with_media(&self, tweet_id: &str) -> Result<Tweet> {
        let url = format!(
            "{TWITTER_API_BASE}/tweets/{tweet_id}?expansions=attachments.media_keys&media.fields=url,preview_image_url,alt_text,variants,media_key,type&tweet.fields=created_at,entities,referenced_tweets,author_id,note_tweet"
        );

        debug!("Fetching tweet with media: {url}");
        let response = self
            .api_request(&format!("tweet:{tweet_id}"), &url)
            .await
            .context("API request for tweet with media failed")?;

        // Parse the response
        let mut tweet_data: Tweet =
            parse_http_response_json(response, "tweet response with media").await?;

        // Check if we have includes with media
        if let Some(includes) = &mut tweet_data.includes {
            if let Some(media_items) = &includes.media {
                debug!(
                    "Found media items in tweet response: {} items",
                    media_items.len()
                );
            } else {
                debug!("No media items found in includes");
            }
        } else {
            debug!("No includes found in tweet response");
        }

        Ok(tweet_data)
    }
}

/// Extracts a tweet ID from a URL or returns the ID if it's already an ID
/// Resolves a shortened URL (like t.co) by making a HEAD request and returning the final URL.
async fn resolve_shortened_url(url: &str) -> Result<String> {
    debug!("Resolving shortened URL: {url}");
    let client = create_http_client_with_context()?;
    let response = client
        .head(url)
        .send()
        .await
        .context("Failed to send HEAD request to resolve shortened URL")?;
    let final_url = response.url().to_string();
    debug!("Resolved {url} to {final_url}");
    Ok(final_url)
}

pub fn parse_tweet_id(url_or_id: &str) -> Result<String> {
    // Check for empty string
    if url_or_id.is_empty() {
        bail!("Tweet ID cannot be empty");
    }

    // If it's already just a numeric ID, return it
    if url_or_id.chars().all(|c| c.is_ascii_digit()) {
        return Ok(url_or_id.to_string());
    }

    // Try to parse as URL
    if let Ok(parsed_url) = Url::parse(url_or_id) {
        // Check if it's a Twitter/X URL
        if parsed_url
            .host_str()
            .is_some_and(|h| h.contains("twitter.com") || h.contains("x.com"))
        {
            // Extract the tweet ID from path segments
            let path_segments: Vec<&str> = parsed_url
                .path_segments()
                .map_or(Vec::new(), |s| s.collect());

            // Path format should be /username/status/tweet_id
            if path_segments.len() >= 3 && path_segments[1] == "status" {
                return Ok(path_segments[2].to_string());
            }
        }
    }

    // Try to match with regex for URLs that might not parse correctly
    let re = Regex::new(r"(?:twitter\.com|x\.com)/\w+/status/(\d+)")
        .context("Failed to compile tweet ID regex")?;
    if let Some(captures) = re.captures(url_or_id) {
        if let Some(id_match) = captures.get(1) {
            return Ok(id_match.as_str().to_string());
        }
    }

    bail!("Could not extract tweet ID from: {url_or_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tweet_id_from_url() {
        // Standard URLs
        assert_eq!(
            parse_tweet_id("https://twitter.com/user/status/1234567890").unwrap(),
            "1234567890"
        );
        assert_eq!(
            parse_tweet_id("https://x.com/user/status/1234567890").unwrap(),
            "1234567890"
        );

        // With query parameters
        assert_eq!(
            parse_tweet_id("https://twitter.com/user/status/1234567890?s=20").unwrap(),
            "1234567890"
        );

        // Mobile URLs
        assert_eq!(
            parse_tweet_id("https://mobile.twitter.com/user/status/1234567890").unwrap(),
            "1234567890"
        );

        // Just the ID
        assert_eq!(parse_tweet_id("1234567890").unwrap(), "1234567890");

        // Invalid URLs
        assert!(parse_tweet_id("https://twitter.com/user").is_err());
        assert!(parse_tweet_id("not-a-url").is_err());
        assert!(parse_tweet_id("").is_err());
    }

    #[test]
    fn test_parse_tweet_json() {
        let tweet_json = serde_json::json!({
            "id": "1234567890",
            "text": "This is a test tweet",
            "created_at": "2023-01-01T00:00:00Z",
            "author_id": "987654321",
            "entities": {
                "urls": [{
                    "start": 0,
                    "end": 23,
                    "url": "https://t.co/abc123",
                    "expanded_url": "https://example.com",
                    "display_url": "example.com"
                }]
            }
        });

        let tweet: Tweet = serde_json::from_value(tweet_json).unwrap();
        assert_eq!(tweet.id, "1234567890");
        assert_eq!(tweet.text, "This is a test tweet");
        assert_eq!(tweet.author_id, Some("987654321".to_string()));
        assert!(tweet.entities.is_some());
    }

    #[test]
    fn test_parse_tweet_with_media() {
        let tweet_json = serde_json::json!({
            "id": "1234567890",
            "text": "Tweet with media",
            "created_at": "2023-01-01T00:00:00Z",
            "author_id": "987654321",
            "attachments": {
                "media_keys": ["3_1234567890"]
            }
        });

        let tweet: Tweet = serde_json::from_value(tweet_json).unwrap();
        assert!(tweet.attachments.is_some());
        let attachments = tweet.attachments.unwrap();
        assert_eq!(attachments.media_keys.unwrap()[0], "3_1234567890");
    }

    #[test]
    fn test_parse_retweet() {
        let tweet_json = serde_json::json!({
            "id": "1234567890",
            "text": "RT @original_user: Original tweet text",
            "created_at": "2023-01-01T00:00:00Z",
            "author_id": "987654321",
            "referenced_tweets": [{
                "type": "retweeted",
                "id": "1111111111"
            }]
        });

        let tweet: Tweet = serde_json::from_value(tweet_json).unwrap();
        assert!(tweet.referenced_tweets.is_some());
        let refs = tweet.referenced_tweets.unwrap();
        assert_eq!(refs[0].type_field, "retweeted");
        assert_eq!(refs[0].id, "1111111111");
    }

    #[test]
    fn test_parse_quoted_tweet() {
        let tweet_json = serde_json::json!({
            "id": "1234567890",
            "text": "Quoting this tweet",
            "created_at": "2023-01-01T00:00:00Z",
            "author_id": "987654321",
            "referenced_tweets": [{
                "type": "quoted",
                "id": "2222222222"
            }]
        });

        let tweet: Tweet = serde_json::from_value(tweet_json).unwrap();
        assert!(tweet.referenced_tweets.is_some());
        let refs = tweet.referenced_tweets.unwrap();
        assert_eq!(refs[0].type_field, "quoted");
        assert_eq!(refs[0].id, "2222222222");
    }

    #[test]
    fn test_parse_note_tweet() {
        let tweet_json = serde_json::json!({
            "id": "1234567890",
            "text": "Short preview...",
            "created_at": "2023-01-01T00:00:00Z",
            "author_id": "987654321",
            "note_tweet": {
                "text": "This is a very long tweet that exceeds the normal character limit. ".repeat(10)
            }
        });

        let tweet: Tweet = serde_json::from_value(tweet_json).unwrap();
        assert!(tweet.note_tweet.is_some());
        assert!(tweet.note_tweet.unwrap().text.len() > 280);
    }

    #[test]
    fn test_parse_user() {
        let user_json = serde_json::json!({
            "id": "987654321",
            "name": "Test User",
            "username": "testuser",
            "profile_image_url": "https://example.com/profile.jpg",
            "description": "Test user description",
            "url": "https://example.com"
        });

        let user: User = serde_json::from_value(user_json).unwrap();
        assert_eq!(user.id, "987654321");
        assert_eq!(user.username, "testuser");
        assert_eq!(user.name, Some("Test User".to_string()));
        assert_eq!(user.description, Some("Test user description".to_string()));
    }

    #[test]
    fn test_parse_media() {
        let media_json = serde_json::json!({
            "media_key": "3_1234567890",
            "type": "photo",
            "url": "https://pbs.twimg.com/media/abc123.jpg"
        });

        let media: Media = serde_json::from_value(media_json).unwrap();
        assert_eq!(media.media_key, "3_1234567890");
        assert_eq!(media.type_field, "photo");
        assert_eq!(
            media.url,
            Some("https://pbs.twimg.com/media/abc123.jpg".to_string())
        );
    }

    #[test]
    fn test_parse_video_media() {
        let media_json = serde_json::json!({
            "media_key": "7_1234567890",
            "type": "video",
            "preview_image_url": "https://pbs.twimg.com/media/preview.jpg",
            "variants": [{
                "bit_rate": 2176000,
                "content_type": "video/mp4",
                "url": "https://video.twimg.com/video.mp4"
            }]
        });

        let media: Media = serde_json::from_value(media_json).unwrap();
        assert_eq!(media.type_field, "video");
        assert!(media.variants.is_some());
        let variants = media.variants.unwrap();
        assert_eq!(variants[0].content_type, "video/mp4");
    }
}
