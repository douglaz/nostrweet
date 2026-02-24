use ::time::{OffsetDateTime, format_description};
use anyhow::{Context, Result, bail, ensure};
use backoff::ExponentialBackoff;
use backoff::future::retry;
use clap::{Args, Parser, Subcommand};
use dotenvy::dotenv;
use nostr_sdk::nips::nip65::RelayMetadata;
use nostr_sdk::{
    Alphabet, Client as NostrClient, EventBuilder, Filter, FromBech32, JsonUtil, Keys, Kind,
    Metadata, SingleLetterTag, Tag, Timestamp, ToBech32,
};
use nostrweet_blossom::BlossomClient;
use nostrweet_core::{BlossomPort, TwitterPort};
use nostrweet_core::{
    HttpUrl, Media, MediaAsset, MediaKind, MediaVariant, MnemonicPhrase, NostrEventDraft,
    NostrEventId, NostrEventInfo, NostrEventResult, NostrPubkey, NostrTag, StoragePort, Tweet,
    TweetId, UnixTimestamp, User, UserId, UserTweetsQuery, Username, decode_html_entities,
    derive_nostr_secret_key, expand_urls_in_text, extract_media_urls,
};
use nostrweet_storage::FileStorage;
use nostrweet_twitter::{TwitterAdapterError, TwitterClient};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, oneshot};
use tokio::time;
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

trait MediaFetcher {
    async fn fetch_media_assets(&self, data_dir: &Path, tweet: &Tweet) -> Result<Vec<MediaAsset>>;
}

struct DefaultMediaFetcher;

impl MediaFetcher for DefaultMediaFetcher {
    async fn fetch_media_assets(&self, data_dir: &Path, tweet: &Tweet) -> Result<Vec<MediaAsset>> {
        fetch_media_assets(data_dir, tweet).await
    }
}

trait NostrAdapter {
    async fn publish_event(&self, draft: &NostrEventDraft, keys: &Keys)
    -> Result<NostrEventResult>;
    async fn find_event_by_tweet(
        &self,
        tweet_id: &TweetId,
        keys: &Keys,
    ) -> Result<Option<nostr_sdk::Event>>;
    async fn profile_exists(&self, pubkey: &nostr_sdk::PublicKey) -> Result<bool>;
    async fn publish_profile(&self, metadata: Metadata, keys: &Keys) -> Result<nostr_sdk::EventId>;
    async fn update_relay_list(&self, relays: &[String], keys: &Keys) -> Result<()>;
}

struct NostrSdkAdapter {
    client: NostrClient,
    data_dir: PathBuf,
}

impl NostrSdkAdapter {
    async fn new(keys: &Keys, relays: &[String], data_dir: &Path) -> Result<Self> {
        let client = build_nostr_client(keys, relays).await?;
        Ok(Self {
            client,
            data_dir: data_dir.to_path_buf(),
        })
    }
}

impl NostrAdapter for NostrSdkAdapter {
    async fn publish_event(
        &self,
        draft: &NostrEventDraft,
        keys: &Keys,
    ) -> Result<NostrEventResult> {
        let event = build_event_from_draft(draft, keys).await?;
        save_nostr_event_json(&self.data_dir, &event)?;
        let output = self
            .client
            .send_event(&event)
            .await
            .context("Failed to publish Nostr event")?;
        Ok(NostrEventResult {
            event_id: NostrEventId::parse(&output.val.to_hex())?,
            event_json: Some(
                serde_json::to_string_pretty(&event).context("Failed to serialize Nostr event")?,
            ),
        })
    }

    async fn find_event_by_tweet(
        &self,
        tweet_id: &TweetId,
        keys: &Keys,
    ) -> Result<Option<nostr_sdk::Event>> {
        find_existing_event(&self.client, tweet_id, keys).await
    }

    async fn profile_exists(&self, pubkey: &nostr_sdk::PublicKey) -> Result<bool> {
        let filter = Filter::new().author(*pubkey).kind(Kind::Metadata).limit(1);
        let events = self
            .client
            .fetch_events(filter, Duration::from_secs(10))
            .await?;
        Ok(!events.is_empty())
    }

    async fn publish_profile(&self, metadata: Metadata, keys: &Keys) -> Result<nostr_sdk::EventId> {
        let event = EventBuilder::metadata(&metadata)
            .sign(keys)
            .await
            .context("Failed to sign metadata event")?;
        save_nostr_event_json(&self.data_dir, &event)?;
        let output = self
            .client
            .send_event(&event)
            .await
            .context("Failed to publish profile event")?;
        Ok(*output.id())
    }

    async fn update_relay_list(&self, relays: &[String], keys: &Keys) -> Result<()> {
        let relay_list: Vec<(nostr_sdk::RelayUrl, Option<RelayMetadata>)> = relays
            .iter()
            .filter_map(|relay| match nostr_sdk::RelayUrl::parse(relay) {
                Ok(url) => Some((url, None)),
                Err(_) => None,
            })
            .collect();
        if relay_list.is_empty() && !relays.is_empty() {
            bail!("No valid relay URLs provided");
        }

        let event = EventBuilder::relay_list(relay_list)
            .sign(keys)
            .await
            .context("Failed to sign relay list event")?;
        let _ = self
            .client
            .send_event(&event)
            .await
            .context("Failed to publish relay list event")?;
        Ok(())
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "nostrweet",
    author = "Tweet Downloader",
    version,
    about = "Download tweets and their media",
    long_about = "A CLI tool for downloading tweets and all associated media"
)]
struct Cli {
    /// Directory to save all data (tweets, media, profiles, etc.)
    #[arg(
        short = 'o',
        long = "data-dir",
        env = "NOSTRWEET_DATA_DIR",
        global = true
    )]
    data_dir: Option<PathBuf>,

    /// Twitter API bearer token for authentication
    #[arg(long, env = "TWITTER_BEARER_TOKEN", global = true)]
    bearer_token: Option<String>,

    /// BIP39 mnemonic phrase for deriving Nostr keys
    #[arg(short = 'm', long, env = "NOSTRWEET_MNEMONIC", global = true)]
    mnemonic: Option<String>,

    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
enum Commands {
    /// Fetch a user's profile from Twitter
    FetchProfile {
        /// Twitter username (with or without @ symbol)
        #[arg(required = true)]
        username: String,
    },

    /// Fetch a tweet and its media
    FetchTweet {
        /// URL or ID of the tweet to download
        #[arg(required = true)]
        tweet_url_or_id: String,

        /// Skip downloading profiles for referenced users
        #[arg(long, default_value = "false")]
        skip_profiles: bool,
    },

    /// Fetch recent tweets from a user's timeline
    UserTweets {
        /// Twitter username (with or without @ symbol)
        #[arg(required = true)]
        username: String,

        /// Maximum number of tweets to fetch (default: 10)
        #[arg(short = 'c', long, default_value = "10")]
        count: u32,

        /// Only fetch tweets from the last N days
        #[arg(long)]
        days: Option<u32>,

        /// Skip downloading profiles for referenced users
        #[arg(long, default_value = "false")]
        skip_profiles: bool,
    },

    /// List all downloaded tweets in the cache
    ListTweets,

    /// Clear the tweet cache (removes all downloaded tweets and media)
    ClearCache {
        /// Confirm deletion without prompting
        #[arg(short, long)]
        force: bool,
    },

    /// Post a tweet to Nostr relays
    PostTweetToNostr {
        /// URL or ID of the tweet to post to Nostr
        #[arg(required = true)]
        tweet_url_or_id: String,

        /// Nostr relay addresses to post to (comma-separated)
        #[arg(
            short = 'r',
            long,
            required = true,
            value_delimiter = ',',
            env = "NOSTRWEET_RELAYS"
        )]
        relays: Vec<String>,

        /// Blossom server addresses for media uploads (comma-separated)
        #[arg(long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
        blossom_servers: Vec<String>,

        /// Force overwrite of existing Nostr event
        #[arg(short, long)]
        force: bool,

        /// Skip posting profiles for referenced users
        #[arg(long, default_value = "false")]
        skip_profiles: bool,
    },

    /// Post all cached tweets for a user to Nostr relays
    PostUserToNostr {
        /// Twitter username (with or without @ symbol)
        #[arg(required = true)]
        username: String,

        /// Nostr relay addresses to post to (comma-separated)
        #[arg(
            short = 'r',
            long,
            required = true,
            value_delimiter = ',',
            env = "NOSTRWEET_RELAYS"
        )]
        relays: Vec<String>,

        /// Blossom server addresses for media uploads (comma-separated)
        #[arg(long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
        blossom_servers: Vec<String>,

        /// Force overwrite of existing Nostr events
        #[arg(short, long)]
        force: bool,

        /// Skip posting profiles for referenced users
        #[arg(long, default_value = "false")]
        skip_profiles: bool,
    },

    /// Post a single tweet to Nostr relays
    PostTweet {
        /// URL or ID of the tweet to post to Nostr
        #[arg(required = true)]
        tweet_url_or_id: String,

        /// Nostr relay addresses to post to (comma-separated)
        #[arg(
            short = 'r',
            long,
            required = true,
            value_delimiter = ',',
            env = "NOSTRWEET_RELAYS"
        )]
        relays: Vec<String>,

        /// Blossom server addresses for media uploads (comma-separated)
        #[arg(long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
        blossom_servers: Vec<String>,

        /// Force overwrite of existing Nostr event
        #[arg(short, long)]
        force: bool,

        /// Skip posting profiles for referenced users
        #[arg(long, default_value = "false")]
        skip_profiles: bool,
    },

    /// Post a user's latest cached profile to Nostr
    PostProfileToNostr {
        /// The Twitter username of the user to post.
        #[arg(required = true)]
        username: String,

        /// Nostr relay addresses to post to (comma-separated)
        #[arg(
            short = 'r',
            long = "relay",
            required = true,
            value_delimiter = ',',
            env = "NOSTRWEET_RELAYS"
        )]
        relays: Vec<String>,
    },

    /// Update the relay list on Nostr
    UpdateRelayList {
        /// Nostr relay addresses to post to (comma-separated)
        #[arg(
            short = 'r',
            long,
            required = true,
            value_delimiter = ',',
            env = "NOSTRWEET_RELAYS"
        )]
        relays: Vec<String>,
    },

    /// Show a tweet's JSON and its Nostr event representation
    ShowTweet(ShowTweetCommand),

    /// Run in daemon mode to continuously monitor and post tweets
    Daemon {
        /// Twitter usernames to monitor
        #[arg(short, long = "user", required = true, action = clap::ArgAction::Append)]
        users: Vec<String>,

        /// Nostr relay addresses to post to
        #[arg(short, long = "relay", required = true, action = clap::ArgAction::Append)]
        relays: Vec<String>,

        /// Blossom server addresses for media uploads
        #[arg(long = "blossom-server", action = clap::ArgAction::Append)]
        blossom_servers: Vec<String>,

        /// Seconds between polling cycles
        #[arg(short, long, default_value = "300")]
        poll_interval: u64,
    },

    /// Utility commands for Nostr operations
    Utils {
        #[command(subcommand)]
        command: UtilsCommands,
    },
}

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
enum UtilsCommands {
    /// Query events from Nostr relays
    QueryEvents {
        /// Nostr relay addresses to query from
        #[arg(short, long = "relay", required = true, action = clap::ArgAction::Append)]
        relays: Vec<String>,

        /// Filter by event kind (e.g., 0 for metadata, 1 for text notes)
        #[arg(short = 'k', long)]
        kind: Option<u32>,

        /// Filter by author public key (hex or npub format)
        #[arg(short = 'a', long)]
        author: Option<String>,

        /// Maximum number of events to retrieve
        #[arg(short = 'l', long, default_value = "10")]
        limit: usize,

        /// Filter events newer than this Unix timestamp
        #[arg(long)]
        since: Option<u64>,

        /// Filter events older than this Unix timestamp
        #[arg(long)]
        until: Option<u64>,

        /// Output format (json or pretty)
        #[arg(short = 'f', long, default_value = "pretty")]
        format: String,

        /// Save output to file
        #[arg(long)]
        output: Option<String>,
    },
}

