use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use nostr_sdk::nips::nip65::RelayMetadata;
use nostr_sdk::{
    Client as NostrClient, EventBuilder, Filter, FromBech32, JsonUtil, Keys, Kind, Metadata, Tag,
    Timestamp, ToBech32,
};
use nostrweet_blossom::BlossomClient;
use nostrweet_core::{BlossomPort, TwitterPort};
use nostrweet_core::{
    HttpUrl, Media, MediaAsset, MediaKind, MediaVariant, MnemonicPhrase, NostrEventDraft,
    NostrEventId, NostrEventInfo, NostrPubkey, NostrTag, StoragePort, Tweet, TweetId,
    UnixTimestamp, UserId, UserTweetsQuery, Username, decode_html_entities,
    derive_nostr_secret_key, expand_urls_in_text, extract_media_urls,
};
use nostrweet_storage::FileStorage;
use nostrweet_twitter::{TwitterAdapterError, TwitterClient};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use time::OffsetDateTime;

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
    let cli = Cli::parse();
    run(cli).await
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
    let Ok(format) =
        time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]")
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
    if storage.is_tweet_not_found(tweet_id).await? {
        bail!(
            "Tweet {} was previously marked as not found",
            tweet_id.as_str()
        );
    }

    if let Some(tweet) = storage.load_tweet(tweet_id).await? {
        return Ok(tweet);
    }

    let token = bearer_token.context("Twitter bearer token required to fetch tweet from API")?;
    let twitter = TwitterClient::new(token)?;
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

fn parse_relay_urls(relays: &[String]) -> Result<Vec<nostrweet_core::RelayUrl>> {
    relays
        .iter()
        .map(|relay| {
            nostrweet_core::RelayUrl::parse(relay)
                .with_context(|| format!("Invalid relay URL: {relay}"))
        })
        .collect()
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

    let mut tweet = load_or_fetch_tweet(data_dir, bearer_token, &tweet_id).await?;
    if let Some(token) = bearer_token {
        let twitter = TwitterClient::new(token)?;
        twitter.enrich_referenced_tweets(&mut tweet).await?;
        if !skip_profiles {
            download_profiles_for_tweet(&twitter, &storage, &tweet).await?;
        }
    }

    let original_media_urls = extract_media_urls(&tweet);
    let assets = fetch_media_assets(data_dir, &tweet).await?;
    let blossom_urls = if let Some(client) = build_blossom_client(blossom_servers).await? {
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
    let mut resolver = NostrLinkResolver::new(Some(data_dir.to_path_buf()), Some(mnemonic.clone()));
    let (content, mentioned_pubkeys) =
        format_tweet_as_nostr_content_with_mentions(&tweet, &content_media_urls, &mut resolver)
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

    let keys = derive_keys_for_user(&tweet.author.id, &mnemonic)?;
    let event = build_event_from_draft(&draft, &keys).await?;
    let client = build_nostr_client(&keys, relays).await?;
    let output = client
        .send_event(&event)
        .await
        .context("Failed to publish Nostr event")?;

    let event_id_hex = output.val.to_hex();
    let pubkey_hex = keys.public_key().to_hex();
    let info = NostrEventInfo {
        tweet_id: tweet.id.clone(),
        event_id: NostrEventId::parse(&event_id_hex)?,
        pubkey: NostrPubkey::parse(&pubkey_hex)?,
        created_at,
        media_urls: content_media_urls,
        relays: parse_relay_urls(relays)?,
        event_json: Some(event.as_json()),
    };
    storage.save_nostr_event_info(&info).await?;

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

    for summary in summaries {
        let tweet = summary.tweet;
        if tweet.author.username.normalized() != username.normalized() {
            continue;
        }
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

    if !skip_profiles && !referenced_users.is_empty() {
        for user in referenced_users {
            let _ = post_profile_to_nostr(data_dir, mnemonic, &user, relays).await;
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
    let client = build_nostr_client(&keys, relays).await?;

    let mut metadata = Metadata::new();
    if let Some(name) = &user.name {
        metadata = metadata.name(name);
    }
    if let Some(about) = &user.description {
        metadata = metadata.about(about);
    }
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

    let event = EventBuilder::metadata(&metadata)
        .sign(&keys)
        .await
        .context("Failed to sign metadata event")?;
    let _ = client
        .send_event(&event)
        .await
        .context("Failed to publish profile event")?;
    Ok(())
}

async fn update_relay_list(mnemonic: &str, relays: &[String]) -> Result<()> {
    let mnemonic = MnemonicPhrase::parse(mnemonic)?;
    let user_id = UserId::parse("0")?;
    let keys = derive_keys_for_user(&user_id, &mnemonic)?;
    let client = build_nostr_client(&keys, relays).await?;

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
        .sign(&keys)
        .await
        .context("Failed to sign relay list event")?;
    let _ = client
        .send_event(&event)
        .await
        .context("Failed to publish relay list event")?;
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

async fn daemon(
    data_dir: &Path,
    bearer_token: &str,
    mnemonic: &str,
    users: &[String],
    relays: &[String],
    blossom_servers: &[String],
    poll_interval: u64,
) -> Result<()> {
    let twitter = TwitterClient::new(bearer_token)?;
    let storage = FileStorage::new(data_dir)?;
    let interval = Duration::from_secs(poll_interval.max(1));

    loop {
        for user in users {
            let Ok(username) = Username::parse(user) else {
                continue;
            };
            let since_id = storage.find_latest_tweet_id_for_user(&username).await?;
            let query = UserTweetsQuery {
                count: 20,
                days: None,
                since_id,
            };
            let mut tweets = twitter.fetch_user_tweets(&username, query).await?;
            tweets.sort_by_key(|tweet| tweet.created_at.as_str().to_string());
            for mut tweet in tweets {
                twitter.enrich_referenced_tweets(&mut tweet).await?;
                storage.save_tweet(&tweet).await?;
                let _ = fetch_media_assets(data_dir, &tweet).await?;
                let _ = post_tweet_to_nostr(
                    data_dir,
                    Some(bearer_token),
                    mnemonic,
                    tweet.id.as_str(),
                    relays,
                    blossom_servers,
                    false,
                    false,
                )
                .await;
            }
        }

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                break;
            }
            _ = tokio::time::sleep(interval) => {}
        }
    }
    Ok(())
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
                        ts.format(&time::format_description::well_known::Rfc3339)
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
}
