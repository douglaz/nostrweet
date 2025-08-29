use std::path::Path;

use anyhow::{Context, Result, bail};
use nostr_sdk::prelude::*;
use tracing::{debug, info};

use crate::{keys, nostr, nostr_profile, storage};

pub async fn execute(
    username: &str,
    relays: &[String],
    data_dir: &Path,
    mnemonic: Option<&str>,
) -> Result<()> {
    info!(
        "Attempting to post profile for user '{}' to Nostr.",
        username
    );

    // Find the latest profile for the user
    let latest_profile_path = storage::find_latest_user_profile(username, data_dir)
        .context("Failed to find latest user profile")?;

    let Some(profile_path) = latest_profile_path else {
        bail!(
            "No profile found for user '{username}'",
            username = username
        );
    };

    debug!(
        "Found latest profile at: {path}",
        path = profile_path.display()
    );

    // Load the user profile
    let user =
        storage::load_user_from_file(&profile_path).context("Failed to load user profile")?;

    // Get Nostr keys
    let keys = keys::get_keys_for_tweet(&user.id, mnemonic)?;

    // Initialize Nostr client
    let client = nostr::initialize_nostr_client(&keys, relays).await?;

    // Create metadata using the shared function
    let metadata = nostr_profile::build_nostr_metadata_from_user(&user, username);

    // Build the event
    let event = EventBuilder::metadata(&metadata)
        .sign(&keys)
        .await
        .context("Failed to build metadata event")?;

    // Save the event locally before publishing
    storage::save_nostr_event(&event, data_dir).context("Failed to save nostr event locally")?;

    // Publish the event
    let event_id = client
        .send_event(&event)
        .await
        .context("Failed to publish metadata to Nostr")?;

    info!("Successfully published profile for '{username}' to Nostr. Event ID: {event_id:?}");

    Ok(())
}