#[derive(Args, Debug)]
struct ShowTweetCommand {
    /// Tweet ID or URL to show
    #[arg(value_name = "TWEET_ID_OR_URL")]
    tweet: String,

    /// Show pretty-printed JSON (default: true)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pretty: bool,

    /// Show compact JSON (opposite of --pretty)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    compact: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    init_logging();
    let cli = Cli::parse();
    if cli.verbose {
        debug!("Verbose mode enabled");
    }
    run(cli).await
}

fn init_logging() {
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(filter)
        .init();
}

async fn run(cli: Cli) -> Result<()> {
    let data_dir = resolve_data_dir(&cli)?;
    let bearer_token = require_bearer_token(&cli, command_needs_bearer_token(&cli.command))?;
    let mnemonic = require_mnemonic(&cli, command_needs_mnemonic(&cli.command))?;
    let _ = cli.verbose;

    match cli.command {
        Commands::ListTweets => list_tweets(&data_dir).await,
        Commands::ClearCache { force } => clear_cache(&data_dir, force),
        Commands::FetchProfile { username } => {
            fetch_profile(&data_dir, bearer_token.as_deref().unwrap(), &username).await
        }
        Commands::FetchTweet {
            tweet_url_or_id,
            skip_profiles,
        } => {
            fetch_tweet(
                &data_dir,
                bearer_token.as_deref().unwrap(),
                &tweet_url_or_id,
                skip_profiles,
            )
            .await
        }
        Commands::UserTweets {
            username,
            count,
            days,
            skip_profiles,
        } => {
            user_tweets(
                &data_dir,
                bearer_token.as_deref().unwrap(),
                &username,
                count,
                days,
                skip_profiles,
            )
            .await
        }
        Commands::PostTweetToNostr {
            tweet_url_or_id,
            relays,
            blossom_servers,
            force,
            skip_profiles,
        } => {
            post_tweet_to_nostr(
                &data_dir,
                bearer_token.as_deref(),
                mnemonic.as_deref().unwrap(),
                &tweet_url_or_id,
                &relays,
                &blossom_servers,
                force,
                skip_profiles,
            )
            .await
        }
        Commands::PostUserToNostr {
            username,
            relays,
            blossom_servers,
            force,
            skip_profiles,
        } => {
            post_user_to_nostr(
                &data_dir,
                mnemonic.as_deref().unwrap(),
                &username,
                &relays,
                &blossom_servers,
                force,
                skip_profiles,
            )
            .await
        }
        Commands::PostTweet {
            tweet_url_or_id,
            relays,
            blossom_servers,
            force,
            skip_profiles,
        } => {
            post_tweet_to_nostr(
                &data_dir,
                bearer_token.as_deref(),
                mnemonic.as_deref().unwrap(),
                &tweet_url_or_id,
                &relays,
                &blossom_servers,
                force,
                skip_profiles,
            )
            .await
        }
        Commands::PostProfileToNostr { username, relays } => {
            post_profile_to_nostr(&data_dir, mnemonic.as_deref().unwrap(), &username, &relays).await
        }
        Commands::UpdateRelayList { relays } => {
            update_relay_list(mnemonic.as_deref().unwrap(), &relays).await
        }
        Commands::ShowTweet(cmd) => {
            show_tweet(&data_dir, bearer_token.as_deref(), mnemonic.as_deref(), cmd).await
        }
        Commands::Daemon {
            users,
            relays,
            blossom_servers,
            poll_interval,
        } => {
            daemon(
                &data_dir,
                bearer_token.as_deref().unwrap(),
                mnemonic.as_deref().unwrap(),
                &users,
                &relays,
                &blossom_servers,
                poll_interval,
            )
            .await
        }
        Commands::Utils { command } => match command {
            UtilsCommands::QueryEvents {
                relays,
                kind,
                author,
                limit,
                since,
                until,
                format,
                output,
            } => {
                utils_query_events(relays, kind, author, limit, since, until, format, output).await
            }
        },
    }
}

fn resolve_data_dir(cli: &Cli) -> Result<PathBuf> {
    let fallback = std::env::var("NOSTRWEET_OUTPUT_DIR")
        .ok()
        .map(PathBuf::from);
    resolve_data_dir_with_fallback(cli, fallback)
}

fn resolve_data_dir_with_fallback(cli: &Cli, fallback: Option<PathBuf>) -> Result<PathBuf> {
    let data_dir = cli.data_dir.clone().or(fallback).context(
        "Data directory not specified. Please set --data-dir or NOSTRWEET_DATA_DIR environment variable",
    )?;
    ensure_data_dir_exists(&data_dir)?;
    Ok(data_dir)
}

fn ensure_data_dir_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path).context("Failed to create data directory")?;
    }
    Ok(())
}

fn command_needs_bearer_token(command: &Commands) -> bool {
    matches!(
        command,
        Commands::FetchProfile { .. }
            | Commands::FetchTweet { .. }
            | Commands::UserTweets { .. }
            | Commands::Daemon { .. }
    )
}

fn command_needs_mnemonic(command: &Commands) -> bool {
    matches!(
        command,
        Commands::PostTweetToNostr { .. }
            | Commands::PostUserToNostr { .. }
            | Commands::PostTweet { .. }
            | Commands::PostProfileToNostr { .. }
            | Commands::UpdateRelayList { .. }
            | Commands::Daemon { .. }
    )
}

fn require_bearer_token(cli: &Cli, required: bool) -> Result<Option<String>> {
    if required {
        let token = cli.bearer_token.clone().context(
            "Twitter bearer token not specified. Please set --bearer-token or TWITTER_BEARER_TOKEN environment variable",
        )?;
        Ok(Some(token))
    } else {
        Ok(cli.bearer_token.clone())
    }
}

fn require_mnemonic(cli: &Cli, required: bool) -> Result<Option<String>> {
    if required {
        let mnemonic = cli.mnemonic.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Mnemonic not provided. Please use --mnemonic flag or NOSTRWEET_MNEMONIC environment variable."
            )
        })?;
        Ok(Some(mnemonic))
    } else {
        Ok(cli.mnemonic.clone())
    }
}

async fn list_tweets(data_dir: &Path) -> Result<()> {
    let storage = FileStorage::new(data_dir)?;
    let summaries = storage.list_tweets().await?;

    println!("Found {} tweets in {}", summaries.len(), data_dir.display());
    println!("{:-^80}", "");

    for summary in summaries {
        let tweet = summary.tweet;
        let author_display = if !tweet.author.username.as_str().is_empty() {
            match tweet.author.name.as_ref().filter(|name| !name.is_empty()) {
                Some(name) => format!(
                    "{name} (@{username})",
                    username = tweet.author.username.as_str()
                ),
                None => format!("@{username}", username = tweet.author.username.as_str()),
            }
        } else if let Some(author_id) = &tweet.author_id {
            format!("ID: {author_id}")
        } else {
            "Unknown".to_string()
        };

        let time_str = format_timestamp(summary.modified_at);
        let first_line = tweet.text.as_str().lines().next().unwrap_or("");

        println!("ID: {id}", id = tweet.id.as_str());
        println!("  │ Author: {author_display}");
        println!("Text: {first_line}");
        println!("Date: {time_str}");
        println!("File: {filename}", filename = summary.file_name);
        println!("{:-^80}", "");
    }

    Ok(())
}

fn format_timestamp(timestamp: UnixTimestamp) -> String {
    let Ok(raw) = i64::try_from(timestamp.value()) else {
        return "Unknown".to_string();
    };
    let Ok(parsed) = OffsetDateTime::from_unix_timestamp(raw) else {
        return "Unknown".to_string();
    };
    let Ok(format) = format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]")
    else {
        return "Unknown".to_string();
    };
    parsed
        .format(&format)
        .unwrap_or_else(|_| "Unknown".to_string())
}

fn clear_cache(data_dir: &Path, force: bool) -> Result<()> {
    if !force {
        print!(
            "Are you sure you want to delete all cached tweets and media from {path}? [y/N] ",
            path = data_dir.display()
        );
        io::stdout().flush().context("Failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("Failed to read user input")?;

        if !input.trim().eq_ignore_ascii_case("y") {
            return Ok(());
        }
    }

    let entries = std::fs::read_dir(data_dir).context("Failed to read output directory")?;
    let mut deleted_count = 0;

    for entry in entries {
        let entry = entry.context("Failed to read directory entry")?;
        let path = entry.path();
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            deleted_count += 1;
        }
    }

    let _ = deleted_count;
    Ok(())
}

fn build_twitter_status_url(tweet_id: &TweetId) -> String {
    format!("https://twitter.com/i/status/{}", tweet_id.as_str())
}

fn sanitize_filename(value: &str) -> String {
    value.replace(['\\', '/'], "_")
}

fn media_extension_from_content_type(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/jpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "video/mp4" => Some("mp4"),
        "video/quicktime" => Some("mov"),
        "video/webm" => Some("webm"),
        _ => None,
    }
}

fn extension_from_url(url: &HttpUrl) -> Option<String> {
    url.as_str()
        .split('?')
        .next()
        .and_then(|value| value.rsplit('.').next())
        .map(|ext| ext.to_lowercase())
}

fn content_type_from_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "webm" => Some("video/webm"),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct MediaDownload {
    url: HttpUrl,
    filename: String,
    content_type: String,
}

fn select_media_variant(media: &Media) -> Option<MediaVariant> {
    media
        .variants
        .iter()
        .filter_map(|variant| variant.bit_rate.map(|br| (br, variant)))
        .max_by_key(|(br, _)| *br)
        .map(|(_, variant)| variant.clone())
}

fn select_media_url(media: &Media) -> Option<(HttpUrl, Option<String>)> {
    if let Some(url) = &media.url {
        return Some((url.clone(), None));
    }

    if matches!(media.kind, MediaKind::Video | MediaKind::AnimatedGif) {
        if let Some(variant) = select_media_variant(media) {
            return Some((variant.url.clone(), Some(variant.content_type)));
        }
    }

    media.preview_image_url.clone().map(|url| (url, None))
}

fn media_downloads_for_tweet(tweet: &Tweet) -> Vec<MediaDownload> {
    let mut downloads = Vec::new();
    let mut seen = HashSet::new();
    collect_media_downloads(tweet, &mut downloads, &mut seen);
    downloads
}

fn collect_media_downloads(
    tweet: &Tweet,
    downloads: &mut Vec<MediaDownload>,
    seen: &mut HashSet<String>,
) {
    if let Some(includes) = &tweet.includes {
        for media in &includes.media {
            let key = media.media_key.as_str().to_string();
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some((url, variant_content_type)) = select_media_url(media) else {
                continue;
            };

            let media_key_clean = media.media_key.cleaned();
            let username = if tweet.author.username.as_str().is_empty() {
                "unknown"
            } else {
                tweet.author.username.as_str()
            };

            let extension = match media.kind {
                MediaKind::Photo => Some("jpg".to_string()),
                MediaKind::Video | MediaKind::AnimatedGif => Some("mp4".to_string()),
                MediaKind::Other(_) => extension_from_url(&url),
            }
            .or_else(|| {
                media_extension_from_content_type(&variant_content_type.clone().unwrap_or_default())
                    .map(|ext| ext.to_string())
            })
            .unwrap_or_else(|| "bin".to_string());

            let content_type = variant_content_type
                .or_else(|| content_type_from_extension(&extension).map(|v| v.to_string()))
                .unwrap_or_else(|| "application/octet-stream".to_string());

            let filename = format!(
                "{username}_{media_key}.{extension}",
                username = sanitize_filename(username),
                media_key = sanitize_filename(media_key_clean),
                extension = sanitize_filename(&extension)
            );

            downloads.push(MediaDownload {
                url,
                filename,
                content_type,
            });
        }
    }

    for reference in &tweet.referenced_tweets {
        if let Some(data) = &reference.data {
            collect_media_downloads(data, downloads, seen);
        }
    }
}

