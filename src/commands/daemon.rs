use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::signal;
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio::time;
use tracing::{debug, error, info, trace, warn};

use crate::nostr;
use crate::nostr_profile;
use crate::profile_collector;
use crate::storage;
use crate::twitter::TwitterClient;

/// Configuration for the daemon
pub struct DaemonConfig {
    pub users: Vec<String>,
    pub relays: Vec<String>,
    pub blossom_servers: Vec<String>,
    pub poll_interval: u64,
    pub output_dir: std::path::PathBuf,
}

/// Per-user state tracking
#[derive(Clone, Debug)]
pub struct UserState {
    pub _username: String,
    pub last_poll_time: Option<Instant>,
    pub last_success_time: Option<Instant>,
    pub consecutive_failures: u32,
    pub total_tweets_downloaded: u64,
    pub total_tweets_posted: u64,
    pub is_processing: bool,
}

impl UserState {
    fn new(username: String) -> Self {
        Self {
            _username: username,
            last_poll_time: None,
            last_success_time: None,
            consecutive_failures: 0,
            total_tweets_downloaded: 0,
            total_tweets_posted: 0,
            is_processing: false,
        }
    }

    /// Calculate the next poll time based on failures and success rate
    fn next_poll_delay(&self, base_interval: u64) -> Duration {
        // Exponential backoff for failures
        if self.consecutive_failures > 0 {
            let backoff_seconds = base_interval * 2_u64.pow(self.consecutive_failures.min(5));
            return Duration::from_secs(backoff_seconds.min(3600)); // Max 1 hour
        }

        Duration::from_secs(base_interval)
    }
}

/// Global daemon statistics
#[derive(Clone, Debug)]
pub struct DaemonStats {
    pub start_time: Instant,
    pub total_polls: u64,
    pub successful_polls: u64,
    pub failed_polls: u64,
    pub total_tweets_downloaded: u64,
    pub total_tweets_posted: u64,
    pub _total_profiles_downloaded: u64,
    pub _total_profiles_posted: u64,
}

/// State for the running daemon
pub struct DaemonState {
    pub config: Arc<DaemonConfig>,
    pub twitter_client: Arc<TwitterClient>,
    pub nostr_client: Arc<nostr_sdk::Client>,
    pub user_states: Arc<RwLock<HashMap<String, UserState>>>,
    pub stats: Arc<RwLock<DaemonStats>>,
    pub rate_limiter: Arc<Mutex<RateLimiter>>,
}

/// Rate limiter for Twitter API
pub struct RateLimiter {
    requests_per_window: u32,
    window_duration: Duration,
    request_times: std::collections::VecDeque<Instant>,
}

impl RateLimiter {
    fn new(requests_per_window: u32, window_seconds: u64) -> Self {
        Self {
            requests_per_window,
            window_duration: Duration::from_secs(window_seconds),
            request_times: std::collections::VecDeque::new(),
        }
    }

    async fn wait_if_needed(&mut self) {
        // Remove old requests outside window
        let cutoff = Instant::now() - self.window_duration;
        while let Some(&front) = self.request_times.front() {
            if front < cutoff {
                self.request_times.pop_front();
            } else {
                break;
            }
        }

        // If at limit, wait
        if self.request_times.len() >= self.requests_per_window as usize {
            if let Some(&oldest) = self.request_times.front() {
                let wait_until = oldest + self.window_duration;
                let wait_duration = wait_until.saturating_duration_since(Instant::now());
                if wait_duration > Duration::ZERO {
                    info!("Rate limit reached, waiting {:?}", wait_duration);
                    time::sleep(wait_duration).await;
                }
            }
        }

        // Record this request
        self.request_times.push_back(Instant::now());
    }
}

