use anyhow::{Context, Result};
use nostr_sdk::{prelude::*, Metadata};
use std::collections::HashSet;
use std::path::Path;
use tracing::{debug, info};

use crate::{keys, storage, twitter};

/// Generate the profile disclaimer text for a given username
fn get_profile_disclaimer(username: &str) -> String {
    format!(
        "\n\nThis account is a mirror of https://x.com/{username}\n\nMirror created using nostrweet: https://github.com/douglaz/nostrweet"
    )
}

/// Build Nostr metadata from a Twitter user
pub fn build_nostr_metadata_from_user(user: &twitter::User, username: &str) -> Metadata {
    let mut metadata = Metadata::new();

    if let Some(name) = &user.name {
        metadata = metadata.name(name);
    }

    let disclaimer = get_profile_disclaimer(username);
    let about = match &user.description {
        Some(d) => format!("{d}{disclaimer}"),
        None => disclaimer,
    };
    metadata = metadata.about(&about);

    if let Some(profile_image_url) = &user.profile_image_url {
        if let Ok(url) = Url::parse(profile_image_url) {
            metadata = metadata.picture(url);
        }
    }

    if let Some(url_str) = &user.url {
        if let Ok(url) = Url::parse(url_str) {
            metadata = metadata.website(url);
        }
    }

    metadata
}

/// Posts a Twitter user profile to Nostr as metadata
async fn post_single_profile(
    username: &str,
    client: &nostr_sdk::Client,
    output_dir: &Path,
) -> Result<EventId> {
    debug!("Attempting to post profile for @{username} to Nostr");

    // Find the latest profile for the user
    let profile_path = storage::find_latest_user_profile(username, output_dir)?
        .ok_or_else(|| anyhow::anyhow!("No profile found for user '{username}'"))?;

    debug!(
        "Found profile for @{username} at {path}",
        path = profile_path.display()
    );

    // Load the user profile
    let user = storage::load_user_from_file(&profile_path)
        .with_context(|| format!("Failed to load profile for @{username}"))?;

    // Get Nostr keys for this user
    let user_keys = keys::get_keys_for_tweet(&user.id)?;

    // Create metadata using the shared function
    let metadata = build_nostr_metadata_from_user(&user, username);

    // Build the event
    let event = EventBuilder::metadata(&metadata)
        .sign(&user_keys)
        .await
        .context("Failed to build metadata event")?;

    // Save the event locally
    storage::save_nostr_event(&event, output_dir)
        .context("Failed to save nostr profile event locally")?;

    // Publish the event
    let output = client
        .send_event(&event)
        .await
        .with_context(|| format!("Failed to publish profile for @{username} to Nostr"))?;

    // Extract the event ID from the output
    let event_id = *output.id();

    debug!("Successfully published profile for @{username} with event ID: {event_id:?}");

    Ok(event_id)
}

/// Posts profiles for all referenced users in a tweet to Nostr
/// Returns the number of profiles successfully posted
pub async fn post_referenced_profiles(
    usernames: &HashSet<String>,
    client: &nostr_sdk::Client,
    output_dir: &Path,
) -> Result<usize> {
    if usernames.is_empty() {
        return Ok(0);
    }

    info!(
        "Posting {} referenced user profiles to Nostr",
        usernames.len()
    );

    let mut posted_count = 0;
    let mut failed_count = 0;

    for username in usernames {
        match post_single_profile(username, client, output_dir).await {
            Ok(event_id) => {
                debug!("Posted profile for @{username} with event ID: {event_id:?}");
                posted_count += 1;
            }
            Err(e) => {
                // Log the error but continue with other profiles
                debug!("Failed to post profile for @{username}: {e}");
                failed_count += 1;
            }
        }
    }

    if failed_count > 0 {
        debug!("Posted {posted_count} profiles, {failed_count} failed",);
    }

    if posted_count > 0 {
        info!("Successfully posted {posted_count} user profiles to Nostr");
    }

    Ok(posted_count)
}

/// Check which profiles need to be posted (not already posted or outdated)
/// Returns a set of usernames that should be posted
pub async fn filter_profiles_to_post(
    usernames: HashSet<String>,
    client: &nostr_sdk::Client,
    output_dir: &Path,
    force: bool,
) -> Result<HashSet<String>> {
    if force {
        // If force flag is set, post all profiles
        return Ok(usernames);
    }

    let mut profiles_to_post = HashSet::new();

    for username in usernames {
        // Check if profile exists locally
        if storage::find_latest_user_profile(&username, output_dir)?.is_none() {
            debug!("Profile for @{username} not found locally, skipping");
            continue;
        }

        // Load the user profile to get the user ID
        let profile_path = storage::find_latest_user_profile(&username, output_dir)?
            .ok_or_else(|| anyhow::anyhow!("Profile path disappeared for {username}"))?;

        let user = storage::load_user_from_file(&profile_path)?;
        let user_keys = keys::get_keys_for_tweet(&user.id)?;
        let pubkey = user_keys.public_key();

        // Check if we've already posted a profile for this user
        // Query for metadata events (Kind 0) from this pubkey
        let filter = Filter::new().author(pubkey).kind(Kind::Metadata).limit(1);

        // Try to query the database for existing events
        let has_existing_profile = match client.database().query(filter).await {
            Ok(events) => !events.is_empty(),
            Err(_) => false, // Assume no profile if we can't query
        };

        if !has_existing_profile {
            debug!("No existing profile found for @{username}, will post");
            profiles_to_post.insert(username);
        } else {
            debug!("Profile already exists for @{username}, skipping");
        }
    }

    Ok(profiles_to_post)
}