async fn fetch_media_assets(data_dir: &Path, tweet: &Tweet) -> Result<Vec<MediaAsset>> {
    let mut assets = Vec::new();
    for download in media_downloads_for_tweet(tweet) {
        let path = data_dir.join(&download.filename);
        let bytes = if path.exists() {
            std::fs::read(&path).with_context(|| {
                format!("Failed to read cached media file at {}", path.display())
            })?
        } else {
            let response = reqwest::get(download.url.as_str())
                .await
                .with_context(|| format!("Failed to download media {}", download.url.as_str()))?;
            let status = response.status();
            if !status.is_success() {
                anyhow::bail!(
                    "Media download failed with HTTP {status} for {}",
                    download.url.as_str()
                );
            }
            let bytes = response
                .bytes()
                .await
                .context("Failed to read media response bytes")?
                .to_vec();
            std::fs::write(&path, &bytes)
                .with_context(|| format!("Failed to write media file to {}", path.display()))?;
            bytes
        };

        assets.push(MediaAsset {
            name: download.filename,
            content_type: download.content_type,
            bytes,
        });
    }
    Ok(assets)
}

async fn build_blossom_client(servers: &[String]) -> Result<Option<BlossomClient>> {
    if servers.is_empty() {
        return Ok(None);
    }
    let mut parsed = Vec::new();
    for server in servers {
        parsed.push(
            nostrweet_core::BlossomUrl::parse(server)
                .with_context(|| format!("Invalid Blossom server URL: {server}"))?,
        );
    }
    Ok(Some(BlossomClient::new(parsed)?))
}

fn parse_username(input: &str) -> Result<Username> {
    Username::parse(input).with_context(|| format!("Invalid username: {input}"))
}

fn derive_keys_for_user(user_id: &UserId, mnemonic: &MnemonicPhrase) -> Result<Keys> {
    let secret = derive_nostr_secret_key(user_id, mnemonic, None)?;
    Keys::parse(&secret.to_hex()).context("Failed to derive Nostr keys")
}

async fn fetch_profile(data_dir: &Path, bearer_token: &str, username: &str) -> Result<()> {
    let username = parse_username(username)?;
    let twitter = TwitterClient::new(bearer_token)?;
    let storage = FileStorage::new(data_dir)?;
    let user = twitter.fetch_user_profile(&username).await?;
    storage.save_user_profile(&user).await?;
    Ok(())
}

async fn load_or_fetch_tweet(
    data_dir: &Path,
    bearer_token: Option<&str>,
    tweet_id: &TweetId,
) -> Result<Tweet> {
    let storage = FileStorage::new(data_dir)?;
    let twitter = if let Some(token) = bearer_token {
        Some(TwitterClient::new(token)?)
    } else {
        None
    };
    load_or_fetch_tweet_with_ports(&storage, twitter.as_ref(), tweet_id).await
}

async fn load_or_fetch_tweet_with_ports<S: StoragePort, T: TwitterPort>(
    storage: &S,
    twitter: Option<&T>,
    tweet_id: &TweetId,
) -> Result<Tweet> {
    if storage.is_tweet_not_found(tweet_id).await? {
        bail!(
            "Tweet {} was previously marked as not found",
            tweet_id.as_str()
        );
    }

    if let Some(tweet) = storage.load_tweet(tweet_id).await? {
        return Ok(tweet);
    }

    let twitter = twitter.context("Twitter bearer token required to fetch tweet from API")?;
    match twitter.fetch_tweet(tweet_id).await {
        Ok(tweet) => {
            storage.save_tweet(&tweet).await?;
            Ok(tweet)
        }
        Err(err) => {
            if let Some(TwitterAdapterError::TweetNotFound { .. }) =
                err.downcast_ref::<TwitterAdapterError>()
            {
                storage.mark_tweet_not_found(tweet_id).await?;
            }
            Err(err)
        }
    }
}

async fn fetch_tweet(
    data_dir: &Path,
    bearer_token: &str,
    tweet_url_or_id: &str,
    skip_profiles: bool,
) -> Result<()> {
    let tweet_id = TweetId::parse(tweet_url_or_id)
        .with_context(|| format!("Failed to parse tweet ID from {tweet_url_or_id}"))?;
    let twitter = TwitterClient::new(bearer_token)?;
    let storage = FileStorage::new(data_dir)?;

    let mut tweet = load_or_fetch_tweet(data_dir, Some(bearer_token), &tweet_id).await?;
    twitter.enrich_referenced_tweets(&mut tweet).await?;
    storage.save_tweet(&tweet).await?;
    let _ = fetch_media_assets(data_dir, &tweet).await?;

    if !skip_profiles {
        download_profiles_for_tweet(&twitter, &storage, &tweet).await?;
    }

    Ok(())
}

async fn user_tweets(
    data_dir: &Path,
    bearer_token: &str,
    username: &str,
    count: u32,
    days: Option<u32>,
    skip_profiles: bool,
) -> Result<()> {
    let username = parse_username(username)?;
    let twitter = TwitterClient::new(bearer_token)?;
    let storage = FileStorage::new(data_dir)?;
    let query = UserTweetsQuery {
        count,
        days,
        since_id: None,
    };
    let mut tweets = twitter.fetch_user_tweets(&username, query).await?;
    let mut processed = Vec::new();
    for tweet in &mut tweets {
        twitter.enrich_referenced_tweets(tweet).await?;
        storage.save_tweet(tweet).await?;
        let _ = fetch_media_assets(data_dir, tweet).await?;
        processed.push(tweet.clone());
    }

    if !skip_profiles {
        download_profiles_for_tweets(&twitter, &storage, &processed).await?;
    }

    Ok(())
}

async fn download_profiles_for_tweets(
    twitter: &TwitterClient,
    storage: &FileStorage<nostrweet_storage::SystemClock>,
    tweets: &[Tweet],
) -> Result<()> {
    let mut usernames = HashSet::new();
    for tweet in tweets {
        usernames.extend(collect_usernames_from_tweet(tweet));
    }
    download_profiles_for_usernames(twitter, storage, usernames).await
}

async fn download_profiles_for_tweet(
    twitter: &TwitterClient,
    storage: &FileStorage<nostrweet_storage::SystemClock>,
    tweet: &Tweet,
) -> Result<()> {
    let usernames = collect_usernames_from_tweet(tweet);
    download_profiles_for_usernames(twitter, storage, usernames).await
}

async fn download_profiles_for_usernames(
    twitter: &TwitterClient,
    storage: &FileStorage<nostrweet_storage::SystemClock>,
    usernames: HashSet<String>,
) -> Result<()> {
    for username in usernames {
        let Ok(username) = Username::parse(&username) else {
            continue;
        };
        if let Ok(user) = twitter.fetch_user_profile(&username).await {
            let _ = storage.save_user_profile(&user).await?;
        }
    }
    Ok(())
}

fn collect_usernames_from_tweet(tweet: &Tweet) -> HashSet<String> {
    let mut usernames = HashSet::new();
    if !tweet.author.username.as_str().is_empty() {
        usernames.insert(tweet.author.username.as_str().to_string());
    }
    if let Some(entities) = &tweet.entities {
        for mention in &entities.mentions {
            usernames.insert(mention.username.as_str().to_string());
        }
    }
    for reference in &tweet.referenced_tweets {
        if let Some(data) = &reference.data {
            if !data.author.username.as_str().is_empty() {
                usernames.insert(data.author.username.as_str().to_string());
            }
            if let Some(entities) = &data.entities {
                for mention in &entities.mentions {
                    usernames.insert(mention.username.as_str().to_string());
                }
            }
        }
    }
    usernames
}

#[derive(Debug)]
struct FormattedContent {
    text: String,
    used_media_urls: Vec<HttpUrl>,
    mentioned_pubkeys: Vec<nostr_sdk::PublicKey>,
}

#[derive(Debug)]
struct NostrLinkResolver {
    username_to_pubkey: HashMap<String, nostr_sdk::PublicKey>,
    user_id_to_pubkey: HashMap<String, nostr_sdk::PublicKey>,
    data_dir: Option<PathBuf>,
    mnemonic: Option<MnemonicPhrase>,
}

impl NostrLinkResolver {
    fn new(data_dir: Option<PathBuf>, mnemonic: Option<MnemonicPhrase>) -> Self {
        Self {
            username_to_pubkey: HashMap::new(),
            user_id_to_pubkey: HashMap::new(),
            data_dir,
            mnemonic,
        }
    }

    fn add_known_user(&mut self, username: &Username, user_id: &UserId) -> Result<()> {
        let Some(mnemonic) = &self.mnemonic else {
            return Ok(());
        };
        if self.user_id_to_pubkey.contains_key(user_id.as_str()) {
            if let Some(pubkey) = self.user_id_to_pubkey.get(user_id.as_str()) {
                self.username_to_pubkey
                    .insert(username.as_str().to_string(), *pubkey);
            }
            return Ok(());
        }

        let keys = derive_keys_for_user(user_id, mnemonic)?;
        let pubkey = keys.public_key();
        self.user_id_to_pubkey
            .insert(user_id.as_str().to_string(), pubkey);
        self.username_to_pubkey
            .insert(username.as_str().to_string(), pubkey);
        Ok(())
    }

    async fn resolve_username(&mut self, username: &str) -> Result<Option<nostr_sdk::PublicKey>> {
        if let Some(pubkey) = self.username_to_pubkey.get(username) {
            return Ok(Some(*pubkey));
        }
        let Some(mnemonic) = &self.mnemonic else {
            return Ok(None);
        };
        let Some(data_dir) = &self.data_dir else {
            return Ok(None);
        };

        let storage = FileStorage::new(data_dir)?;
        let Ok(username_parsed) = Username::parse(username) else {
            return Ok(None);
        };
        let user = storage.load_latest_user_profile(&username_parsed).await?;
        let Some(user) = user else {
            return Ok(None);
        };
        let keys = derive_keys_for_user(&user.id, mnemonic)?;
        let pubkey = keys.public_key();
        self.user_id_to_pubkey
            .insert(user.id.as_str().to_string(), pubkey);
        self.username_to_pubkey.insert(username.to_string(), pubkey);
        Ok(Some(pubkey))
    }
}

fn extract_additional_mentions(text: &str, already_processed: &HashSet<String>) -> Vec<String> {
    let mut mentions = Vec::new();
    for word in text.split_whitespace() {
        if !word.starts_with('@') || word.len() <= 1 {
            continue;
        }
        let username = word[1..].trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
        if username.is_empty() || username.len() > 15 {
            continue;
        }
        if already_processed.contains(username) {
            continue;
        }
        mentions.push(username.to_string());
    }
    mentions
}

fn pubkey_to_bech32(pubkey: &nostr_sdk::PublicKey) -> String {
    match pubkey.to_bech32() {
        Ok(npub) => npub,
        Err(err) => match err {},
    }
}