/// Main entry point for daemon mode
pub async fn execute(
    users: Vec<String>,
    relays: Vec<String>,
    blossom_servers: Vec<String>,
    poll_interval: u64,
    output_dir: &Path,
) -> Result<()> {
    info!(
        "Starting daemon v2 for {user_count} users with {poll_interval} second base interval",
        user_count = users.len()
    );

    let config = Arc::new(DaemonConfig {
        users: users.clone(),
        relays,
        blossom_servers,
        poll_interval,
        output_dir: output_dir.to_path_buf(),
    });

    // Initialize daemon state
    let state = init_daemon(config).await?;

    // Set up graceful shutdown
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    // Spawn a task to handle shutdown signals
    tokio::spawn(async move {
        let _ = signal::ctrl_c().await;
        info!("Received shutdown signal (Ctrl+C)");
        let _ = shutdown_tx.send(());
    });

    // Spawn statistics reporter
    let stats_clone = state.stats.clone();
    let stats_handle = spawn_stats_reporter(stats_clone);

    // Save stats reference for shutdown
    let final_stats = state.stats.clone();

    // Run the main daemon loop with shutdown handling
    tokio::select! {
        result = run_daemon_v2(state.clone()) => {
            if let Err(e) = result {
                error!("Daemon error: {e}");
                return Err(e);
            }
            Ok(())
        }
        _ = shutdown_rx => {
            info!("Received shutdown signal, gracefully shutting down daemon...");

            // Cancel the stats reporter
            stats_handle.abort();

            // No longer saving daemon state - all state inferred from disk cache

            // Print final statistics
            print_final_stats(&final_stats).await;
            info!("Daemon shutdown complete");
            Ok(())
        }
    }
}

/// Initialize the daemon state
async fn init_daemon(config: Arc<DaemonConfig>) -> Result<DaemonState> {
    // Ensure output directory exists
    if !config.output_dir.exists() {
        std::fs::create_dir_all(&config.output_dir).context("Failed to create output directory")?;
    }

    // Initialize Twitter client
    info!("Initializing Twitter client");
    let twitter_client = Arc::new(
        TwitterClient::new(&config.output_dir).context("Failed to initialize Twitter client")?,
    );

    // Connect to Nostr relays
    info!(
        "Connecting to {relay_count} Nostr relays",
        relay_count = config.relays.len()
    );
    // Use ephemeral keys for relay connection (just for subscribing/querying)
    let keys = nostr_sdk::Keys::generate();
    let nostr_client = Arc::new(
        nostr::initialize_nostr_client(&keys, &config.relays)
            .await
            .context("Failed to connect to Nostr relays")?,
    );

    // Initialize user states
    let mut user_states = HashMap::new();
    for username in &config.users {
        user_states.insert(username.clone(), UserState::new(username.clone()));
    }

    // Initialize rate limiter (100 requests per 15 minutes for Twitter API)
    let rate_limiter = Arc::new(Mutex::new(RateLimiter::new(100, 900)));

    Ok(DaemonState {
        config,
        twitter_client,
        nostr_client,
        user_states: Arc::new(RwLock::new(user_states)),
        stats: Arc::new(RwLock::new(DaemonStats {
            start_time: Instant::now(),
            total_polls: 0,
            successful_polls: 0,
            failed_polls: 0,
            total_tweets_downloaded: 0,
            total_tweets_posted: 0,
            _total_profiles_downloaded: 0,
            _total_profiles_posted: 0,
        })),
        rate_limiter,
    })
}

