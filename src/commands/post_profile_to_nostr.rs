use std::path::Path;

use anyhow::{bail, Context, Result};
use nostr_sdk::{prelude::*, Metadata};
use tracing::{debug, info};

use crate::{keys, nostr, storage};

pub async fn execute(username: &str, relays: &[String], output_dir: &Path) -> Result<()> {
    info!(
        "Attempting to post profile for user '{}' to Nostr.",
        username
    );

    // Find the latest profile for the user
    let latest_profile_path = storage::find_latest_user_profile(username, output_dir)
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
    let keys = keys::get_keys_for_tweet(&user.id)?;

    // Initialize Nostr client
    let client = nostr::initialize_nostr_client(&keys, relays).await?;

    // Create metadata
    let mut metadata = Metadata::new();
    if let Some(name) = user.name {
        metadata = metadata.name(name);
    }
    let disclaimer = format!("\n\nThis account is a mirror of https://x.com/{username}");
    let about = match user.description {
        Some(d) => format!("{d}{disclaimer}"),
        None => disclaimer,
    };
    metadata = metadata.about(&about);
    if let Some(profile_image_url) = user.profile_image_url {
        if let Ok(url) = Url::parse(&profile_image_url) {
            metadata = metadata.picture(url);
        }
    }
    if let Some(url) = user.url {
        if let Ok(url) = Url::parse(&url) {
            metadata = metadata.website(url);
        }
    }

    // Build the event
    let event = EventBuilder::metadata(&metadata)
        .sign(&keys)
        .await
        .context("Failed to build metadata event")?;

    // Save the event locally before publishing
    storage::save_nostr_event(&event, output_dir).context("Failed to save nostr event locally")?;

    // Publish the event
    let event_id = client
        .send_event(&event)
        .await
        .context("Failed to publish metadata to Nostr")?;

    info!("Successfully published profile for '{username}' to Nostr. Event ID: {event_id:?}");

    Ok(())
}