async fn process_mentions_in_text(
    text: &str,
    entities: Option<&nostrweet_core::Entities>,
    resolver: &mut NostrLinkResolver,
) -> Result<(String, Vec<nostr_sdk::PublicKey>)> {
    let mut result = text.to_string();
    let mut mentioned_pubkeys = Vec::new();
    let mut processed = HashSet::new();

    if let Some(entities) = entities {
        for mention in &entities.mentions {
            let username = mention.username.as_str();
            if processed.contains(username) {
                continue;
            }
            processed.insert(username.to_string());
            if let Some(pubkey) = resolver.resolve_username(username).await? {
                let npub = pubkey_to_bech32(&pubkey);
                let old = format!("@{username}");
                let new = format!("nostr:{npub}");
                result = result.replace(&old, &new);
                mentioned_pubkeys.push(pubkey);
            }
        }
    }

    for username in extract_additional_mentions(&result, &processed) {
        if let Some(pubkey) = resolver.resolve_username(&username).await? {
            let npub = pubkey_to_bech32(&pubkey);
            let old = format!("@{username}");
            let new = format!("nostr:{npub}");
            result = result.replace(&old, &new);
            mentioned_pubkeys.push(pubkey);
        }
    }

    Ok((result, mentioned_pubkeys))
}

async fn format_tweet_text_with_mentions(
    tweet: &Tweet,
    media_urls: &[HttpUrl],
    resolver: &mut NostrLinkResolver,
) -> Result<FormattedContent> {
    resolver.add_known_user(&tweet.author.username, &tweet.author.id)?;
    let raw_text = tweet
        .note_tweet
        .as_ref()
        .map(|note| note.text.as_str())
        .unwrap_or_else(|| tweet.text.as_str());
    let decoded = decode_html_entities(raw_text);
    let expanded = expand_urls_in_text(&decoded, tweet.entities.as_ref(), media_urls, tweet);
    let (text_with_mentions, mentioned_pubkeys) =
        process_mentions_in_text(&expanded.text, tweet.entities.as_ref(), resolver).await?;
    Ok(FormattedContent {
        text: text_with_mentions,
        used_media_urls: expanded.used_media_urls,
        mentioned_pubkeys,
    })
}

fn is_simple_retweet(tweet: &Tweet) -> (bool, Option<String>) {
    if !tweet
        .referenced_tweets
        .iter()
        .any(|rt| matches!(rt.kind, nostrweet_core::ReferenceKind::Retweeted))
    {
        return (false, None);
    }
    let raw_text = tweet
        .note_tweet
        .as_ref()
        .map(|note| note.text.as_str())
        .unwrap_or_else(|| tweet.text.as_str());
    let is_simple = raw_text.starts_with("RT @")
        && raw_text.contains(':')
        && !raw_text.contains('\n')
        && !raw_text.contains(" // ");
    let username = if is_simple {
        raw_text.find(':').and_then(|end_idx| {
            raw_text
                .find('@')
                .map(|start_idx| raw_text[(start_idx + 1)..end_idx].trim().to_string())
        })
    } else {
        None
    };
    (is_simple, username)
}

async fn format_reply_section(
    content: &mut String,
    ref_tweet: &nostrweet_core::ReferencedTweet,
    resolver: &mut NostrLinkResolver,
) -> Result<Vec<nostr_sdk::PublicKey>> {
    let mut mentioned_pubkeys = Vec::new();
    let tweet_url = build_twitter_status_url(&ref_tweet.id);
    if let Some(data) = &ref_tweet.data {
        resolver.add_known_user(&data.author.username, &data.author.id)?;
        let author = if let Some(pubkey) = resolver
            .resolve_username(data.author.username.as_str())
            .await?
        {
            mentioned_pubkeys.push(pubkey);
            format!("nostr:{}", pubkey_to_bech32(&pubkey))
        } else {
            format!("@{}", data.author.username.as_str())
        };

        content.push_str(&format!("↩️ Reply to {author}:\n"));

        let media_urls = extract_media_urls(data);
        let formatted = format_tweet_text_with_mentions(data, &media_urls, resolver).await?;
        mentioned_pubkeys.extend(formatted.mentioned_pubkeys);
        content.push_str(&formatted.text);
        content.push('\n');

        for url in media_urls {
            if !formatted.used_media_urls.contains(&url) {
                content.push_str(&format!("{url}\n"));
            }
        }
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        content.push_str(&format!(
            "↩️ Reply to Tweet {id}\n{tweet_url}\n",
            id = ref_tweet.id
        ));
    }
    Ok(mentioned_pubkeys)
}

async fn format_quote_section(
    content: &mut String,
    ref_tweet: &nostrweet_core::ReferencedTweet,
    resolver: &mut NostrLinkResolver,
) -> Result<Vec<nostr_sdk::PublicKey>> {
    let mut mentioned_pubkeys = Vec::new();
    let tweet_url = build_twitter_status_url(&ref_tweet.id);
    if let Some(data) = &ref_tweet.data {
        resolver.add_known_user(&data.author.username, &data.author.id)?;
        let author = if let Some(pubkey) = resolver
            .resolve_username(data.author.username.as_str())
            .await?
        {
            mentioned_pubkeys.push(pubkey);
            format!("nostr:{}", pubkey_to_bech32(&pubkey))
        } else {
            format!("@{}", data.author.username.as_str())
        };

        content.push_str(&format!("💬 Quote of {author}:\n"));
        let media_urls = extract_media_urls(data);
        let formatted = format_tweet_text_with_mentions(data, &media_urls, resolver).await?;
        mentioned_pubkeys.extend(formatted.mentioned_pubkeys);
        content.push_str(&formatted.text);
        content.push('\n');

        for url in media_urls {
            if !formatted.used_media_urls.contains(&url) {
                content.push_str(&format!("{url}\n"));
            }
        }
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        content.push_str(&format!(
            "💬 Quote of Tweet {id}\n{tweet_url}\n",
            id = ref_tweet.id
        ));
    }
    Ok(mentioned_pubkeys)
}

async fn format_retweet_section(
    content: &mut String,
    ref_tweet: &nostrweet_core::ReferencedTweet,
    retweeter: &str,
    resolver: &mut NostrLinkResolver,
) -> Result<Vec<nostr_sdk::PublicKey>> {
    let mut mentioned_pubkeys = Vec::new();
    let tweet_url = build_twitter_status_url(&ref_tweet.id);
    if let Some(data) = &ref_tweet.data {
        resolver.add_known_user(&data.author.username, &data.author.id)?;
        let author = if let Some(pubkey) = resolver
            .resolve_username(data.author.username.as_str())
            .await?
        {
            mentioned_pubkeys.push(pubkey);
            format!("nostr:{}", pubkey_to_bech32(&pubkey))
        } else {
            format!("@{}", data.author.username.as_str())
        };

        content.push_str(&format!("🔁 @{retweeter} retweeted {author}:\n"));
        let media_urls = extract_media_urls(data);
        let formatted = format_tweet_text_with_mentions(data, &media_urls, resolver).await?;
        mentioned_pubkeys.extend(formatted.mentioned_pubkeys);
        content.push_str(&formatted.text);
        content.push('\n');
        for url in media_urls {
            if !formatted.used_media_urls.contains(&url) {
                content.push_str(&format!("{url}\n"));
            }
        }
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        content.push_str(&format!(
            "🔁 @{retweeter} retweeted Tweet {id}\n{tweet_url}\n",
            id = ref_tweet.id
        ));
    }
    Ok(mentioned_pubkeys)
}

async fn format_tweet_as_nostr_content_with_mentions(
    tweet: &Tweet,
    media_urls: &[HttpUrl],
    resolver: &mut NostrLinkResolver,
) -> Result<(String, Vec<nostr_sdk::PublicKey>)> {
    let mut content = String::new();
    let mut mentioned_pubkeys = Vec::new();
    let (is_simple_retweet, rt_username) = is_simple_retweet(tweet);

    if !is_simple_retweet {
        if !tweet.author.username.as_str().is_empty() {
            content.push_str(&format!("🐦 @{}: ", tweet.author.username.as_str()));
        } else if let Some(author_id) = tweet.author_id.as_ref() {
            content.push_str(&format!("🐦 User {}: ", author_id.as_str()));
        } else {
            content.push_str("🐦 Tweet: ");
        }

        let formatted = format_tweet_text_with_mentions(tweet, media_urls, resolver).await?;
        mentioned_pubkeys.extend(formatted.mentioned_pubkeys);
        content.push_str(&formatted.text);
        content.push('\n');

        for url in media_urls {
            if !formatted.used_media_urls.contains(url) {
                content.push_str(&format!("{url}\n"));
            }
        }
    }

    for reference in &tweet.referenced_tweets {
        match reference.kind {
            nostrweet_core::ReferenceKind::RepliedTo => {
                mentioned_pubkeys
                    .extend(format_reply_section(&mut content, reference, resolver).await?);
            }
            nostrweet_core::ReferenceKind::Quoted => {
                mentioned_pubkeys
                    .extend(format_quote_section(&mut content, reference, resolver).await?);
            }
            nostrweet_core::ReferenceKind::Retweeted => {
                let retweeter = rt_username
                    .as_deref()
                    .unwrap_or_else(|| tweet.author.username.as_str());
                mentioned_pubkeys.extend(
                    format_retweet_section(&mut content, reference, retweeter, resolver).await?,
                );
            }
            nostrweet_core::ReferenceKind::Unknown(_) => {}
        }
    }

    content.push('\n');
    content.push_str(&format!(
        "Original tweet: {}",
        build_twitter_status_url(&tweet.id)
    ));

    Ok((content, mentioned_pubkeys))
}

fn build_nostr_event_tags(
    tweet_id: &TweetId,
    original_media_urls: &[HttpUrl],
    blossom_urls: &[HttpUrl],
    mentioned_pubkeys: &[nostr_sdk::PublicKey],
) -> Result<Vec<NostrTag>> {
    let mut tags = Vec::new();
    let original_url = HttpUrl::parse(&build_twitter_status_url(tweet_id))?;
    tags.push(NostrTag::r(&original_url));

    for pubkey in mentioned_pubkeys {
        let nostr_pubkey = NostrPubkey::parse(&pubkey.to_hex())?;
        tags.push(NostrTag::p(&nostr_pubkey));
    }

    if blossom_urls.is_empty() {
        for url in original_media_urls {
            tags.push(NostrTag::media(url));
        }
    } else {
        for (source, media) in original_media_urls.iter().zip(blossom_urls.iter()) {
            tags.push(NostrTag::source(source));
            tags.push(NostrTag::media(media));
        }
    }

    tags.push(NostrTag::client());
    Ok(tags)
}

async fn build_event_from_draft(draft: &NostrEventDraft, keys: &Keys) -> Result<nostr_sdk::Event> {
    let mut builder = EventBuilder::new(Kind::TextNote, draft.content.clone());
    if let Some(created_at) = draft.created_at {
        builder = builder.custom_created_at(Timestamp::from(created_at.value()));
    }
    for tag in &draft.tags {
        let mut values = Vec::with_capacity(1 + tag.values.len());
        values.push(tag.name.clone());
        values.extend(tag.values.clone());
        builder = builder.tag(Tag::parse(values)?);
    }
    let event = builder
        .sign(keys)
        .await
        .context("Failed to sign Nostr event")?;
    Ok(event)
}

async fn build_nostr_client(keys: &Keys, relays: &[String]) -> Result<NostrClient> {
    let client = NostrClient::new(keys.clone());
    for relay in relays {
        client
            .add_relay(relay)
            .await
            .with_context(|| format!("Failed to add relay: {relay}"))?;
    }
    client.connect().await;
    Ok(client)
}

