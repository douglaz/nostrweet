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
    /// Directory to save the tweet and media
    #[arg(short, long, env = "NOSTRWEET_OUTPUT_DIR", global = true)]
    output_dir: Option<PathBuf>,

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
        #[arg(short, long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
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
        #[arg(short, long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
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
        #[arg(short, long, value_delimiter = ',', env = "NOSTRWEET_BLOSSOM_SERVERS")]
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
            long,
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
        #[arg(short = 'b', long = "blossom-server", action = clap::ArgAction::Append)]
        blossom_servers: Vec<String>,

        /// Seconds between polling cycles
        #[arg(short, long, default_value = "300")]
        poll_interval: u64,

        /// Maximum concurrent users to process
        #[arg(short = 'c', long, default_value = "3")]
        max_concurrent: usize,
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

    // Get output directory, ensuring it's provided
    let output_dir = args.output_dir.context(
        "Output directory not specified. Please set --output-dir or NOSTRWEET_OUTPUT_DIR environment variable"
    )?;

    // Make sure output directory exists
    if !output_dir.exists() {
        std::fs::create_dir_all(&output_dir).context("Failed to create output directory")?;
        info!(
            "Created output directory: {path}",
            path = output_dir.display()
        );
    }

    // Handle subcommands
    match args.command {
        Commands::FetchProfile { username } => {
            commands::fetch_profile::execute(&username, &output_dir).await?
        }
        Commands::FetchTweet {
            tweet_url_or_id,
            skip_profiles,
        } => commands::fetch_tweet::execute(&tweet_url_or_id, &output_dir, skip_profiles).await?,
        Commands::UserTweets {
            username,
            count,
            days,
            skip_profiles,
        } => {
            commands::user_tweets::execute(&username, &output_dir, Some(count), days, skip_profiles)
                .await?
        }
        Commands::ListTweets => commands::list_tweets::execute(&output_dir).await?,
        Commands::ClearCache { force } => {
            commands::clear_cache::execute(&output_dir, force).await?
        }
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
                &output_dir,
                force,
                skip_profiles,
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
                &output_dir,
                force,
                skip_profiles,
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
                &output_dir,
                force,
                skip_profiles,
            )
            .await?
        }
        Commands::PostProfileToNostr { username, relays } => {
            commands::post_profile_to_nostr::execute(&username, &relays, &output_dir).await?
        }
        Commands::UpdateRelayList { relays } => {
            commands::update_relay_list::execute(&relays).await?
        }
        Commands::ShowTweet(cmd) => cmd.execute(&output_dir).await?,
        Commands::Daemon {
            users,
            relays,
            blossom_servers,
            poll_interval,
            max_concurrent: _,
        } => {
            commands::daemon::execute(users, relays, blossom_servers, poll_interval, &output_dir)
                .await?
        }
    }

    Ok(())
}
