use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

pub struct NostrRelay {
    process: Option<Child>,
    port: u16,
    #[allow(dead_code)]
    data_dir: TempDir,
    #[allow(dead_code)]
    config_path: PathBuf,
}

impl NostrRelay {
    /// Start a new nostr-rs-relay instance
    pub async fn start(port: u16) -> Result<Self> {
        info!("Starting nostr-rs-relay on port {port}");

        // Create temporary directory for relay data
        let data_dir = TempDir::new().context("Failed to create temp directory")?;
        let data_path = data_dir.path().to_path_buf();

        // Create config file
        let config_path = data_path.join("config.toml");
        let config_content = format!(
            r#"
[network]
port = {port}
address = "127.0.0.1"

[database]
data_directory = "{}"

[logging]
level = "info"

[limits]
messages_per_sec = 100
subscriptions_per_min = 100
"#,
            data_path.display()
        );

        std::fs::write(&config_path, config_content).context("Failed to write relay config")?;

        // Start relay process
        let mut cmd = Command::new("nostr-rs-relay");
        cmd.arg("--config")
            .arg(&config_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        debug!("Starting relay with command: {:?}", cmd);
        let process = cmd.spawn().context("Failed to start nostr-rs-relay")?;

        let relay = Self {
            process: Some(process),
            port,
            data_dir,
            config_path,
        };

        // Wait for relay to be ready
        relay.wait_until_ready().await?;

        Ok(relay)
    }

    /// Wait for the relay to be ready to accept connections
    async fn wait_until_ready(&self) -> Result<()> {
        info!("Waiting for relay to be ready...");

        let url = format!("http://127.0.0.1:{}", self.port);
        let max_wait = Duration::from_secs(30);
        let start = std::time::Instant::now();

        while start.elapsed() < max_wait {
            // Try to connect with HTTP
            match reqwest::get(&url).await {
                Ok(_) => {
                    info!("Relay is ready!");
                    return Ok(());
                }
                Err(e) => {
                    debug!("Relay not ready yet: {e}");
                    sleep(Duration::from_millis(500)).await;
                }
            }
        }

        bail!("Relay failed to start after 30 seconds");
    }

    /// Get the WebSocket URL for this relay
    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    /// Stop the relay
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(mut process) = self.process.take() {
            info!("Stopping relay...");

            // Try graceful shutdown first
            process.kill().await.ok();

            // Wait for process to exit
            match timeout(Duration::from_secs(5), process.wait()).await {
                Ok(_) => info!("Relay stopped gracefully"),
                Err(_) => {
                    warn!("Relay did not stop gracefully, force killing");
                    // Force kill is already done by kill_on_drop
                }
            }
        }

        Ok(())
    }

    /// Check if the relay is still running
    #[allow(dead_code)]
    pub async fn is_running(&mut self) -> bool {
        if let Some(ref mut process) = self.process {
            match process.try_wait() {
                Ok(Some(_)) => false, // Process has exited
                Ok(None) => true,     // Still running
                Err(_) => false,      // Error checking status
            }
        } else {
            false
        }
    }

    /// Get the data directory path
    #[allow(dead_code)]
    pub fn data_dir(&self) -> &std::path::Path {
        self.data_dir.path()
    }

    /// Get the config file path
    #[allow(dead_code)]
    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }
}

impl Drop for NostrRelay {
    fn drop(&mut self) {
        // Ensure process is killed when relay is dropped
        if let Some(mut process) = self.process.take() {
            // This will be handled by kill_on_drop, but we can try to be explicit
            let _ = process.start_kill();
        }
    }
}