async fn find_existing_event(
    client: &NostrClient,
    tweet_id: &TweetId,
    keys: &Keys,
) -> Result<Option<nostr_sdk::Event>> {
    let pubkey = keys.public_key();
    let twitter_url = build_twitter_status_url(tweet_id);
    let filter = Filter::new()
        .author(pubkey)
        .kind(Kind::TextNote)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::R), twitter_url)
        .limit(10);

    match client.fetch_events(filter, Duration::from_secs(10)).await {
        Ok(events) => Ok(events.into_iter().next()),
        Err(err) => {
            warn!("Failed to check existing events for {tweet_id}: {err}");
            Ok(None)
        }
    }
}

fn save_nostr_event_json(data_dir: &Path, event: &nostr_sdk::Event) -> Result<()> {
    let dir = data_dir.join("nostr_events");
    std::fs::create_dir_all(&dir).with_context(|| {
        format!(
            "Failed to create nostr_events directory at {}",
            dir.display()
        )
    })?;
    let path = dir.join(format!("{}.json", event.id.to_hex()));
    let json = serde_json::to_string_pretty(event).context("Failed to serialize Nostr event")?;
    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write Nostr event to {}", path.display()))?;
    Ok(())
}

fn parse_relay_urls(relays: &[String]) -> Result<Vec<nostrweet_core::RelayUrl>> {
    relays
        .iter()
        .map(|relay| {
            nostrweet_core::RelayUrl::parse(relay)
                .with_context(|| format!("Invalid relay URL: {relay}"))
        })
        .collect()
}

fn profile_disclaimer(username: &Username) -> String {
    format!(
        "\n\nThis account is a mirror of https://x.com/{username}\n\nMirror created using nostrweet: https://github.com/douglaz/nostrweet",
        username = username.as_str()
    )
}

fn build_profile_metadata(user: &User, username: &Username) -> Metadata {
    let mut metadata = Metadata::new();

    if let Some(name) = &user.name {
        metadata = metadata.name(name);
    }

    let disclaimer = profile_disclaimer(username);
    let about = match &user.description {
        Some(desc) => format!("{desc}{disclaimer}"),
        None => disclaimer,
    };
    metadata = metadata.about(&about);

    if let Some(url) = &user.profile_image_url {
        if let Ok(parsed) = url.as_str().parse() {
            metadata = metadata.picture(parsed);
        }
    }
    if let Some(url) = &user.url {
        if let Ok(parsed) = url.as_str().parse() {
            metadata = metadata.website(parsed);
        }
    }

    metadata
}

async fn post_single_profile(
    username: &Username,
    adapter: &impl NostrAdapter,
    data_dir: &Path,
    mnemonic: &MnemonicPhrase,
) -> Result<nostr_sdk::EventId> {
    let storage = FileStorage::new(data_dir)?;
    let Some(user) = storage.load_latest_user_profile(username).await? else {
        bail!(
            "No profile found for user '@{username}'",
            username = username.as_str()
        );
    };

    let keys = derive_keys_for_user(&user.id, mnemonic)?;
    let metadata = build_profile_metadata(&user, username);
    adapter
        .publish_profile(metadata, &keys)
        .await
        .with_context(|| {
            format!(
                "Failed to publish profile for @{username}",
                username = username.as_str()
            )
        })
}

async fn post_relay_list_for_user(
    username: &Username,
    adapter: &impl NostrAdapter,
    data_dir: &Path,
    mnemonic: &MnemonicPhrase,
    relays: &[String],
) -> Result<()> {
    let storage = FileStorage::new(data_dir)?;
    let Some(user) = storage.load_latest_user_profile(username).await? else {
        bail!(
            "No profile found for user '@{username}'",
            username = username.as_str()
        );
    };

    let keys = derive_keys_for_user(&user.id, mnemonic)?;
    adapter.update_relay_list(relays, &keys).await?;
    Ok(())
}

async fn post_user_profile_with_relay_list(
    username: &Username,
    adapter: &impl NostrAdapter,
    data_dir: &Path,
    mnemonic: &MnemonicPhrase,
    relays: &[String],
) -> Result<()> {
    let event_id = post_single_profile(username, adapter, data_dir, mnemonic).await?;
    debug!(
        "Posted profile for @{username} with event ID {event_id}",
        username = username.as_str()
    );
    post_relay_list_for_user(username, adapter, data_dir, mnemonic, relays).await?;
    Ok(())
}

async fn check_profile_exists(
    username: &Username,
    adapter: &impl NostrAdapter,
    data_dir: &Path,
    mnemonic: &MnemonicPhrase,
) -> Result<bool> {
    let storage = FileStorage::new(data_dir)?;
    let Some(user) = storage.load_latest_user_profile(username).await? else {
        return Ok(false);
    };

    let keys = derive_keys_for_user(&user.id, mnemonic)?;
    adapter.profile_exists(&keys.public_key()).await
}

async fn filter_profiles_to_post(
    usernames: HashSet<String>,
    adapter: &impl NostrAdapter,
    data_dir: &Path,
    force: bool,
    mnemonic: &MnemonicPhrase,
) -> Result<HashSet<String>> {
    if force {
        return Ok(usernames);
    }

    let storage = FileStorage::new(data_dir)?;
    let mut profiles_to_post = HashSet::new();

    for username in usernames {
        let Ok(username_parsed) = Username::parse(&username) else {
            continue;
        };
        let Some(user) = storage.load_latest_user_profile(&username_parsed).await? else {
            continue;
        };

        let keys = derive_keys_for_user(&user.id, mnemonic)?;
        let has_existing_profile = adapter
            .profile_exists(&keys.public_key())
            .await
            .unwrap_or(false);

        if !has_existing_profile {
            profiles_to_post.insert(username);
        }
    }

    Ok(profiles_to_post)
}

async fn post_referenced_profiles(
    usernames: &HashSet<String>,
    adapter: &impl NostrAdapter,
    data_dir: &Path,
    mnemonic: &MnemonicPhrase,
) -> Result<usize> {
    if usernames.is_empty() {
        return Ok(0);
    }

    let mut posted_count = 0;
    let mut failed_count = 0;

    for username in usernames {
        let Ok(username_parsed) = Username::parse(username) else {
            continue;
        };
        match post_single_profile(&username_parsed, adapter, data_dir, mnemonic).await {
            Ok(_) => posted_count += 1,
            Err(err) => {
                debug!("Failed to post profile for @{username}: {err}");
                failed_count += 1;
            }
        }
    }

    if failed_count > 0 {
        debug!("Posted {posted_count} profiles, {failed_count} failed");
    }

    Ok(posted_count)
}

#[allow(clippy::too_many_arguments)]
async fn post_tweet_to_nostr(
    data_dir: &Path,
    bearer_token: Option<&str>,
    mnemonic: &str,
    tweet_url_or_id: &str,
    relays: &[String],
    blossom_servers: &[String],
    force: bool,
    skip_profiles: bool,
) -> Result<()> {
    let tweet_id = TweetId::parse(tweet_url_or_id)
        .with_context(|| format!("Failed to parse tweet ID from {tweet_url_or_id}"))?;
    let storage = FileStorage::new(data_dir)?;

    if let Some(existing) = storage.load_nostr_event_info(&tweet_id).await? {
        if !force {
            let _ = existing;
            return Ok(());
        }
    }

    let twitter = if let Some(token) = bearer_token {
        Some(TwitterClient::new(token)?)
    } else {
        None
    };
    let mut tweet = load_or_fetch_tweet_with_ports(&storage, twitter.as_ref(), &tweet_id).await?;
    if let Some(twitter) = twitter.as_ref() {
        twitter.enrich_referenced_tweets(&mut tweet).await?;
        if !skip_profiles {
            download_profiles_for_tweet(twitter, &storage, &tweet).await?;
        }
    }

    let original_media_urls = extract_media_urls(&tweet);
    let media_fetcher = DefaultMediaFetcher;
    let assets = media_fetcher.fetch_media_assets(data_dir, &tweet).await?;
    let blossom = build_blossom_client(blossom_servers).await?;
    let blossom_urls = if let Some(client) = blossom.as_ref() {
        client.upload_media(&assets).await?
    } else {
        Vec::new()
    };

    let content_media_urls = if blossom_urls.is_empty() {
        original_media_urls.clone()
    } else {
        blossom_urls.clone()
    };

    let mnemonic = MnemonicPhrase::parse(mnemonic)?;
    let keys = derive_keys_for_user(&tweet.author.id, &mnemonic)?;
    let adapter = NostrSdkAdapter::new(&keys, relays, data_dir).await?;

    let mut resolver = NostrLinkResolver::new(Some(data_dir.to_path_buf()), Some(mnemonic.clone()));
    let (content, mentioned_pubkeys) =
        format_tweet_as_nostr_content_with_mentions(&tweet, &content_media_urls, &mut resolver)
            .await?;

    let existing_event = adapter.find_event_by_tweet(&tweet_id, &keys).await?;
    let (event_id_hex, event_json) = if let Some(existing) = existing_event {
        if force {
            debug!("Existing event found for {tweet_id}, will overwrite due to --force");
            let tags = build_nostr_event_tags(
                &tweet.id,
                &original_media_urls,
                &blossom_urls,
                &mentioned_pubkeys,
            )?;
            let created_at = tweet.created_at.unix_timestamp()?;
            let draft = NostrEventDraft {
                content,
                tags,
                created_at: Some(created_at),
            };
            let result = adapter.publish_event(&draft, &keys).await?;
            (
                result.event_id.as_str().to_string(),
                result.event_json.clone(),
            )
        } else {
            (
                existing.id.to_hex(),
                Some(
                    serde_json::to_string_pretty(&existing)
                        .context("Failed to serialize existing Nostr event to JSON")?,
                ),
            )
        }
    } else {
        let tags = build_nostr_event_tags(
            &tweet.id,
            &original_media_urls,
            &blossom_urls,
            &mentioned_pubkeys,
        )?;
        let created_at = tweet.created_at.unix_timestamp()?;
        let draft = NostrEventDraft {
            content,
            tags,
            created_at: Some(created_at),
        };
        let result = adapter.publish_event(&draft, &keys).await?;
        (
            result.event_id.as_str().to_string(),
            result.event_json.clone(),
        )
    };

    let created_at = tweet.created_at.unix_timestamp()?;
    let pubkey_hex = keys.public_key().to_hex();
    let mut info_media_urls = original_media_urls.clone();
    if !blossom_urls.is_empty() {
        info_media_urls.extend(blossom_urls.clone());
    }
    let info = NostrEventInfo {
        tweet_id: tweet.id.clone(),
        event_id: NostrEventId::parse(&event_id_hex)?,
        pubkey: NostrPubkey::parse(&pubkey_hex)?,
        created_at,
        media_urls: info_media_urls,
        relays: parse_relay_urls(relays)?,
        event_json,
    };
    storage.save_nostr_event_info(&info).await?;

    if !skip_profiles {
        let referenced_users = collect_usernames_from_tweet(&tweet);
        if !referenced_users.is_empty() {
            let profiles_to_post =
                filter_profiles_to_post(referenced_users, &adapter, data_dir, force, &mnemonic)
                    .await?;
            if !profiles_to_post.is_empty() {
                let _ = post_referenced_profiles(&profiles_to_post, &adapter, data_dir, &mnemonic)
                    .await?;
            }
        }
    }

    Ok(())
}

