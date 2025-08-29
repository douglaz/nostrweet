# Daemon Mode Design

## Current Status

The daemon mode is **implemented and functional** with advanced features for production use:

### Available Command

```bash
nostrweet daemon \
  --user alice \
  --user bob \
  --user charlie \
  --relay wss://relay.damus.io \
  --relay wss://nos.lol \
  --blossom-server https://blossom.example.com \
  --max-concurrent 3 \
  --poll-interval 300
```

### Key Features Currently Available
- ✅ Continuous monitoring of multiple Twitter users
- ✅ Automatic posting to Nostr relays
- ✅ Cache-as-state architecture (no separate state files)
- ✅ Automatic recovery from crashes
- ✅ Per-user exponential backoff on failures
- ✅ Rate limiting for Twitter API
- ✅ Statistics reporting every 60 seconds
- ✅ Graceful shutdown with Ctrl+C
- ✅ Downloads and posts referenced profiles
- ✅ Media handling and optional Blossom uploads
- ✅ Smart resume with `since_id` for efficient Twitter API usage


## Overview

The daemon mode continuously monitors a set of Twitter users and automatically posts new tweets to Nostr relays. It uses the local cache as the complete state mechanism, eliminating the need for separate state files.

## Core Architecture

### Cache-as-State Design

The local cache directory serves as the complete state mechanism:
- **Tweet presence**: If `YYYYMMDD_HHMMSS_username_tweetid.json` exists, the tweet has been downloaded
- **Nostr event presence**: If `nostr_events/event_<nostr_event_id>.json` exists, the tweet has been posted to Nostr
- **Profile presence**: If `YYYYMMDD_HHMMSS_username_profile.json` exists, the profile has been downloaded
- **Not-found markers**: `.not_found` files indicate tweets that no longer exist

**Key insight**: The filename format (`YYYYMMDD_HHMMSS_username_tweetid.json`) contains all metadata needed:
- Timestamp tells us when the tweet was created
- Username identifies the Twitter user
- Tweet ID is the unique identifier we can use with Twitter's API

This design ensures:
- State is continuously written during processing (not just at the end)
- Recovery from crashes is automatic (just restart and check cache)
- No state synchronization issues between state file and actual files
- State inspection is simple (just look at the filesystem)
- **The latest tweet ID can be extracted directly from filenames**

## Command Interface

```bash
nostrweet daemon \
  --user user1 \
  --user user2 \
  --user user3 \
  --relay wss://relay1.com \
  --relay wss://relay2.com \
  --poll-interval 300 \
  --data-dir ./downloads
```

### Parameters
- `--user`: Twitter username to monitor (can be specified multiple times)
- `--relay`: Nostr relay to post to (can be specified multiple times)
- `--poll-interval`: Seconds between polling cycles (default: 300)
- `--data-dir`: Data directory for all storage (required)
- `--blossom-server`: Blossom server for media (can be specified multiple times)
- `--mnemonic`: Optional BIP39 mnemonic phrase for deriving Nostr keys

### Clap Definition
```rust
#[derive(Parser, Debug)]
struct DaemonCommand {
    /// Twitter usernames to monitor
    #[arg(short, long = "user", required = true, action = clap::ArgAction::Append)]
    users: Vec<String>,
    
    /// Nostr relay addresses to post to
    #[arg(short, long = "relay", required = true, action = clap::ArgAction::Append)]
    relays: Vec<String>,
    
    /// Blossom server addresses for media uploads
    #[arg(short = 'b', long = "blossom-server", action = clap::ArgAction::Append)]
    blossom_servers: Vec<String>,
    
    /// Seconds between polling cycles
    #[arg(short, long, default_value = "300")]
    poll_interval: u64,
    
    /// BIP39 mnemonic phrase for deriving Nostr keys
    #[arg(long, env = "NOSTRWEET_MNEMONIC")]
    mnemonic: Option<String>,
}
```

## Processing Flow

### 1. Initialization
```rust
async fn init_daemon(config: DaemonConfig) -> Result<DaemonState> {
    // Verify output directory exists
    ensure_output_dir(&config.output_dir)?;
    
    // Test Twitter API access
    let twitter_client = TwitterClient::new(&config.output_dir)?;
    
    // Connect to Nostr relays
    let nostr_client = connect_to_relays(&config.relays).await?;
    
    // Initialize user states from cache
    let user_states = init_user_states(&config.users, &config.output_dir)?;
    
    Ok(DaemonState { 
        config, 
        twitter_client, 
        nostr_client, 
        user_states 
    })
}
```

