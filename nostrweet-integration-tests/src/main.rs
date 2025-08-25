use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod relay;
mod test_runner;
mod tests;

#[derive(Parser)]
#[command(name = "nostrweet-integration-tests")]
#[command(about = "Integration test suite for nostrweet", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Port for the Nostr relay
    #[arg(long, default_value_t = 8080)]
    relay_port: u16,

    /// Path to relay config file
    #[arg(long)]
    relay_config: Option<String>,

    /// Keep relay running after tests complete
    #[arg(long)]
    keep_relay: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Run all integration tests
    RunAll,

    /// Run a specific test
    Run {
        /// Name of the test to run
        #[arg(long)]
        test: String,
    },

    /// Clean up test artifacts
    Cleanup,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging to stderr
    let filter_level = if cli.verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter_level));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();

    match cli.command {
        Commands::RunAll => {
            info!("Running all integration tests");
            test_runner::run_all_tests(cli.relay_port, cli.keep_relay).await?;
        }
        Commands::Run { test } => {
            info!("Running test: {test}");
            test_runner::run_single_test(&test, cli.relay_port, cli.keep_relay).await?;
        }
        Commands::Cleanup => {
            info!("Cleaning up test artifacts");
            test_runner::cleanup().await?;
        }
    }

    Ok(())
}
