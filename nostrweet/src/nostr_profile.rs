use anyhow::{Context, Result};
use nostr_sdk::{Filter, Kind, Metadata, prelude::*};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info};

use crate::{keys, nostr, storage, twitter};

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
    data_dir: &Path,
    mnemonic: Option<&str>,
) -> Result<EventId> {
    debug!("Attempting to post profile for @{username} to Nostr");

    // Find the latest profile for the user
    let profile_path = storage::find_latest_user_profile(username, data_dir)?
        .ok_or_else(|| anyhow::anyhow!("No profile found for user '{username}'"))?;

    debug!(
        "Found profile for @{username} at {path}",
        path = profile_path.display()
    );

    // Load the user profile
    let user = storage::load_user_from_file(&profile_path)
        .with_context(|| format!("Failed to load profile for @{username}"))?;

    // Get Nostr keys for this user
    let user_keys = keys::get_keys_for_tweet(&user.id, mnemonic)?;

    // Create metadata using the shared function
    let metadata = build_nostr_metadata_from_user(&user, username);

    // Build the event
    let event = EventBuilder::metadata(&metadata)
        .sign(&user_keys)
        .await
        .context("Failed to build metadata event")?;

    // Save the event locally
    storage::save_nostr_event(&event, data_dir)
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

/// Posts a relay list for a specific Twitter user
async fn post_relay_list_for_user(
    username: &str,
    client: &nostr_sdk::Client,
    data_dir: &Path,
    mnemonic: Option<&str>,
    relays: &[String],
) -> Result<EventId> {
    debug!("Posting relay list for @{username}");

    // Find the user profile to get the user ID
    let profile_path = storage::find_latest_user_profile(username, data_dir)?
        .ok_or_else(|| anyhow::anyhow!("No profile found for user '{username}'"))?;

    let user = storage::load_user_from_file(&profile_path)?;
    let user_keys = keys::get_keys_for_tweet(&user.id, mnemonic)?;

    // Use the existing update_relay_list function from the nostr module
    nostr::update_relay_list(client, &user_keys, relays)
        .await
        .with_context(|| format!("Failed to update relay list for @{username}"))?;

    debug!("Successfully posted relay list for @{username}");

    // Return a dummy event ID for now (the actual function doesn't return one)
    // In a real implementation, we might want to modify update_relay_list to return the event ID
    Ok(EventId::all_zeros())
}

/// Posts profiles for all referenced users in a tweet to Nostr
/// Returns the number of profiles successfully posted
pub async fn post_referenced_profiles(
    usernames: &HashSet<String>,
    client: &nostr_sdk::Client,
    data_dir: &Path,
    mnemonic: Option<&str>,
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
        match post_single_profile(username, client, data_dir, mnemonic).await {
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

/// Check if a profile exists for a specific user on Nostr
/// Returns true if a profile (metadata event) exists for the user
pub async fn check_profile_exists(
    username: &str,
    client: &nostr_sdk::Client,
    data_dir: &Path,
    mnemonic: Option<&str>,
) -> Result<bool> {
    debug!("Checking if profile exists for @{username}");

    // Check if profile exists locally first
    let profile_path = match storage::find_latest_user_profile(username, data_dir)? {
        Some(path) => path,
        None => {
            debug!("Profile for @{username} not found locally");
            return Ok(false);
        }
    };

    // Load the user profile to get the user ID
    let user = storage::load_user_from_file(&profile_path)?;
    let user_keys = keys::get_keys_for_tweet(&user.id, mnemonic)?;
    let pubkey = user_keys.public_key();

    // Query for metadata events (Kind 0) from this pubkey
    let filter = Filter::new().author(pubkey).kind(Kind::Metadata).limit(1);

    // Try to query the relays for existing events
    let events = client.fetch_events(filter, Duration::from_secs(10)).await?;
    let has_profile = !events.is_empty();

    if has_profile {
        debug!("Profile exists for @{username} on Nostr");
    } else {
        debug!("No profile found for @{username} on Nostr");
    }

    Ok(has_profile)
}

/// Check which profiles need to be posted (not already posted or outdated)
/// Returns a set of usernames that should be posted
pub async fn filter_profiles_to_post(
    usernames: HashSet<String>,
    client: &nostr_sdk::Client,
    data_dir: &Path,
    force: bool,
    mnemonic: Option<&str>,
) -> Result<HashSet<String>> {
    if force {
        // If force flag is set, post all profiles
        return Ok(usernames);
    }

    let mut profiles_to_post = HashSet::new();

    for username in usernames {
        // Check if profile exists locally
        if storage::find_latest_user_profile(&username, data_dir)?.is_none() {
            debug!("Profile for @{username} not found locally, skipping");
            continue;
        }

        // Load the user profile to get the user ID
        let profile_path = storage::find_latest_user_profile(&username, data_dir)?
            .ok_or_else(|| anyhow::anyhow!("Profile path disappeared for {username}"))?;

        let user = storage::load_user_from_file(&profile_path)?;
        let user_keys = keys::get_keys_for_tweet(&user.id, mnemonic)?;
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

/// Post a user profile and their relay list to Nostr
pub async fn post_user_profile_with_relay_list(
    username: &str,
    client: &nostr_sdk::Client,
    data_dir: &Path,
    mnemonic: Option<&str>,
    relays: &[String],
) -> Result<()> {
    info!("Posting profile and relay list for @{username}");

    // First post the profile
    let profile_event_id = post_single_profile(username, client, data_dir, mnemonic).await?;
    info!("Posted profile for @{username} with event ID: {profile_event_id:?}");

    // Then post the relay list
    post_relay_list_for_user(username, client, data_dir, mnemonic, relays).await?;
    info!("Posted relay list for @{username}");

    Ok(())
}