### 2. Main Loop
```rust
async fn run_daemon(mut state: DaemonState) -> Result<()> {
    loop {
        for username in &state.config.users {
            // Process each user
            process_user(&mut state, username).await?;
            
            // Small delay between users
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        
        // Wait for next polling cycle
        info!("Sleeping for {} seconds", state.config.poll_interval);
        tokio::time::sleep(Duration::from_secs(state.config.poll_interval)).await;
    }
}
```

### 3. User Processing
```rust
async fn process_user(state: &mut DaemonState, username: &str) -> Result<()> {
    info!("Processing user: @{}", username);
    
    // Fetch recent tweets (with retry logic)
    let tweets = fetch_with_retry(&state.twitter_client, username).await?;
    
    for tweet in tweets {
        // Check if already downloaded (cache check)
        if is_tweet_cached(&tweet.id, &state.config.output_dir) {
            debug!("Tweet {} already cached", tweet.id);
            continue;
        }
        
        // Download tweet and media
        download_tweet_and_media(&tweet, &state.config.output_dir).await?;
        info!("Downloaded new tweet: {}", tweet.id);
        
        // Download referenced profiles
        download_referenced_profiles(&tweet, &state.twitter_client).await?;
        
        // Check if already posted to Nostr
        if is_tweet_posted_to_nostr(&tweet.id, &state.config.output_dir) {
            debug!("Tweet {} already posted to Nostr", tweet.id);
            continue;
        }
        
        // Post to Nostr
        post_tweet_to_nostr(&tweet, &state.nostr_client).await?;
        info!("Posted tweet {} to Nostr", tweet.id);
        
        // Post referenced profiles to Nostr
        post_referenced_profiles_to_nostr(&tweet, &state.nostr_client).await?;
    }
    
    Ok(())
}
```

## State Management

### Continuous State Updates

State is written immediately after each operation:

1. **Tweet Download**: Save `tweet.json` immediately after download
2. **Media Download**: Save media files as they complete
3. **Nostr Posting**: Save `nostr_events/event_*.json` immediately after successful post
4. **Profile Updates**: Save profile JSON after each fetch

### Cache Directory Structure
```
downloads/
├── 20240101_120000_user1_123456789.json       # Tweet
├── user1_123456789_1.jpg                       # Media
├── 20240101_120000_user1_profile.json         # Profile
├── tweet_987654321.not_found                   # Not found marker
└── nostr_events/
    └── event_abc123def456.json                # Nostr event
```

### Smart Resume Mechanism

The daemon uses the filesystem to intelligently resume from the exact point where it left off:

#### Finding the Latest Processed Tweet
```rust
fn find_latest_tweet_id_for_user(username: &str, output_dir: &Path) -> Option<String> {
    // List all files matching *_username_*.json
    let pattern = format!("{dir}/*_{username}_*.json", dir = output_dir.display());
    
    let mut latest_tweet_id: Option<String> = None;
    
    for path in glob::glob(&pattern).ok()?.flatten() {
        if let Some(filename) = path.file_stem() {
            // Extract tweet ID from filename (last part after final underscore)
            if let Some(tweet_id) = filename.to_string_lossy().split('_').last() {
                // Twitter IDs are sortable (snowflake IDs increase over time)
                if latest_tweet_id.as_ref().map_or(true, |latest| tweet_id > latest) {
                    latest_tweet_id = Some(tweet_id.to_string());
                }
            }
        }
    }
    
    latest_tweet_id
}
```

#### Efficient Twitter API Usage
```rust
async fn fetch_new_tweets_for_user(client: &TwitterClient, username: &str, output_dir: &Path) -> Result<Vec<Tweet>> {
    // Find the latest tweet we already have
    let since_id = find_latest_tweet_id_for_user(username, output_dir);
    
    // Fetch only tweets newer than our latest
    client.get_user_timeline(username, since_id).await
}
```

#### Benefits of This Approach
1. **No duplicate fetching**: Only retrieves tweets newer than what's cached
2. **Efficient catch-up**: Can resume after any downtime period
3. **Self-healing**: If files are deleted, automatically re-fetches
4. **No state files**: The cache directory IS the state
5. **API-friendly**: Uses Twitter's `since_id` parameter for optimal pagination

### Recovery Mechanism

On startup or after crash:
1. For each user, scan cache to find the latest tweet ID
2. Use `since_id` to fetch only newer tweets from Twitter
3. Check nostr_events/ to see what's been posted
4. Post any cached tweets that haven't been posted yet