async fn post_user_to_nostr(
    data_dir: &Path,
    mnemonic: &str,
    username: &str,
    relays: &[String],
    blossom_servers: &[String],
    force: bool,
    skip_profiles: bool,
) -> Result<()> {
    let username = parse_username(username)?;
    let storage = FileStorage::new(data_dir)?;
    let summaries = storage.list_tweets().await?;
    let mut referenced_users = HashSet::new();
    let mut matched = 0usize;

    for summary in summaries {
        let tweet = summary.tweet;
        if tweet.author.username.normalized() != username.normalized() {
            continue;
        }
        matched += 1;
        if !skip_profiles {
            referenced_users.extend(collect_usernames_from_tweet(&tweet));
        }
        post_tweet_to_nostr(
            data_dir,
            None,
            mnemonic,
            tweet.id.as_str(),
            relays,
            blossom_servers,
            force,
            true,
        )
        .await?;
    }

    ensure!(
        matched > 0,
        "No cached tweets found for user @{username}. Please fetch tweets first using the 'user-tweets' command.",
        username = username.as_str()
    );

    if !skip_profiles && !referenced_users.is_empty() {
        let mnemonic = MnemonicPhrase::parse(mnemonic)?;
        let keys = Keys::generate();
        let adapter = NostrSdkAdapter::new(&keys, relays, data_dir).await?;
        let profiles_to_post =
            filter_profiles_to_post(referenced_users, &adapter, data_dir, force, &mnemonic).await?;
        if !profiles_to_post.is_empty() {
            let _ =
                post_referenced_profiles(&profiles_to_post, &adapter, data_dir, &mnemonic).await?;
        }
    }

    Ok(())
}

async fn post_profile_to_nostr(
    data_dir: &Path,
    mnemonic: &str,
    username: &str,
    relays: &[String],
) -> Result<()> {
    let username = parse_username(username)?;
    let storage = FileStorage::new(data_dir)?;
    let Some(user) = storage.load_latest_user_profile(&username).await? else {
        bail!(
            "No cached profile found for @{username}",
            username = username.as_str()
        );
    };

    let mnemonic = MnemonicPhrase::parse(mnemonic)?;
    let keys = derive_keys_for_user(&user.id, &mnemonic)?;
    let adapter = NostrSdkAdapter::new(&keys, relays, data_dir).await?;
    let metadata = build_profile_metadata(&user, &username);
    let _ = adapter.publish_profile(metadata, &keys).await?;
    Ok(())
}

async fn update_relay_list(mnemonic: &str, relays: &[String]) -> Result<()> {
    let mnemonic = MnemonicPhrase::parse(mnemonic)?;
    let user_id = UserId::parse("0")?;
    let keys = derive_keys_for_user(&user_id, &mnemonic)?;
    let adapter = NostrSdkAdapter::new(&keys, relays, Path::new(".")).await?;
    adapter.update_relay_list(relays, &keys).await?;
    Ok(())
}

async fn show_tweet(
    data_dir: &Path,
    bearer_token: Option<&str>,
    mnemonic: Option<&str>,
    cmd: ShowTweetCommand,
) -> Result<()> {
    let tweet_id = TweetId::parse(&cmd.tweet)
        .with_context(|| format!("Failed to parse tweet ID from {}", cmd.tweet))?;
    let tweet = load_or_fetch_tweet(data_dir, bearer_token, &tweet_id).await?;
    let media_urls = extract_media_urls(&tweet);

    let mnemonic = mnemonic.map(MnemonicPhrase::parse).transpose()?;
    let mut resolver = NostrLinkResolver::new(Some(data_dir.to_path_buf()), mnemonic);
    let (content, mentioned_pubkeys) =
        format_tweet_as_nostr_content_with_mentions(&tweet, &media_urls, &mut resolver).await?;

    let tags = build_nostr_event_tags(&tweet.id, &media_urls, &[], &mentioned_pubkeys)?;
    let created_at = tweet.created_at.unix_timestamp().ok();
    let keys = Keys::generate();
    let draft = NostrEventDraft {
        content: content.clone(),
        tags,
        created_at: created_at.map(|ts| UnixTimestamp::new(ts.value())),
    };
    let event = build_event_from_draft(&draft, &keys).await?;

    let author = if !tweet.author.username.as_str().is_empty() {
        tweet.author.username.as_str().to_string()
    } else {
        tweet.author.id.as_str().to_string()
    };
    let preview = format!("{}...", content.chars().take(100).collect::<String>());

    let output = json!({
        "twitter": tweet,
        "nostr": {
            "event": event,
            "metadata": {
                "original_tweet_id": tweet_id.as_str(),
                "original_author": author,
                "created_at_human": tweet.created_at.as_str(),
                "content_preview": preview,
                "tags_count": event.tags.len(),
                "pubkey_hex": event.pubkey.to_hex(),
                "event_id_hex": event.id.to_hex(),
            }
        }
    });

    if cmd.compact {
        println!("{}", serde_json::to_string(&output)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&output)?);
    }
    Ok(())
}

struct DaemonConfig {
    users: Vec<String>,
    relays: Vec<String>,
    blossom_servers: Vec<String>,
    poll_interval: u64,
    data_dir: PathBuf,
    mnemonic: MnemonicPhrase,
    bearer_token: String,
}

#[derive(Clone, Debug)]
struct UserState {
    last_poll_time: Option<Instant>,
    last_success_time: Option<Instant>,
    last_profile_post_time: Option<Instant>,
    profile_posted: bool,
    consecutive_failures: u32,
    total_tweets_downloaded: u64,
    total_tweets_posted: u64,
    is_processing: bool,
}

impl UserState {
    fn new() -> Self {
        Self {
            last_poll_time: None,
            last_success_time: None,
            last_profile_post_time: None,
            profile_posted: false,
            consecutive_failures: 0,
            total_tweets_downloaded: 0,
            total_tweets_posted: 0,
            is_processing: false,
        }
    }

    fn next_poll_delay(&self, base_interval: u64) -> Duration {
        if self.consecutive_failures > 0 {
            let backoff_seconds = base_interval * 2_u64.pow(self.consecutive_failures.min(5));
            return Duration::from_secs(backoff_seconds.min(3600));
        }

        Duration::from_secs(base_interval)
    }
}

#[derive(Clone, Debug)]
struct DaemonStats {
    start_time: Instant,
    total_polls: u64,
    successful_polls: u64,
    failed_polls: u64,
    total_tweets_downloaded: u64,
    total_tweets_posted: u64,
}

#[derive(Clone)]
struct DaemonState {
    config: Arc<DaemonConfig>,
    twitter_client: Arc<TwitterClient>,
    nostr_adapter: Arc<NostrSdkAdapter>,
    user_states: Arc<RwLock<HashMap<String, UserState>>>,
    stats: Arc<RwLock<DaemonStats>>,
    rate_limiter: Arc<Mutex<RateLimiter>>,
}

struct RateLimiter {
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
        let cutoff = Instant::now() - self.window_duration;
        while let Some(&front) = self.request_times.front() {
            if front < cutoff {
                self.request_times.pop_front();
            } else {
                break;
            }
        }

        if self.request_times.len() >= self.requests_per_window as usize
            && let Some(&oldest) = self.request_times.front()
        {
            let wait_until = oldest + self.window_duration;
            let wait_duration = wait_until.saturating_duration_since(Instant::now());
            if wait_duration > Duration::ZERO {
                info!("Rate limit reached, waiting {wait_duration:?}");
                time::sleep(wait_duration).await;
            }
        }

        self.request_times.push_back(Instant::now());
    }
}

async fn daemon(
    data_dir: &Path,
    bearer_token: &str,
    mnemonic: &str,
    users: &[String],
    relays: &[String],
    blossom_servers: &[String],
    poll_interval: u64,
) -> Result<()> {
    info!(
        "Starting daemon for {user_count} users with {poll_interval} second base interval",
        user_count = users.len()
    );

    let mnemonic = MnemonicPhrase::parse(mnemonic)?;
    let config = Arc::new(DaemonConfig {
        users: users.to_vec(),
        relays: relays.to_vec(),
        blossom_servers: blossom_servers.to_vec(),
        poll_interval,
        data_dir: data_dir.to_path_buf(),
        mnemonic,
        bearer_token: bearer_token.to_string(),
    });

    let state = init_daemon(config).await?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("Received shutdown signal (Ctrl+C)");
        let _ = shutdown_tx.send(());
    });

    let stats_handle = spawn_stats_reporter(state.stats.clone());
    let final_stats = state.stats.clone();

    tokio::select! {
        result = run_daemon_loop(state.clone()) => {
            if let Err(e) = result {
                error!("Daemon error: {e}");
                return Err(e);
            }
            Ok(())
        }
        _ = shutdown_rx => {
            info!("Received shutdown signal, shutting down daemon...");
            stats_handle.abort();
            print_final_stats(&final_stats).await;
            Ok(())
        }
    }
}

async fn init_daemon(config: Arc<DaemonConfig>) -> Result<DaemonState> {
    if !config.data_dir.exists() {
        std::fs::create_dir_all(&config.data_dir).context("Failed to create data directory")?;
    }

    info!("Initializing Twitter client");
    let twitter_client = Arc::new(TwitterClient::new(&config.bearer_token)?);

    info!(
        "Connecting to {relay_count} Nostr relays",
        relay_count = config.relays.len()
    );
    let ephemeral = Keys::generate();
    let nostr_adapter =
        Arc::new(NostrSdkAdapter::new(&ephemeral, &config.relays, &config.data_dir).await?);

    let mut user_states = HashMap::new();
    for username in &config.users {
        let key = Username::parse(username)
            .map(|u| u.as_str().to_string())
            .unwrap_or_else(|_| username.clone());
        user_states.insert(key.clone(), UserState::new());
    }

    let rate_limiter = Arc::new(Mutex::new(RateLimiter::new(100, 900)));

    Ok(DaemonState {
        config,
        twitter_client,
        nostr_adapter,
        user_states: Arc::new(RwLock::new(user_states)),
        stats: Arc::new(RwLock::new(DaemonStats {
            start_time: Instant::now(),
            total_polls: 0,
            successful_polls: 0,
            failed_polls: 0,
            total_tweets_downloaded: 0,
            total_tweets_posted: 0,
        })),
        rate_limiter,
    })
}

async fn run_daemon_loop(state: DaemonState) -> Result<()> {
    loop {
        let poll_start = Instant::now();

        let users_to_poll = get_users_ready_for_polling(&state).await;
        if users_to_poll.is_empty() {
            trace!("No users ready for polling, sleeping for 10 seconds");
            time::sleep(Duration::from_secs(10)).await;
            continue;
        }

        info!(
            "Starting polling for {user_count} users",
            user_count = users_to_poll.len()
        );

        for username in &users_to_poll {
            match process_user(state.clone(), username.clone()).await {
                Ok(()) => {
                    debug!("Successfully processed user: {username}");
                    let mut user_states = state.user_states.write().await;
                    if let Some(user_state) = user_states.get_mut(username) {
                        user_state.consecutive_failures = 0;
                        user_state.last_success_time = Some(Instant::now());
                    }
                }
                Err(e) => {
                    error!("Error processing user @{username}: {e}");
                    let mut user_states = state.user_states.write().await;
                    if let Some(user_state) = user_states.get_mut(username) {
                        user_state.consecutive_failures += 1;
                        match user_state.consecutive_failures {
                            1..=2 => warn!(
                                "User @{username} failed {failures} times, retrying with backoff",
                                failures = user_state.consecutive_failures
                            ),
                            3..=5 => error!(
                                "User @{username} failed {failures} times, increasing backoff",
                                failures = user_state.consecutive_failures
                            ),
                            _ => error!(
                                "User @{username} failed {failures} times, manual intervention may be required",
                                failures = user_state.consecutive_failures
                            ),
                        }
                    }

                    if let Some(twitter_err) = e.downcast_ref::<TwitterAdapterError>() {
                        match twitter_err {
                            TwitterAdapterError::UserNotFound { username: user } => {
                                error!("User @{user} not found");
                            }
                            TwitterAdapterError::TweetNotFound { tweet_id } => {
                                debug!("Tweet {tweet_id} not found for @{username}");
                            }
                        }
                    }
                }
            }
        }

        let poll_duration = poll_start.elapsed();
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
                "Tweets: {total_tweets_downloaded} downloaded, {total_tweets_posted} posted",
                total_tweets_downloaded = stats.total_tweets_downloaded,
                total_tweets_posted = stats.total_tweets_posted
            );
            info!("Users: {healthy_users} healthy, {failing_users} failing");

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

        time::sleep(Duration::from_secs(5)).await;
    }
}