/// Main daemon loop with concurrent processing
async fn run_daemon_v2(state: DaemonState) -> Result<()> {
    info!("Daemon v2 started with concurrent processing support");

    loop {
        let poll_start = Instant::now();

        // Get users that need polling
        let users_to_poll = get_users_ready_for_polling(&state).await;

        if users_to_poll.is_empty() {
            trace!("No users ready for polling, sleeping for 10 seconds");
            time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        info!(
            "Starting sequential polling for {user_count} users",
            user_count = users_to_poll.len()
        );

        // Process users sequentially with rate limiting
        // Note: Sequential processing respects Twitter API rate limits via the RateLimiter
        for username in &users_to_poll {
            // Process user with enhanced error handling
            match process_user_v2(state.clone(), username.clone()).await {
                Ok(()) => {
                    debug!("Successfully processed user: {username}");
                    // Reset failure count on success
                    let mut user_states = state.user_states.write().await;
                    if let Some(user_state) = user_states.get_mut(username) {
                        user_state.consecutive_failures = 0;
                        user_state.last_success_time = Some(Instant::now());
                    }
                }
                Err(e) => {
                    error!("Error processing user @{username}: {e}");

                    // Update failure count
                    let mut user_states = state.user_states.write().await;
                    if let Some(user_state) = user_states.get_mut(username) {
                        user_state.consecutive_failures += 1;

                        // Log different levels based on failure count
                        match user_state.consecutive_failures {
                            1..=2 => warn!(
                                "User @{username} failed {failures} times, will retry with backoff",
                                failures = user_state.consecutive_failures
                            ),
                            3..=5 => error!(
                                "User @{username} failed {failures} times, increasing backoff delay",
                                failures = user_state.consecutive_failures
                            ),
                            _ => error!(
                                "User @{username} failed {failures} times, may need manual intervention",
                                failures = user_state.consecutive_failures
                            ),
                        }
                    }

                    // Check if it's a rate limit error and handle accordingly
                    if e.to_string().contains("rate limit") || e.to_string().contains("429") {
                        warn!("Rate limit detected for @{username}, will back off");
                        // Rate limiter will handle the backoff automatically
                    }
                }
            }
        }

        let poll_duration = poll_start.elapsed();

        // Update statistics
        let mut stats_guard = state.stats.write().await;
        stats_guard.total_polls += 1;
        if !users_to_poll.is_empty() {
            stats_guard.successful_polls += 1;
        }
        drop(stats_guard);

        info!(
            "Polling cycle completed in {duration:.2}s - processed {user_count} users",
            duration = poll_duration.as_secs_f64(),
            user_count = users_to_poll.len()
        );

        // Log detailed status every 10 cycles (roughly every hour with 6min intervals)
        let stats = state.stats.read().await;
        if stats.total_polls % 10 == 0 {
            let uptime = stats.start_time.elapsed();
            let user_states = state.user_states.read().await;
            let healthy_users = user_states
                .values()
                .filter(|u| u.consecutive_failures == 0)
                .count();
            let failing_users = user_states
                .values()
                .filter(|u| u.consecutive_failures > 0)
                .count();

            info!("=== Daemon Status Report ===");
            info!("Uptime: {:.1} hours", uptime.as_secs_f64() / 3600.0);
            info!(
                "Total polls: {total_polls}, Success rate: {success_rate:.1}%",
                total_polls = stats.total_polls,
                success_rate = if stats.total_polls > 0 {
                    (stats.successful_polls as f64 / stats.total_polls as f64) * 100.0
                } else {
                    0.0
                }
            );
            info!(
                "Tweets: {total_tweets_downloaded} downloaded, {total_tweets_posted} posted to Nostr",
                total_tweets_downloaded = stats.total_tweets_downloaded,
                total_tweets_posted = stats.total_tweets_posted
            );
            info!("Users: {healthy_users} healthy, {failing_users} failing");

            // Log failing users for debugging
            if failing_users > 0 {
                for (username, state) in user_states.iter() {
                    if state.consecutive_failures > 0 {
                        warn!(
                            "User @{username} has {failures} consecutive failures",
                            username = username,
                            failures = state.consecutive_failures
                        );
                    }
                }
            }
            info!("=============================");
        }

        // Short sleep before next cycle
        time::sleep(Duration::from_secs(5)).await;
    }
}

/// Get users that are ready for polling based on their state
async fn get_users_ready_for_polling(state: &DaemonState) -> Vec<String> {
    let user_states = state.user_states.read().await;
    let mut ready_users = Vec::new();

    for (username, user_state) in user_states.iter() {
        // Skip if already processing
        if user_state.is_processing {
            continue;
        }

        // Check if enough time has passed since last poll
        let delay = user_state.next_poll_delay(state.config.poll_interval);
        let should_poll = match user_state.last_poll_time {
            None => true, // Never polled
            Some(last) => last.elapsed() >= delay,
        };

        if should_poll {
            ready_users.push(username.clone());
        }
    }

    ready_users
}

/// Process a single user with better error handling and state tracking
async fn process_user_v2(state: DaemonState, username: String) -> Result<()> {
    // Mark as processing
    {
        let mut user_states = state.user_states.write().await;
        if let Some(user_state) = user_states.get_mut(&username) {
            user_state.is_processing = true;
            user_state.last_poll_time = Some(Instant::now());
        }
    }

    debug!("Processing user: @{username}");

    // Apply rate limiting
    state.rate_limiter.lock().await.wait_if_needed().await;

    // Process and track results
    let result = process_user_tweets(&state, &username).await;

    // Update user state based on result
    {
        let mut user_states = state.user_states.write().await;
        let mut stats = state.stats.write().await;

        if let Some(user_state) = user_states.get_mut(&username) {
            user_state.is_processing = false;

            match &result {
                Ok((downloaded, posted)) => {
                    user_state.last_success_time = Some(Instant::now());
                    user_state.consecutive_failures = 0;
                    user_state.total_tweets_downloaded += downloaded;
                    user_state.total_tweets_posted += posted;

                    stats.successful_polls += 1;
                    stats.total_tweets_downloaded += downloaded;
                    stats.total_tweets_posted += posted;

                    if *downloaded > 0 || *posted > 0 {
                        info!("User @{username}: downloaded {downloaded} tweets, posted {posted} to Nostr");
                    }
                }
                Err(e) => {
                    user_state.consecutive_failures += 1;
                    stats.failed_polls += 1;

                    warn!(
                        "Failed to process @{username} (failure #{failures}): {e}",
                        failures = user_state.consecutive_failures
                    );
                }
            }

            stats.total_polls += 1;
        }
    }

    result.map(|_| ())
}

/// Process tweets for a user (returns downloaded count and posted count)
async fn process_user_tweets(state: &DaemonState, username: &str) -> Result<(u64, u64)> {
    // Find the latest tweet ID we already have for smart resume
    let since_id = storage::find_latest_tweet_id_for_user(username, &state.config.output_dir)?;

    if let Some(ref id) = since_id {
        debug!("Resuming from tweet ID {id} for @{username}");
    } else {
        debug!("No cached tweets found for @{username}, fetching from beginning");
    }

    // Fetch recent tweets with retry and smart resume
    let tweets = fetch_timeline_with_retry(&state.twitter_client, username, since_id)
        .await
        .with_context(|| format!("Failed to fetch timeline for @{username}"))?;

    if tweets.is_empty() {
        debug!("No tweets found for @{username}");
        return Ok((0, 0));
    }

    let mut new_tweet_count = 0u64;
    let mut posted_to_nostr_count = 0u64;

    for tweet in tweets {
        let tweet_id = &tweet.id;

        // Check if tweet is already cached
        if is_tweet_cached(tweet_id, &state.config.output_dir) {
            // Even if cached, check if it needs to be posted to Nostr
            // Get keys for checking (we need to load the cached tweet to get the author ID)
            if let Some(tweet_path) =
                storage::find_existing_tweet_json(tweet_id, &state.config.output_dir)
            {
                let cached_tweet = storage::load_tweet_from_file(&tweet_path)?;

                // Get keys for this tweet's author
                let keys = crate::keys::get_keys_for_tweet(&cached_tweet.author.id)?;

                // Check if already posted to Nostr by querying the relay
                if !is_tweet_posted_to_nostr(tweet_id, &state.nostr_client, &keys).await? {
                    // Post the cached tweet to Nostr
                    if post_tweet_to_nostr_with_state(&cached_tweet, state)
                        .await
                        .is_ok()
                    {
                        posted_to_nostr_count += 1;

                        // Also post referenced profiles for cached tweets
                        let referenced_usernames =
                            profile_collector::collect_usernames_from_tweet(&cached_tweet);
                        if !referenced_usernames.is_empty() {
                            let _ = nostr_profile::post_referenced_profiles(
                                &referenced_usernames,
                                &state.nostr_client,
                                &state.config.output_dir,
                            )
                            .await;
                        }
                    }
                }
            }
            continue;
        }

        // New tweet - process it
        let mut enriched_tweet = tweet.clone();

        // Enrich with referenced tweets
        if let Err(e) = state
            .twitter_client
            .enrich_referenced_tweets(&mut enriched_tweet, Some(&state.config.output_dir))
            .await
        {
            debug!("Failed to enrich referenced tweets for {tweet_id}: {e}");
        }

        // Download media
        let _media_results =
            crate::media::download_media(&enriched_tweet, &state.config.output_dir)
                .await
                .with_context(|| format!("Failed to download media for tweet {tweet_id}"))?;

        // Save tweet
        storage::save_tweet(&enriched_tweet, &state.config.output_dir)?;
        new_tweet_count += 1;

        // Download referenced profiles
        let referenced_usernames = profile_collector::collect_usernames_from_tweet(&enriched_tweet);
        if !referenced_usernames.is_empty() {
            let username_vec: Vec<String> = referenced_usernames.clone().into_iter().collect();
            let _ = state
                .twitter_client
                .download_user_profiles(&username_vec, &state.config.output_dir)
                .await;
        }

        // Check if already posted to Nostr before attempting to post
        let keys = crate::keys::get_keys_for_tweet(&enriched_tweet.author.id)?;

        if !is_tweet_posted_to_nostr(tweet_id, &state.nostr_client, &keys).await? {
            // Post to Nostr
            if post_tweet_to_nostr_with_state(&enriched_tweet, state)
                .await
                .is_ok()
            {
                posted_to_nostr_count += 1;

                // Post referenced profiles to Nostr
                if !referenced_usernames.is_empty() {
                    let _ = nostr_profile::post_referenced_profiles(
                        &referenced_usernames,
                        &state.nostr_client,
                        &state.config.output_dir,
                    )
                    .await;
                }
            }
        } else {
            debug!("Tweet {tweet_id} already posted to Nostr, skipping");
        }
    }

    Ok((new_tweet_count, posted_to_nostr_count))
}

/// Spawn a task to periodically report statistics
fn spawn_stats_reporter(stats: Arc<RwLock<DaemonStats>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            let stats = stats.read().await;
            let uptime = stats.start_time.elapsed();
            let hours = uptime.as_secs() / 3600;
            let minutes = (uptime.as_secs() % 3600) / 60;

            info!(
                "📊 Stats | Uptime: {hours}h{minutes}m | Polls: {total_polls} (✓{successful_polls} ✗{failed_polls}) | Downloaded: {total_tweets_downloaded} | Posted: {total_tweets_posted}",
                total_polls = stats.total_polls,
                successful_polls = stats.successful_polls,
                failed_polls = stats.failed_polls,
                total_tweets_downloaded = stats.total_tweets_downloaded,
                total_tweets_posted = stats.total_tweets_posted
            );
        }
    })
}

