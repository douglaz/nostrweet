use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{error, info, warn};

use crate::relay::NostrRelay;
use crate::tests;

/// Type alias for test function
type TestFn =
    fn(&TestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + '_>>;

/// Test metadata
pub struct TestInfo {
    pub name: String,
    pub description: String,
    pub run_fn: TestFn,
}

/// Context provided to each test
pub struct TestContext {
    pub relay_url: String,
    pub output_dir: PathBuf,
    pub private_key: String,
    pub nostrweet_binary: PathBuf,
    pub twitter_token: String,
    pub mnemonic: String,
}

impl TestContext {
    /// Run a nostrweet command
    pub async fn run_nostrweet(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new(&self.nostrweet_binary);

        // Add common environment variables
        cmd.env("NOSTRWEET_OUTPUT_DIR", &self.output_dir)
            .env("NOSTRWEET_PRIVATE_KEY", &self.private_key)
            .env("NOSTRWEET_RELAYS", &self.relay_url)
            .env("TWITTER_BEARER_TOKEN", &self.twitter_token)
            .env("NOSTRWEET_MNEMONIC", &self.mnemonic);

        // Add arguments
        for arg in args {
            cmd.arg(arg);
        }

        // Inherit both stdout and stderr directly to parent for immediate visibility
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        info!("Running: {:?}", cmd);

        // Apply a 120-minute timeout to allow for Twitter API rate limit retries
        let status = match timeout(Duration::from_secs(7200), cmd.status()).await {
            Ok(result) => result.context("Failed to run nostrweet")?,
            Err(_) => {
                bail!(
                    "Command timed out after 120 minutes. This may indicate a Twitter API issue."
                );
            }
        };

        if !status.success() {
            bail!("Command failed with exit code {:?}", status.code());
        }

        // Since we're inheriting stdout, we can't capture the output
        // Tests that need to verify output should use run_nostrweet_with_output
        Ok(String::new())
    }

    /// Check if a file exists in the output directory
    #[allow(dead_code)]
    pub fn file_exists(&self, pattern: &str) -> bool {
        let pattern_path = self.output_dir.join(pattern);
        std::fs::metadata(pattern_path).is_ok()
    }

    /// Read a JSON file from the output directory
    #[allow(dead_code)]
    pub fn read_json<T: serde::de::DeserializeOwned>(&self, filename: &str) -> Result<T> {
        let path = self.output_dir.join(filename);
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse JSON from {}", path.display()))
    }

    /// Run a nostrweet command and return its output
    /// This captures stdout for verification while still showing stderr
    pub async fn run_nostrweet_with_output(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new(&self.nostrweet_binary);

        // Add common environment variables
        cmd.env("NOSTRWEET_OUTPUT_DIR", &self.output_dir)
            .env("NOSTRWEET_PRIVATE_KEY", &self.private_key)
            .env("NOSTRWEET_RELAYS", &self.relay_url)
            .env("TWITTER_BEARER_TOKEN", &self.twitter_token)
            .env("NOSTRWEET_MNEMONIC", &self.mnemonic);

        // Add arguments
        for arg in args {
            cmd.arg(arg);
        }

        // Capture stdout but inherit stderr for debugging
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());

        info!("Running (with output capture): {:?}", cmd);

        // Apply a 120-minute timeout to allow for Twitter API rate limit retries
        let output = match timeout(Duration::from_secs(7200), cmd.output()).await {
            Ok(result) => result.context("Failed to run nostrweet")?,
            Err(_) => {
                bail!(
                    "Command timed out after 120 minutes. This may indicate a Twitter API issue."
                );
            }
        };

        if !output.status.success() {
            bail!("Command failed with exit code {:?}", output.status.code());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Get all available tests
fn get_tests() -> Vec<TestInfo> {
    vec![
        TestInfo {
            name: "tweet_fetch".to_string(),
            description: "Test fetching and posting a tweet".to_string(),
            run_fn: |ctx| Box::pin(tests::tweet_fetch::run(ctx)),
        },
        TestInfo {
            name: "profile_sync".to_string(),
            description: "Test fetching and posting a profile".to_string(),
            run_fn: |ctx| Box::pin(tests::profile::run(ctx)),
        },
        TestInfo {
            name: "daemon_mode".to_string(),
            description: "Test daemon mode functionality".to_string(),
            run_fn: |ctx| Box::pin(tests::daemon::run(ctx)),
        },
        TestInfo {
            name: "nostr_post".to_string(),
            description: "Test posting to Nostr relay".to_string(),
            run_fn: |ctx| Box::pin(tests::nostr_post::run(ctx)),
        },
        TestInfo {
            name: "user_tweets".to_string(),
            description: "Test fetching multiple tweets from user timeline".to_string(),
            run_fn: |ctx| Box::pin(tests::user_tweets::run(ctx)),
        },
        TestInfo {
            name: "batch_post".to_string(),
            description: "Test batch posting user tweets to Nostr".to_string(),
            run_fn: |ctx| Box::pin(tests::batch_post::run(ctx)),
        },
        TestInfo {
            name: "cache_management".to_string(),
            description: "Test cache listing and clearing commands".to_string(),
            run_fn: |ctx| Box::pin(tests::cache_management::run(ctx)),
        },
        TestInfo {
            name: "utils_query".to_string(),
            description: "Test querying events from Nostr relay".to_string(),
            run_fn: |ctx| Box::pin(tests::utils_query::run(ctx)),
        },
    ]
}

/// Run all integration tests
pub async fn run_all_tests(
    relay_port: u16,
    keep_relay: bool,
    twitter_token: &str,
    mnemonic: &str,
) -> Result<()> {
    let tests = get_tests();
    let mut results = HashMap::new();
    let mut relay = None;

    // Start relay once if keeping it running
    if keep_relay {
        relay = Some(NostrRelay::start(relay_port).await?);
    }

    for test in tests {
        info!("Running test: {} - {}", test.name, test.description);

        // Start new relay for each test if not keeping
        let test_relay = if keep_relay {
            None
        } else {
            Some(NostrRelay::start(relay_port).await?)
        };

        let relay_url = if let Some(ref r) = relay {
            r.ws_url()
        } else if let Some(ref r) = test_relay {
            r.ws_url()
        } else {
            bail!("No relay available");
        };

        // Create test context
        let temp_dir = TempDir::new()?;
        let ctx = TestContext {
            relay_url,
            output_dir: temp_dir.path().to_path_buf(),
            private_key: hex::encode(rand::random::<[u8; 32]>()),
            nostrweet_binary: find_nostrweet_binary()?,
            twitter_token: twitter_token.to_string(),
            mnemonic: mnemonic.to_string(),
        };

        // Run test
        let result = (test.run_fn)(&ctx).await;

        match result {
            Ok(_) => {
                info!("✅ Test {} passed", test.name);
                results.insert(test.name.clone(), true);
            }
            Err(e) => {
                error!("❌ Test {} failed: {e}", test.name);
                results.insert(test.name.clone(), false);
            }
        }

        // Stop test-specific relay
        if let Some(mut r) = test_relay {
            r.stop().await.ok();
        }
    }

    // Stop shared relay if it was started
    if let Some(mut r) = relay {
        if !keep_relay {
            r.stop().await.ok();
        }
    }

    // Print summary
    info!("\n=== Test Summary ===");
    let total = results.len();
    let passed = results.values().filter(|v| **v).count();
    let failed = total - passed;

    for (test_name, passed) in &results {
        let status = if *passed { "✅ PASS" } else { "❌ FAIL" };
        info!("{status}: {test_name}");
    }

    info!("\nTotal: {total}, Passed: {passed}, Failed: {failed}");

    if failed > 0 {
        bail!("{failed} tests failed");
    }

    Ok(())
}

/// Run a single test
pub async fn run_single_test(
    test_name: &str,
    relay_port: u16,
    keep_relay: bool,
    twitter_token: &str,
    mnemonic: &str,
) -> Result<()> {
    let tests = get_tests();
    let test = tests
        .into_iter()
        .find(|t| t.name == test_name)
        .with_context(|| format!("Test not found: {test_name}"))?;

    info!("Running test: {} - {}", test.name, test.description);

    // Start relay
    let mut relay = NostrRelay::start(relay_port).await?;
    let relay_url = relay.ws_url();

    // Create test context
    let temp_dir = TempDir::new()?;
    let ctx = TestContext {
        relay_url,
        output_dir: temp_dir.path().to_path_buf(),
        private_key: hex::encode(rand::random::<[u8; 32]>()),
        nostrweet_binary: find_nostrweet_binary()?,
        twitter_token: twitter_token.to_string(),
        mnemonic: mnemonic.to_string(),
    };

    // Run test
    let result = (test.run_fn)(&ctx).await;

    // Stop relay unless keeping it
    if !keep_relay {
        relay.stop().await.ok();
    }

    match result {
        Ok(_) => {
            info!("✅ Test {} passed", test.name);
            Ok(())
        }
        Err(e) => {
            error!("❌ Test {} failed: {e}", test.name);
            bail!("Test failed: {e}");
        }
    }
}

/// Clean up test artifacts
pub async fn cleanup() -> Result<()> {
    // Clean up any leftover processes
    warn!("Cleanup: killing any leftover nostr-rs-relay processes");

    let output = Command::new("pkill")
        .arg("-f")
        .arg("nostr-rs-relay")
        .output()
        .await
        .context("Failed to run pkill")?;

    if output.status.success() {
        info!("Killed leftover relay processes");
    } else {
        info!("No leftover relay processes found");
    }

    // Clean up temp directories
    let temp_dir = std::env::temp_dir();
    let pattern = temp_dir.join("nostrweet-test-*");

    if let Ok(entries) = glob::glob(&pattern.to_string_lossy()) {
        for entry in entries.flatten() {
            if let Err(e) = std::fs::remove_dir_all(&entry) {
                warn!("Failed to remove {}: {e}", entry.display());
            } else {
                info!("Removed {}", entry.display());
            }
        }
    }

    Ok(())
}

/// Find the nostrweet binary
fn find_nostrweet_binary() -> Result<PathBuf> {
    // First, check if we can find it in the target directory
    let workspace_root = std::env::current_dir()?
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Failed to find workspace root"))?
        .to_path_buf();

    let candidates = vec![
        workspace_root.join("target/debug/nostrweet"),
        workspace_root.join("target/release/nostrweet"),
        workspace_root.join("nostrweet/target/debug/nostrweet"),
        workspace_root.join("nostrweet/target/release/nostrweet"),
    ];

    for candidate in candidates {
        if candidate.exists() {
            info!("Found nostrweet binary at: {}", candidate.display());
            return Ok(candidate);
        }
    }

    // Try to find it in PATH
    if let Ok(output) = std::process::Command::new("which")
        .arg("nostrweet")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                info!("Found nostrweet in PATH: {path}");
                return Ok(PathBuf::from(path));
            }
        }
    }

    bail!("Could not find nostrweet binary. Please build it first with 'cargo build'");
}
