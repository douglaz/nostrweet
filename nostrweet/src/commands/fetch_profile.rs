use anyhow::{Context, Result};
use std::path::Path;
use tracing::info;

use crate::storage;
use crate::twitter;

/// Fetch a user's profile and save it to a file
pub async fn execute(username: &str, data_dir: &Path, bearer_token: &str) -> Result<()> {
    info!("Downloading profile for {username}");

    let client = twitter::TwitterClient::new(data_dir, bearer_token)
        .context("Failed to initialize Twitter client")?;
    let user = client
        .get_user_by_username(username)
        .await
        .context("Failed to download profile")?;

    let saved_path =
        storage::save_user_profile(&user, data_dir).context("Failed to save user profile")?;

    info!(
        "Successfully saved profile for {username} to {path}",
        path = saved_path.display()
    );

    Ok(())
}