/// Print final statistics on shutdown
async fn print_final_stats(stats: &Arc<RwLock<DaemonStats>>) {
    let stats = stats.read().await;
    let uptime = stats.start_time.elapsed();

    info!("=== Final Daemon Statistics ===");
    info!(
        "Uptime: {uptime:.2} hours",
        uptime = uptime.as_secs_f64() / 3600.0
    );
    info!(
        "Total polls: {total_polls}",
        total_polls = stats.total_polls
    );
    info!(
        "Successful polls: {successful_polls}",
        successful_polls = stats.successful_polls
    );
    info!(
        "Failed polls: {failed_polls}",
        failed_polls = stats.failed_polls
    );
    info!(
        "Total tweets downloaded: {total_tweets_downloaded}",
        total_tweets_downloaded = stats.total_tweets_downloaded
    );
    info!(
        "Total tweets posted to Nostr: {total_tweets_posted}",
        total_tweets_posted = stats.total_tweets_posted
    );
    info!("===============================");
}

// Helper functions

/// Check if a tweet is already cached
pub fn is_tweet_cached(tweet_id: &str, output_dir: &Path) -> bool {
    storage::find_existing_tweet_json(tweet_id, output_dir).is_some()
}

/// Check if a tweet has been posted to Nostr by querying the relay
pub async fn is_tweet_posted_to_nostr(
    tweet_id: &str,
    nostr_client: &nostr_sdk::Client,
    keys: &nostr_sdk::Keys,
) -> Result<bool> {
    // Use the existing find_existing_event function to check the relay
    match crate::nostr::find_existing_event(nostr_client, tweet_id, keys).await {
        Ok(Some(_event)) => {
            debug!("Tweet {tweet_id} already exists on Nostr relay");
            Ok(true)
        }
        Ok(None) => {
            debug!("Tweet {tweet_id} not found on Nostr relay");
            Ok(false)
        }
        Err(e) => {
            // Log the error but don't fail - assume not posted to be safe
            warn!("Failed to check if tweet {tweet_id} exists on relay: {e}");
            Ok(false)
        }
    }
}

