use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use std::path::PathBuf;
use tracing::{debug, info};
use tracing_subscriber::{filter::EnvFilter, fmt, prelude::*};

mod commands;
mod datetime_utils;
mod error_utils;
mod filename_utils;
mod keys;
mod media;
mod nostr;
mod nostr_linking;
mod nostr_profile;
mod profile_collector;
mod storage;
mod twitter;

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
    #[arg(short, long = "data-dir", env = "NOSTRWEET_DATA_DIR", global = true)]
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
        #[arg(short, long, default_value = "10")]
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
            short,
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
            short,
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
            short,
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
            short,
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
            short,
            long,
            required = true,
            value_delimiter = ',',
            env = "NOSTRWEET_RELAYS"
        )]
        relays: Vec<String>,
    },

    /// Show a tweet's JSON and its Nostr event representation
    ShowTweet(commands::show_tweet::ShowTweetCommand),

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
enum UtilsCommands {
    /// Query events from Nostr relays
    QueryEvents {
        /// Nostr relay addresses to query from
        #[arg(short, long = "relay", required = true, action = clap::ArgAction::Append)]
        relays: Vec<String>,

        /// Filter by event kind (e.g., 0 for metadata, 1 for text notes)
        #[arg(short, long)]
        kind: Option<u32>,

        /// Filter by author public key (hex or npub format)
        #[arg(short, long)]
        author: Option<String>,

        /// Maximum number of events to retrieve
        #[arg(short, long, default_value = "10")]
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

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    // Initialize logging
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(filter)
        .init();

    // Parse command line arguments
    let args = Cli::parse();

    // Set up verbose logging if requested
    if args.verbose {
        debug!("Verbose mode enabled");
    }

    // Get data directory, ensuring it's provided (check old env var for backwards compatibility)
    let data_dir = args.data_dir
        .or_else(|| std::env::var("NOSTRWEET_OUTPUT_DIR").ok().map(PathBuf::from))
        .context(
            "Data directory not specified. Please set --data-dir or NOSTRWEET_DATA_DIR environment variable"
        )?;

    // Make sure data directory exists
    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir).context("Failed to create data directory")?;
        info!("Created data directory: {path}", path = data_dir.display());
    }

    // Determine if we need bearer token for the current command
    let needs_bearer_token = matches!(
        &args.command,
        Commands::FetchProfile { .. }
            | Commands::FetchTweet { .. }
            | Commands::UserTweets { .. }
            | Commands::Daemon { .. }
    );

    // Get bearer token if needed
    let bearer_token = if needs_bearer_token {
        Some(args.bearer_token.context(
            "Twitter bearer token not specified. Please set --bearer-token or TWITTER_BEARER_TOKEN environment variable"
        )?)
    } else {
        args.bearer_token
    };

    // Handle subcommands
    match args.command {
        Commands::FetchProfile { username } => {
            commands::fetch_profile::execute(&username, &data_dir, bearer_token.as_deref().unwrap())
                .await?
        }
        Commands::FetchTweet {
            tweet_url_or_id,
            skip_profiles,
        } => {
            commands::fetch_tweet::execute(
                &tweet_url_or_id,
                &data_dir,
                skip_profiles,
                bearer_token.as_deref().unwrap(),
            )
            .await?
        }
        Commands::UserTweets {
            username,
            count,
            days,
            skip_profiles,
        } => {
            commands::user_tweets::execute(
                &username,
                &data_dir,
                Some(count),
                days,
                skip_profiles,
                bearer_token.as_deref().unwrap(),
            )
            .await?
        }
        Commands::ListTweets => commands::list_tweets::execute(&data_dir).await?,
        Commands::ClearCache { force } => commands::clear_cache::execute(&data_dir, force).await?,
        Commands::PostTweetToNostr {
            tweet_url_or_id,
            relays,
            blossom_servers,
            force,
            skip_profiles,
        } => {
            commands::post_tweet_to_nostr::execute(
                &tweet_url_or_id,
                &relays,
                &blossom_servers,
                &data_dir,
                force,
                skip_profiles,
                args.mnemonic.as_deref(),
                bearer_token.as_deref(),
            )
            .await?
        }
        Commands::PostUserToNostr {
            username,
            relays,
            blossom_servers,
            force,
            skip_profiles,
        } => {
            commands::post_user_to_nostr::execute(
                &username,
                &relays,
                &blossom_servers,
                &data_dir,
                force,
                skip_profiles,
                args.mnemonic.as_deref(),
            )
            .await?
        }
        Commands::PostTweet {
            tweet_url_or_id,
            relays,
            blossom_servers,
            force,
            skip_profiles,
        } => {
            commands::post_tweet::execute(
                &tweet_url_or_id,
                &relays,
                &blossom_servers,
                &data_dir,
                force,
                skip_profiles,
                args.mnemonic.as_deref(),
                bearer_token.as_deref(),
            )
            .await?
        }
        Commands::PostProfileToNostr { username, relays } => {
            commands::post_profile_to_nostr::execute(
                &username,
                &relays,
                &data_dir,
                args.mnemonic.as_deref(),
            )
            .await?
        }
        Commands::UpdateRelayList { relays } => {
            commands::update_relay_list::execute(&relays, args.mnemonic.as_deref()).await?
        }
        Commands::ShowTweet(cmd) => cmd.execute(&data_dir, bearer_token.as_deref()).await?,
        Commands::Daemon {
            users,
            relays,
            blossom_servers,
            poll_interval,
        } => {
            commands::daemon::execute(
                users,
                relays,
                blossom_servers,
                poll_interval,
                &data_dir,
                args.mnemonic.as_deref(),
                bearer_token.as_deref().unwrap(),
            )
            .await?
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
                commands::utils::query_events(
                    relays, kind, author, limit, since, until, format, output,
                )
                .await?
            }
        },
    }

    Ok(())
}