async fn get_users_ready_for_polling(state: &DaemonState) -> Vec<String> {
    let user_states = state.user_states.read().await;
    let mut ready = Vec::new();

    for (username, user_state) in user_states.iter() {
        if user_state.is_processing {
            continue;
        }

        let delay = user_state.next_poll_delay(state.config.poll_interval);
        let should_poll = match user_state.last_poll_time {
            None => true,
            Some(last) => last.elapsed() >= delay,
        };

        if should_poll {
            ready.push(username.clone());
        }
    }

    ready
}

async fn process_user(state: DaemonState, username: String) -> Result<()> {
    {
        let mut user_states = state.user_states.write().await;
        if let Some(user_state) = user_states.get_mut(&username) {
            user_state.is_processing = true;
            user_state.last_poll_time = Some(Instant::now());
        }
    }

    debug!("Processing user: @{username}");
    state.rate_limiter.lock().await.wait_if_needed().await;

    {
        let user_states = state.user_states.read().await;
        let user_state = user_states.get(&username).cloned();
        drop(user_states);

        if let Some(user_state) = user_state {
            if !user_state.profile_posted
                || should_refresh_profile(user_state.last_profile_post_time)
            {
                if let Ok(username_parsed) = Username::parse(&username) {
                    let _ = ensure_user_profile_posted(&state, &username_parsed).await;
                }
            }
        }
    }

    let result = process_user_tweets(&state, &username).await;

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
                        info!("User @{username}: downloaded {downloaded} tweets, posted {posted}",);
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

async fn process_user_tweets(state: &DaemonState, username: &str) -> Result<(u64, u64)> {
    let username_parsed = Username::parse(username)?;
    let storage = FileStorage::new(&state.config.data_dir)?;
    let since_id = storage
        .find_latest_tweet_id_for_user(&username_parsed)
        .await?;

    let tweets =
        fetch_timeline_with_retry(&state.twitter_client, &username_parsed, since_id).await?;

    if tweets.is_empty() {
        return Ok((0, 0));
    }

    let mut new_tweets = 0u64;
    let mut posted_to_nostr = 0u64;

    for mut tweet in tweets {
        if let Some(cached) = storage.load_tweet(&tweet.id).await? {
            let keys = derive_keys_for_user(&cached.author.id, &state.config.mnemonic)?;
            if !is_tweet_posted_to_nostr(&cached.id, &*state.nostr_adapter, &keys).await?
                && post_tweet_to_nostr_with_state(&cached, state).await.is_ok()
            {
                posted_to_nostr += 1;
                let referenced = collect_usernames_from_tweet(&cached);
                if !referenced.is_empty() {
                    let _ = post_referenced_profiles(
                        &referenced,
                        &*state.nostr_adapter,
                        &state.config.data_dir,
                        &state.config.mnemonic,
                    )
                    .await;
                }
            }
            continue;
        }

        state
            .twitter_client
            .enrich_referenced_tweets(&mut tweet)
            .await?;
        storage.save_tweet(&tweet).await?;
        new_tweets += 1;
        let _ = fetch_media_assets(&state.config.data_dir, &tweet).await?;

        let referenced = collect_usernames_from_tweet(&tweet);
        if !referenced.is_empty() {
            download_profiles_for_usernames(&state.twitter_client, &storage, referenced.clone())
                .await?;
        }

        let keys = derive_keys_for_user(&tweet.author.id, &state.config.mnemonic)?;
        if !is_tweet_posted_to_nostr(&tweet.id, &*state.nostr_adapter, &keys).await?
            && post_tweet_to_nostr_with_state(&tweet, state).await.is_ok()
        {
            posted_to_nostr += 1;
            if !referenced.is_empty() {
                let _ = post_referenced_profiles(
                    &referenced,
                    &*state.nostr_adapter,
                    &state.config.data_dir,
                    &state.config.mnemonic,
                )
                .await;
            }
        }
    }

    Ok((new_tweets, posted_to_nostr))
}

async fn is_tweet_posted_to_nostr(
    tweet_id: &TweetId,
    adapter: &impl NostrAdapter,
    keys: &Keys,
) -> Result<bool> {
    match adapter.find_event_by_tweet(tweet_id, keys).await {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(_) => Ok(false),
    }
}

fn should_refresh_profile(last_post_time: Option<Instant>) -> bool {
    const PROFILE_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
    match last_post_time {
        None => true,
        Some(time) => time.elapsed() > PROFILE_REFRESH_INTERVAL,
    }
}

async fn ensure_user_profile_posted(state: &DaemonState, username: &Username) -> Result<bool> {
    let exists = check_profile_exists(
        username,
        &*state.nostr_adapter,
        &state.config.data_dir,
        &state.config.mnemonic,
    )
    .await?;

    if !exists {
        return post_user_profile(state, username).await;
    }

    Ok(false)
}

async fn post_user_profile(state: &DaemonState, username: &Username) -> Result<bool> {
    download_profiles_for_usernames(
        &state.twitter_client,
        &FileStorage::new(&state.config.data_dir)?,
        HashSet::from([username.as_str().to_string()]),
    )
    .await?;

    match post_user_profile_with_relay_list(
        username,
        &*state.nostr_adapter,
        &state.config.data_dir,
        &state.config.mnemonic,
        &state.config.relays,
    )
    .await
    {
        Ok(()) => {
            let mut user_states = state.user_states.write().await;
            if let Some(user_state) = user_states.get_mut(username.as_str()) {
                user_state.last_profile_post_time = Some(Instant::now());
                user_state.profile_posted = true;
            }
            Ok(true)
        }
        Err(err) => {
            warn!(
                "Failed to post profile for @{username}: {err}",
                username = username.as_str()
            );
            Ok(false)
        }
    }
}

async fn fetch_timeline_with_retry(
    client: &TwitterClient,
    username: &Username,
    since_id: Option<TweetId>,
) -> Result<Vec<Tweet>> {
    let backoff = ExponentialBackoff {
        initial_interval: Duration::from_secs(1),
        randomization_factor: 0.1,
        multiplier: 2.0,
        max_interval: Duration::from_secs(60),
        max_elapsed_time: Some(Duration::from_secs(300)),
        ..Default::default()
    };

    retry(backoff, || async {
        let query = UserTweetsQuery {
            count: 20,
            days: None,
            since_id: since_id.clone(),
        };

        match client.fetch_user_tweets(username, query).await {
            Ok(tweets) => Ok(tweets),
            Err(e) => {
                if let Some(twitter_err) = e.downcast_ref::<TwitterAdapterError>() {
                    match twitter_err {
                        TwitterAdapterError::UserNotFound { .. }
                        | TwitterAdapterError::TweetNotFound { .. } => {
                            return Err(backoff::Error::permanent(e));
                        }
                    }
                }
                Err(backoff::Error::transient(e))
            }
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("Failed after retries: {e}"))
}

async fn post_tweet_to_nostr_with_state(tweet: &Tweet, state: &DaemonState) -> Result<()> {
    let original_media_urls = extract_media_urls(tweet);
    let media_fetcher = DefaultMediaFetcher;
    let assets = media_fetcher
        .fetch_media_assets(&state.config.data_dir, tweet)
        .await?;
    let blossom = build_blossom_client(&state.config.blossom_servers).await?;
    let blossom_urls = if let Some(client) = blossom.as_ref() {
        client.upload_media(&assets).await?
    } else {
        Vec::new()
    };

    let content_media_urls = if blossom_urls.is_empty() {
        original_media_urls.clone()
    } else {
        blossom_urls.clone()
    };

    let keys = derive_keys_for_user(&tweet.author.id, &state.config.mnemonic)?;
    let mut resolver = NostrLinkResolver::new(
        Some(state.config.data_dir.clone()),
        Some(state.config.mnemonic.clone()),
    );
    let (content, mentioned_pubkeys) =
        format_tweet_as_nostr_content_with_mentions(tweet, &content_media_urls, &mut resolver)
            .await?;

    let tags = build_nostr_event_tags(
        &tweet.id,
        &original_media_urls,
        &blossom_urls,
        &mentioned_pubkeys,
    )?;
    let created_at = tweet.created_at.unix_timestamp()?;
    let draft = NostrEventDraft {
        content,
        tags,
        created_at: Some(created_at),
    };

    let _ = state.nostr_adapter.publish_event(&draft, &keys).await?;

    Ok(())
}

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
                "Stats | Uptime: {hours}h{minutes}m | Polls: {total_polls} (✓{successful_polls} ✗{failed_polls}) | Downloaded: {total_tweets_downloaded} | Posted: {total_tweets_posted}",
                total_polls = stats.total_polls,
                successful_polls = stats.successful_polls,
                failed_polls = stats.failed_polls,
                total_tweets_downloaded = stats.total_tweets_downloaded,
                total_tweets_posted = stats.total_tweets_posted
            );
        }
    })
}

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
        "Total tweets posted: {total_tweets_posted}",
        total_tweets_posted = stats.total_tweets_posted
    );
    info!("===============================");
}