/// Fetch timeline with exponential backoff retry and smart resume using since_id
pub async fn fetch_timeline_with_retry(
    client: &TwitterClient,
    username: &str,
    since_id: Option<String>,
) -> Result<Vec<crate::twitter::Tweet>> {
    let backoff = backoff::ExponentialBackoff {
        initial_interval: Duration::from_secs(1),
        randomization_factor: 0.1,
        multiplier: 2.0,
        max_interval: Duration::from_secs(60),
        max_elapsed_time: Some(Duration::from_secs(300)),
        ..Default::default()
    };

    backoff::future::retry(backoff, || async {
        match client
            .get_user_timeline_with_since_id(username, Some(20), since_id.clone())
            .await
        {
            Ok(tweets) => Ok(tweets),
            Err(e) => {
                let error_str = e.to_string();

                // Check if it's a rate limit error
                if error_str.contains("429") || error_str.contains("rate limit") {
                    warn!("Rate limited when fetching @{username}, backing off");
                    Err(backoff::Error::transient(e))
                }
                // Check if it's a network error
                else if error_str.contains("network")
                    || error_str.contains("connection")
                    || error_str.contains("timeout")
                {
                    warn!("Network error when fetching @{username}, retrying");
                    Err(backoff::Error::transient(e))
                }
                // User not found or other permanent errors
                else if error_str.contains("404") || error_str.contains("not found") {
                    error!("User @{username} not found");
                    Err(backoff::Error::permanent(e))
                }
                // Unknown error - treat as permanent
                else {
                    error!("Permanent error fetching @{username}: {e}");
                    Err(backoff::Error::permanent(e))
                }
            }
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("Failed after retries: {e}"))
}

/// Post a tweet to Nostr using the daemon state v2
async fn post_tweet_to_nostr_with_state(
    tweet: &crate::twitter::Tweet,
    state: &DaemonState,
) -> Result<nostr_sdk::EventId> {
    use crate::datetime_utils::parse_rfc3339;
    use crate::media;
    use nostr_sdk::{EventBuilder, Kind, Timestamp};

    let tweet_id = &tweet.id;

    // Get keys for the tweet
    let keys = crate::keys::get_keys_for_tweet(&tweet.author.id)?;

    // Extract media URLs
    let tweet_media_urls = media::extract_media_urls_from_tweet(tweet);

    // Upload media to Blossom if configured
    let media_files = Vec::new(); // For daemon, we assume media is already downloaded
    let blossom_urls = if !media_files.is_empty() && !state.config.blossom_servers.is_empty() {
        nostr::upload_media_to_blossom(&media_files, &state.config.blossom_servers, &keys).await?
    } else {
        Vec::new()
    };

    // Choose which URLs to include
    let media_urls = if blossom_urls.is_empty() {
        tweet_media_urls.clone()
    } else {
        blossom_urls.clone()
    };

    // Create resolver for mentions
    let cache_dir = Some(state.config.output_dir.to_string_lossy().to_string());
    let mut resolver = crate::nostr_linking::NostrLinkResolver::new(cache_dir);

    // Format content
    let (content, mentioned_pubkeys) =
        nostr::format_tweet_as_nostr_content_with_mentions(tweet, &media_urls, &mut resolver)?;

    // Parse timestamp
    let tweet_created_at = parse_rfc3339(&tweet.created_at)?.timestamp() as u64;
    let timestamp = Timestamp::from(tweet_created_at);

    // Create tags
    let tags = crate::commands::post_tweet_to_nostr::create_nostr_event_tags(
        tweet_id,
        &tweet_media_urls,
        &blossom_urls,
        &mentioned_pubkeys,
    )?;

    // Build event
    let builder = EventBuilder::new(Kind::TextNote, content)
        .custom_created_at(timestamp)
        .tags(tags);

    let event = builder.sign_with_keys(&keys)?;

    // Send event to relays
    let output = state.nostr_client.send_event(&event).await?;
    let event_id = *output.id();

    // Save the event
    storage::save_nostr_event(&event, &state.config.output_dir)?;

    Ok(event_id)
}

// Make DaemonState clonable for concurrent processing
impl Clone for DaemonState {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            twitter_client: self.twitter_client.clone(),
            nostr_client: self.nostr_client.clone(),
            user_states: self.user_states.clone(),
            stats: self.stats.clone(),
            rate_limiter: self.rate_limiter.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_state_backoff() {
        let mut user = UserState::new("test".to_string());

        // No failures - normal interval
        assert_eq!(user.next_poll_delay(300), Duration::from_secs(300));

        // One failure - doubled
        user.consecutive_failures = 1;
        assert_eq!(user.next_poll_delay(300), Duration::from_secs(600));

        // Two failures - quadrupled
        user.consecutive_failures = 2;
        assert_eq!(user.next_poll_delay(300), Duration::from_secs(1200));

        // Max backoff (capped at 1 hour)
        user.consecutive_failures = 10;
        assert_eq!(user.next_poll_delay(300), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let mut limiter = RateLimiter::new(2, 1); // 2 requests per second

        // First two should be immediate
        let start = Instant::now();
        limiter.wait_if_needed().await;
        limiter.wait_if_needed().await;
        assert!(start.elapsed() < Duration::from_millis(100));

        // Third should wait
        limiter.wait_if_needed().await;
        assert!(start.elapsed() >= Duration::from_secs(1));
    }
}