This approach is:
- **Deterministic**: Same cache state always produces same behavior
- **Resilient**: No separate state files to corrupt or lose
- **Transparent**: State is human-readable in the filesystem

## Error Handling

### Retry Strategy
```rust
async fn fetch_with_retry(client: &TwitterClient, username: &str) -> Result<Vec<Tweet>> {
    let mut backoff = ExponentialBackoffBuilder::new()
        .with_initial_interval(Duration::from_secs(1))
        .with_max_interval(Duration::from_secs(60))
        .with_max_elapsed_time(Some(Duration::from_secs(300)))
        .build();
    
    backoff::future::retry(backoff, || async {
        match client.get_user_timeline(username, Some(20)).await {
            Ok(tweets) => Ok(tweets),
            Err(e) if is_rate_limit_error(&e) => {
                warn!("Rate limited, backing off");
                Err(backoff::Error::transient(e))
            }
            Err(e) if is_network_error(&e) => {
                warn!("Network error, retrying");
                Err(backoff::Error::transient(e))
            }
            Err(e) => Err(backoff::Error::permanent(e))
        }
    }).await
}
```

### Graceful Shutdown
```rust
async fn main() -> Result<()> {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    
    // Handle SIGTERM/SIGINT
    tokio::spawn(async move {
        signal::ctrl_c().await.unwrap();
        shutdown_tx.send(()).unwrap();
    });
    
    // Run daemon with shutdown signal
    tokio::select! {
        result = run_daemon(state) => result,
        _ = shutdown_rx => {
            info!("Received shutdown signal");
            // State is already persisted continuously
            Ok(())
        }
    }
}
```

## Rate Limiting

### Twitter API Limits
- User timeline: 1500 requests per 15 minutes
- With 10 users, poll every 5 minutes: ~30 requests per 15 minutes (well within limits)

### Implementation
```rust
struct RateLimiter {
    requests_per_window: u32,
    window_duration: Duration,
    request_times: VecDeque<Instant>,
}

impl RateLimiter {
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
            let oldest = self.request_times.front().unwrap();
            let wait_until = *oldest + self.window_duration;
            let wait_duration = wait_until.saturating_duration_since(Instant::now());
            if wait_duration > Duration::ZERO {
                info!("Rate limit reached, waiting {:?}", wait_duration);
                tokio::time::sleep(wait_duration).await;
            }
        }
        
        // Record this request
        self.request_times.push_back(Instant::now());
    }
}
```

## Configuration

### Environment Variables
```bash
# Required
export TWITTER_BEARER_TOKEN="your_token"
export NOSTRWEET_DATA_DIR="./downloads"
export NOSTRWEET_RELAYS="wss://relay1.com,wss://relay2.com"

# Optional
export NOSTRWEET_MNEMONIC="your twelve word mnemonic phrase here"
export RUST_LOG="info,nostrweet=debug"
```

### Configuration File (Future)
```toml
# daemon.toml
[daemon]
poll_interval = 300
max_tweets_per_poll = 20

[twitter]
users = ["user1", "user2", "user3"]

[nostr]
relays = ["wss://relay1.com", "wss://relay2.com"]
blossom_servers = ["https://blossom1.com"]

[rate_limits]
requests_per_window = 100
window_seconds = 900
```

## Monitoring

### Health Checks
```rust
async fn health_check(state: &DaemonState) -> HealthStatus {
    HealthStatus {
        uptime: state.start_time.elapsed(),
        last_poll: state.last_poll_time,
        tweets_downloaded: count_cached_tweets(&state.config.output_dir),
        tweets_posted: count_nostr_events(&state.config.output_dir),
        relay_status: check_relay_connections(&state.nostr_client).await,
    }
}
```

### Metrics
- Tweets downloaded per user
- Tweets posted to Nostr
- API rate limit status
- Relay connection status
- Error counts by type

### Logging
```rust
// Structured logging for monitoring
info!(
    user = username,
    tweet_id = tweet.id,
    action = "downloaded",
    "Tweet downloaded successfully"
);

error!(
    user = username,
    error = %e,
    retry_count = retries,
    "Failed to fetch timeline"
);
```

## Testing Strategy

### Unit Tests
- Cache state detection
- Rate limiter logic
- Error classification (transient vs permanent)

### Integration Tests with Mocking

Use the `faux` crate for creating test doubles:

```rust
use faux::create;

#[create]
pub struct TwitterClient {
    // ...
}

#[cfg_attr(test, faux::methods)]
impl TwitterClient {
    pub async fn get_user_timeline(&self, username: &str, max_results: Option<u32>) -> Result<Vec<Tweet>> {
        // Real implementation
    }
}

#[tokio::test]
async fn test_daemon_recovery() {
    // Setup test cache with existing data
    let temp_dir = setup_test_cache().await;
    
    // Create mock Twitter client
    let mut mock_client = TwitterClient::faux();
    faux::when!(mock_client.get_user_timeline).then_return(Ok(vec![test_tweet()]));
    
    // Start daemon with mock
    let state = init_daemon_with_mocks(test_config(&temp_dir), mock_client).await.unwrap();
    
    // Verify it doesn't re-download existing tweets
    assert!(!will_download_tweet(&state, "existing_tweet_id"));
    
    // Verify it continues from where it left off
    assert!(will_download_tweet(&state, "new_tweet_id"));
}
```

### Mocking Nostr Client
```rust
#[cfg_attr(test, faux::create)]
pub struct NostrClient {
    // ...
}

#[cfg_attr(test, faux::methods)]
impl NostrClient {
    pub async fn send_event(&self, event: Event) -> Result<EventId> {
        // Real implementation
    }
}

#[tokio::test]
async fn test_nostr_posting() {
    let mut mock_nostr = NostrClient::faux();
    
    // Setup expectation
    faux::when!(mock_nostr.send_event).then_return(Ok(EventId::from_hex("abc123")?));
    
    // Test posting logic
    let result = post_tweet_to_nostr(&tweet, &mock_nostr).await;
    assert!(result.is_ok());
    
    // Verify the mock was called
    faux::when!(mock_nostr.send_event).times(1);
}
```

### End-to-End Tests
- Mock Twitter API responses using `faux`
- Mock Nostr relay connections using `faux`
- Verify complete flow from fetch to post

### Test Dependencies
```toml
[dev-dependencies]
faux = "0.1"
```

## Implementation Status

### ✅ Phase 1 & 2: Consolidated Daemon - COMPLETED
The daemon now includes all features from both phases in a single implementation:
- ✅ Multiple user monitoring with round-robin processing
- ✅ Advanced polling loop with per-user state
- ✅ Exponential backoff error handling per user
- ✅ Cache-based state architecture
- ✅ Graceful shutdown (Ctrl+C)
- ✅ Per-user rate limiting with sliding window
- ✅ Concurrent processing control (configurable)
- ✅ Statistics reporting (every 60 seconds)
- ✅ Smart polling intervals based on user state
- ✅ Command: `nostrweet daemon`

### 🚧 Phase 3: Advanced Features - IN PROGRESS
- ⬜ Configuration file support
- ⬜ Health check endpoint (HTTP/metrics)
- ⬜ Prometheus metrics export
- ✅ Graceful shutdown with state preservation
- ✅ Resume from last position after restart using filesystem-based approach with `since_id`
- ⬜ Web dashboard for monitoring

### 🔮 Phase 4: Production Hardening - FUTURE
- ⬜ Distributed locking for multi-instance
- ⬜ Database for state (PostgreSQL/SQLite)
- ⬜ Admin API for management
- ⬜ Alerting integration (PagerDuty/Slack)
- ⬜ Docker container with health checks
- ⬜ Kubernetes deployment manifests

## Security Considerations

1. **Private Key Storage**: Never log or expose private keys
2. **API Token Security**: Mask tokens in logs
3. **File Permissions**: Ensure cache directory has appropriate permissions
4. **Input Validation**: Validate all usernames and URLs
5. **Rate Limit Respect**: Never bypass API rate limits

## Performance Optimizations

1. **Batch Operations**: Process multiple tweets in parallel where possible
2. **Connection Pooling**: Reuse HTTP and WebSocket connections
3. **Incremental Updates**: Only fetch new tweets since last check
4. **Media Caching**: Skip re-downloading existing media
5. **Async I/O**: Use tokio for all I/O operations

## Future Enhancements

1. **Web Dashboard**: Show daemon status and statistics
2. **Webhook Support**: Trigger actions on new tweets
3. **Filter Rules**: Only post tweets matching certain criteria
4. **Thread Support**: Properly handle Twitter threads
5. **Quote Tweet Handling**: Special formatting for quotes
6. **Media Optimization**: Compress/resize media before posting
7. **Multi-Account Support**: Post from different Nostr accounts per Twitter user