#[allow(clippy::too_many_arguments)]
async fn utils_query_events(
    relays: Vec<String>,
    kind: Option<u32>,
    author: Option<String>,
    limit: usize,
    since: Option<u64>,
    until: Option<u64>,
    format: String,
    output: Option<String>,
) -> Result<()> {
    let client = NostrClient::default();
    for relay in &relays {
        client
            .add_relay(relay)
            .await
            .with_context(|| format!("Failed to add relay: {relay}"))?;
    }
    client.connect().await;

    let mut filter = Filter::new();
    if let Some(kind) = kind {
        filter = filter.kind(Kind::from(kind as u16));
    }
    if let Some(author) = author {
        let pubkey = if author.starts_with("npub") {
            nostr_sdk::PublicKey::from_bech32(&author)
                .with_context(|| format!("Invalid npub: {author}"))?
        } else {
            nostr_sdk::PublicKey::from_hex(&author)
                .with_context(|| format!("Invalid pubkey hex: {author}"))?
        };
        filter = filter.author(pubkey);
    }
    if let Some(since) = since {
        filter = filter.since(Timestamp::from(since));
    }
    if let Some(until) = until {
        filter = filter.until(Timestamp::from(until));
    }
    filter = filter.limit(limit);

    let events = client
        .fetch_events(filter, Duration::from_secs(10))
        .await
        .context("Failed to fetch events from relays")?;

    let output_str = match format.as_str() {
        "json" => {
            let json_events: Vec<String> = events.iter().map(|e| e.as_json()).collect();
            let json_array = format!("[{}]", json_events.join(","));
            let parsed: serde_json::Value = serde_json::from_str(&json_array)?;
            serde_json::to_string_pretty(&parsed)?
        }
        _ => {
            let mut out = String::new();
            out.push_str(&format!("Found {} events\n", events.len()));
            out.push_str(&"═".repeat(80));
            out.push('\n');
            for (idx, event) in events.iter().enumerate() {
                out.push_str(&format!("\n📝 Event {} of {}\n", idx + 1, events.len()));
                out.push_str(&"─".repeat(40));
                out.push('\n');
                out.push_str(&format!("ID: {}\n", event.id));
                out.push_str(&format!("Author: {}\n", event.pubkey));
                let kind_desc = match event.kind {
                    Kind::Metadata => "Metadata (0)".to_string(),
                    Kind::TextNote => "Text Note (1)".to_string(),
                    Kind::ContactList => "Contact List (3)".to_string(),
                    Kind::Repost => "Repost (6)".to_string(),
                    Kind::Reaction => "Reaction (7)".to_string(),
                    _ => format!("Kind {}", event.kind.as_u16()),
                };
                out.push_str(&format!("Type: {kind_desc}\n"));
                if let Ok(ts) =
                    OffsetDateTime::from_unix_timestamp(event.created_at.as_secs() as i64)
                {
                    out.push_str(&format!(
                        "Created: {} ({})\n",
                        ts.format(&format_description::well_known::Rfc3339)
                            .unwrap_or_else(|_| "Unknown".to_string()),
                        event.created_at.as_secs()
                    ));
                }
                if !event.tags.is_empty() {
                    out.push_str(&format!("Tags: {} tag(s)\n", event.tags.len()));
                    for tag in event.tags.iter().take(3) {
                        out.push_str(&format!("  - {tag:?}\n"));
                    }
                    if event.tags.len() > 3 {
                        out.push_str(&format!("  ... and {} more\n", event.tags.len() - 3));
                    }
                }
                out.push_str("\nContent:\n");
                if event.kind == Kind::Metadata {
                    if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&event.content) {
                        if let Ok(pretty) = serde_json::to_string_pretty(&meta) {
                            out.push_str(&pretty);
                        } else {
                            out.push_str(&event.content);
                        }
                    } else {
                        out.push_str(&event.content);
                    }
                } else if event.content.len() > 500 {
                    out.push_str(&event.content[..500]);
                    out.push_str(&format!(
                        "\n... ({} more characters)",
                        event.content.len() - 500
                    ));
                } else {
                    out.push_str(&event.content);
                }
                out.push('\n');
            }
            out.push_str(&"═".repeat(80));
            out.push('\n');
            out
        }
    };

    if let Some(path) = output {
        std::fs::write(&path, &output_str)
            .with_context(|| format!("Failed to write output to {path}"))?;
    } else {
        println!("{output_str}");
    }
    client.disconnect().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use tempfile::TempDir;

    struct DummyNostrAdapter;

    impl NostrAdapter for DummyNostrAdapter {
        async fn publish_event(
            &self,
            _draft: &NostrEventDraft,
            _keys: &Keys,
        ) -> Result<NostrEventResult> {
            Ok(NostrEventResult {
                event_id: NostrEventId::parse(&format!("{:064x}", 1))?,
                event_json: None,
            })
        }

        async fn find_event_by_tweet(
            &self,
            _tweet_id: &TweetId,
            _keys: &Keys,
        ) -> Result<Option<nostr_sdk::Event>> {
            Ok(None)
        }

        async fn profile_exists(&self, _pubkey: &nostr_sdk::PublicKey) -> Result<bool> {
            Ok(false)
        }

        async fn publish_profile(
            &self,
            _metadata: Metadata,
            _keys: &Keys,
        ) -> Result<nostr_sdk::EventId> {
            Ok(nostr_sdk::EventId::all_zeros())
        }

        async fn update_relay_list(&self, _relays: &[String], _keys: &Keys) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn help_includes_global_flags_and_commands() {
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("--data-dir"));
        assert!(help.contains("--bearer-token"));
        assert!(help.contains("--mnemonic"));
        assert!(help.contains("--verbose"));

        for command in [
            "fetch-profile",
            "fetch-tweet",
            "user-tweets",
            "list-tweets",
            "clear-cache",
            "post-tweet-to-nostr",
            "post-user-to-nostr",
            "post-tweet",
            "post-profile-to-nostr",
            "update-relay-list",
            "show-tweet",
            "daemon",
            "utils",
        ] {
            assert!(help.contains(command));
        }
    }

    #[test]
    fn fetch_tweet_help_includes_skip_profiles() {
        let mut cmd = Cli::command();
        let sub = cmd.find_subcommand_mut("fetch-tweet").expect("subcommand");
        let help = sub.render_help().to_string();
        assert!(help.contains("--skip-profiles"));
    }

    #[test]
    fn data_dir_is_required() {
        let cli = Cli {
            data_dir: None,
            bearer_token: None,
            mnemonic: None,
            verbose: false,
            command: Commands::ListTweets,
        };

        let err = resolve_data_dir_with_fallback(&cli, None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Data directory not specified. Please set --data-dir or NOSTRWEET_DATA_DIR environment variable"
        );
    }

    #[test]
    fn bearer_token_required_for_fetch_profile() {
        let cli = Cli {
            data_dir: None,
            bearer_token: None,
            mnemonic: None,
            verbose: false,
            command: Commands::FetchProfile {
                username: "tester".to_string(),
            },
        };

        let err = require_bearer_token(&cli, true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Twitter bearer token not specified. Please set --bearer-token or TWITTER_BEARER_TOKEN environment variable"
        );
    }

    #[test]
    fn mnemonic_required_for_post_tweet() {
        let cli = Cli {
            data_dir: None,
            bearer_token: None,
            mnemonic: None,
            verbose: false,
            command: Commands::PostTweet {
                tweet_url_or_id: "123".to_string(),
                relays: vec!["wss://example.com".to_string()],
                blossom_servers: Vec::new(),
                force: false,
                skip_profiles: false,
            },
        };

        let err = require_mnemonic(&cli, true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Mnemonic not provided. Please use --mnemonic flag or NOSTRWEET_MNEMONIC environment variable."
        );
    }

    #[test]
    fn profile_metadata_includes_disclaimer() -> Result<()> {
        let username = Username::parse("tester")?;
        let user = User {
            id: UserId::parse("123")?,
            name: Some("Test User".to_string()),
            username: username.clone(),
            profile_image_url: None,
            description: Some("Hello".to_string()),
            url: None,
            entities: None,
        };

        let metadata = build_profile_metadata(&user, &username);
        let about = metadata.about.unwrap_or_default();
        assert!(about.contains("Hello"));
        assert!(about.contains("https://x.com/tester"));
        assert!(about.contains("nostrweet"));
        Ok(())
    }

    #[test]
    fn should_refresh_profile_after_24h() {
        assert!(should_refresh_profile(None));
        let recent = Instant::now() - Duration::from_secs(60 * 60);
        assert!(!should_refresh_profile(Some(recent)));
        let old = Instant::now() - Duration::from_secs(60 * 60 * 25);
        assert!(should_refresh_profile(Some(old)));
    }

    #[tokio::test]
    async fn save_nostr_event_json_writes_file() -> Result<()> {
        let temp = TempDir::new()?;
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::TextNote, "hello")
            .sign(&keys)
            .await?;
        save_nostr_event_json(temp.path(), &event)?;
        let path = temp
            .path()
            .join("nostr_events")
            .join(format!("{}.json", event.id.to_hex()));
        assert!(path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn post_user_to_nostr_errors_when_no_cached_tweets() -> Result<()> {
        let temp = TempDir::new()?;
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let result =
            post_user_to_nostr(temp.path(), mnemonic, "tester", &[], &[], false, false).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No cached tweets found for user @tester")
        );
        Ok(())
    }

    #[tokio::test]
    async fn post_tweet_to_nostr_skips_when_event_info_exists() -> Result<()> {
        let temp = TempDir::new()?;
        let storage = FileStorage::new(temp.path())?;
        let tweet_id = TweetId::parse("123456789")?;
        let event_id = NostrEventId::parse(&format!("{:064x}", 1))?;
        let pubkey = NostrPubkey::parse(&format!("{:064x}", 2))?;
        let info = NostrEventInfo {
            tweet_id: tweet_id.clone(),
            event_id,
            pubkey,
            created_at: UnixTimestamp::new(1),
            media_urls: vec![HttpUrl::parse("https://example.com/media.jpg")?],
            relays: vec![nostrweet_core::RelayUrl::parse("https://relay.example")?],
            event_json: None,
        };
        storage.save_nostr_event_info(&info).await?;

        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let result = post_tweet_to_nostr(
            temp.path(),
            None,
            mnemonic,
            tweet_id.as_str(),
            &[],
            &[],
            false,
            false,
        )
        .await;
        assert!(result.is_ok());
        Ok(())
    }

    #[test]
    fn next_poll_delay_exponential_backoff() {
        let mut state = UserState::new();
        assert_eq!(state.next_poll_delay(300), Duration::from_secs(300));
        state.consecutive_failures = 1;
        assert_eq!(state.next_poll_delay(300), Duration::from_secs(600));
        state.consecutive_failures = 2;
        assert_eq!(state.next_poll_delay(300), Duration::from_secs(1200));
        state.consecutive_failures = 10;
        assert_eq!(state.next_poll_delay(300), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn get_users_ready_for_polling_respects_delay() -> Result<()> {
        let temp = TempDir::new()?;
        let mnemonic = MnemonicPhrase::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )?;
        let config = Arc::new(DaemonConfig {
            users: vec!["alice".to_string(), "bob".to_string()],
            relays: Vec::new(),
            blossom_servers: Vec::new(),
            poll_interval: 300,
            data_dir: temp.path().to_path_buf(),
            mnemonic,
            bearer_token: "token".to_string(),
        });

        let twitter_client = Arc::new(TwitterClient::new(&config.bearer_token)?);
        let keys = Keys::generate();
        let nostr_adapter =
            Arc::new(NostrSdkAdapter::new(&keys, &config.relays, &config.data_dir).await?);
        let mut user_states = HashMap::new();

        let mut alice = UserState::new();
        alice.last_poll_time = Some(Instant::now() - Duration::from_secs(400));
        user_states.insert("alice".to_string(), alice);

        let mut bob = UserState::new();
        bob.last_poll_time = Some(Instant::now() - Duration::from_secs(100));
        user_states.insert("bob".to_string(), bob);

        let state = DaemonState {
            config,
            twitter_client,
            nostr_adapter,
            user_states: Arc::new(RwLock::new(user_states)),
            stats: Arc::new(RwLock::new(DaemonStats {
                start_time: Instant::now(),
                total_polls: 0,
                successful_polls: 0,
                failed_polls: 0,
                total_tweets_downloaded: 0,
                total_tweets_posted: 0,
            })),
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(100, 900))),
        };

        let ready = get_users_ready_for_polling(&state).await;
        assert!(ready.contains(&"alice".to_string()));
        assert!(!ready.contains(&"bob".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn filter_profiles_to_post_force_returns_all() -> Result<()> {
        let temp = TempDir::new()?;
        let mnemonic = MnemonicPhrase::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )?;
        let usernames = HashSet::from([
            "alice".to_string(),
            "bob".to_string(),
            "charlie".to_string(),
        ]);

        let result = filter_profiles_to_post(
            usernames.clone(),
            &DummyNostrAdapter,
            temp.path(),
            true,
            &mnemonic,
        )
        .await?;
        assert_eq!(result, usernames);
        Ok(())
    }

    #[tokio::test]
    async fn post_referenced_profiles_empty_returns_zero() -> Result<()> {
        let temp = TempDir::new()?;
        let mnemonic = MnemonicPhrase::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )?;
        let usernames = HashSet::new();
        let count =
            post_referenced_profiles(&usernames, &DummyNostrAdapter, temp.path(), &mnemonic)
                .await?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[tokio::test]
    async fn find_existing_event_handles_no_relays() -> Result<()> {
        let client = NostrClient::new(Keys::generate());
        let tweet_id = TweetId::parse("123456789")?;
        let keys = Keys::generate();
        let found = find_existing_event(&client, &tweet_id, &keys).await?;
        assert!(found.is_none());
        Ok(())
    }
}
