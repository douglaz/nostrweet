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
    /// Directory to save the tweet and media
    #[arg(
        short,
        long,
        default_value = "./downloads",
        env = "NOSTRWEET_OUTPUT_DIR",
        global = true
    )]
    output_dir: PathBuf,

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
        #[arg(short, long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
        blossom_servers: Vec<String>,

        /// Nostr private key (hex format, without leading 0x)
        #[arg(short, long, env = "NOSTRWEET_PRIVATE_KEY")]
        private_key: Option<String>,

        /// Force overwrite of existing Nostr event
        #[arg(short, long)]
        force: bool,
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
        #[arg(short, long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
        blossom_servers: Vec<String>,

        /// Nostr private key (hex format, without leading 0x)
        #[arg(short, long, env = "NOSTRWEET_PRIVATE_KEY")]
        private_key: Option<String>,

        /// Force overwrite of existing Nostr events
        #[arg(short, long)]
        force: bool,
    },

    /// Post a user's latest cached profile to Nostr
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
        #[arg(short, long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
        blossom_servers: Vec<String>,

        /// Nostr private key (hex format, without leading 0x)
        #[arg(short, long, env = "NOSTRWEET_PRIVATE_KEY")]
        private_key: Option<String>,

        /// Force overwrite of existing Nostr event
        #[arg(short, long)]
        force: bool,
    },

    /// Post a user's latest cached profile to Nostr
    PostProfileToNostr {
        /// The Twitter username of the user to post.
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

        /// Nostr private key (hex format, without leading 0x)
        #[arg(short, long, env = "NOSTRWEET_PRIVATE_KEY")]
        private_key: Option<String>,
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

        /// Nostr private key (hex format, without leading 0x)
        #[arg(short, long, env = "NOSTRWEET_PRIVATE_KEY")]
        private_key: Option<String>,
    },

    /// Show a tweet's JSON and its Nostr event representation
    ShowTweet(commands::show_tweet::ShowTweetCommand),
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

    // Make sure output directory exists
    if !args.output_dir.exists() {
        std::fs::create_dir_all(&args.output_dir).context("Failed to create output directory")?;
        info!(
            "Created output directory: {path}",
            path = args.output_dir.display()
        );
    }

    // Handle subcommands
    match args.command {
        Commands::FetchProfile { username } => {
            commands::fetch_profile::execute(&username, &args.output_dir).await?
        }
        Commands::FetchTweet { tweet_url_or_id } => {
            commands::fetch_tweet::execute(&tweet_url_or_id, &args.output_dir).await?
        }
        Commands::UserTweets {
            username,
            count,
            days,
        } => commands::user_tweets::execute(&username, &args.output_dir, Some(count), days).await?,
        Commands::ListTweets => commands::list_tweets::execute(&args.output_dir).await?,
        Commands::ClearCache { force } => {
            commands::clear_cache::execute(&args.output_dir, force).await?
        }
        Commands::PostTweetToNostr {
            tweet_url_or_id,
            relays,
            blossom_servers,
            private_key,
            force,
        } => {
            commands::post_tweet_to_nostr::execute(
                &tweet_url_or_id,
                &relays,
                &blossom_servers,
                private_key.as_deref(),
                &args.output_dir,
                force,
            )
            .await?
        }
        Commands::PostUserToNostr {
            username,
            relays,
            blossom_servers,
            private_key,
            force,
        } => {
            commands::post_user_to_nostr::execute(
                &username,
                &relays,
                &blossom_servers,
                private_key.as_deref(),
                &args.output_dir,
                force,
            )
            .await?
        }
        Commands::PostTweet {
            tweet_url_or_id,
            relays,
            blossom_servers,
            private_key,
            force,
        } => {
            commands::post_tweet::execute(
                &tweet_url_or_id,
                &relays,
                &blossom_servers,
                private_key.as_deref(),
                &args.output_dir,
                force,
            )
            .await?
        }
        Commands::PostProfileToNostr {
            username,
            relays,
            private_key,
        } => {
            commands::post_profile_to_nostr::execute(
                &username,
                &relays,
                private_key.as_deref(),
                &args.output_dir,
            )
            .await?
        }
        Commands::UpdateRelayList {
            relays,
            private_key,
        } => commands::update_relay_list::execute(&relays, private_key.as_deref()).await?,
        Commands::ShowTweet(cmd) => cmd.execute(&args.output_dir).await?,
    }

    Ok(())
}
