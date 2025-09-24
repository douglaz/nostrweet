use anyhow::{Context, Result, bail, ensure};
// No Keys import needed as we're using it through the keys module
use std::path::Path;
use tokio::fs;
use tracing::{debug, info};

// Import necessary types from nostr_sdk
use nostr_sdk::{EventBuilder, Kind, Tag};

use crate::datetime_utils::parse_rfc3339;
use crate::keys;
use crate::media;
use crate::nostr;
use crate::nostr_profile;
use crate::profile_collector;
use crate::storage;
use crate::twitter;

/// Creates tags for a Nostr event including original and Blossom media URLs and mentions
pub fn create_nostr_event_tags(
    tweet_id: &str,
    orig_urls: &[String],
    blossom_urls: &[String],
    mentioned_pubkeys: &[nostr_sdk::PublicKey],
) -> Result<Vec<Tag>> {
    let mut tags = Vec::new();

    // reference original tweet
    let twitter_url = crate::nostr::build_twitter_status_url(tweet_id);
    tags.push(Tag::parse(vec!["r", twitter_url.as_str()])?);

    // Add p-tags for mentioned users
    for pubkey in mentioned_pubkeys {
        tags.push(Tag::parse(vec!["p", &pubkey.to_hex()])?);
    }

    // media tagging: if no blossom uploads, tag original URLs as media
    if blossom_urls.is_empty() {
        for orig in orig_urls {
            tags.push(Tag::parse(vec!["media", orig.as_str()])?);
        }
    } else {
        // tag source and uploaded media URLs
        for (orig, bloss) in orig_urls.iter().zip(blossom_urls.iter()) {
            tags.push(Tag::parse(vec!["source", orig.as_str()])?);
            tags.push(Tag::parse(vec!["media", bloss.as_str()])?);
        }
    }

    // Add client identifier
    tags.push(Tag::parse(vec!["client", "nostrweet"])?);

    Ok(tags)
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    tweet_url_or_id: &str,
    relays: &[String],
    blossom_servers: &[String],
    data_dir: &Path,
    force: bool,
    skip_profiles: bool,
    mnemonic: Option<&str>,
    bearer_token: Option<&str>,
) -> Result<()> {
    // Parse tweet ID from URL or ID string
    let tweet_id = twitter::parse_tweet_id(tweet_url_or_id)
        .with_context(|| format!("Failed to parse tweet ID from {tweet_url_or_id}"))?;

    info!("Processing tweet {tweet_id} for Nostr publishing");

    // Check if we already have a Nostr event for this tweet
    if let Some(event_info) = nostr::check_existing_nostr_event(data_dir, &tweet_id).await? {
        if force {
            info!(
                "Force flag enabled: Overwriting existing Nostr event for tweet {tweet_id} (previous event ID: {event_id})",
                event_id = event_info.event_id
            );
        } else {
            info!(
                "Tweet {tweet_id} was already posted to Nostr with event ID: {event_id}",
                event_id = event_info.event_id
            );
            info!("Use --force flag to overwrite the existing event");
            return Ok(());
        }
    }

    // First, check if we have already downloaded the tweet
    let mut tweet =
        if let Some(existing_path) = storage::find_existing_tweet_json(&tweet_id, data_dir) {
            debug!(
                "Found existing tweet data at {path}",
                path = existing_path.display()
            );
            storage::load_tweet_from_file(&existing_path)
                .with_context(|| format!("Failed to load existing tweet data for {tweet_id}"))?
        } else {
            // If not, download it first
            debug!("Tweet {tweet_id} not found locally, downloading it first");

            let bearer = bearer_token
                .ok_or_else(|| anyhow::anyhow!("Bearer token required to download tweet"))?;
            let client = twitter::TwitterClient::new(data_dir, bearer)
                .context("Failed to initialize Twitter client")?;

            let tweet = client
                .get_tweet(&tweet_id)
                .await
                .with_context(|| format!("Failed to download tweet {tweet_id}"))?;

            // Save the tweet locally
            let saved_path = storage::save_tweet(&tweet, data_dir)
                .with_context(|| format!("Failed to save tweet data for {tweet_id}"))?;
            debug!("Saved tweet data to {path}", path = saved_path.display());

            tweet
        };

    // Check if any referenced tweets are available in the local cache
    if tweet.referenced_tweets.is_some() {
        // First try to load referenced tweets from cache without creating a client
        if let Some(ref_tweets) = &mut tweet.referenced_tweets {
            for ref_tweet in ref_tweets {
                if ref_tweet.data.is_none() {
                    debug!(
                        "Looking for referenced tweet {id} in data_dir: {dir}",
                        id = ref_tweet.id,
                        dir = data_dir.display()
                    );

                    // First check in the current output directory
                    if let Some(path) = storage::find_existing_tweet_json(&ref_tweet.id, data_dir) {
                        match storage::load_tweet_from_file(&path) {
                            Ok(referenced_tweet) => {
                                debug!(
                                    "Found referenced tweet {id} in cache: {path}",
                                    id = ref_tweet.id,
                                    path = path.display()
                                );
                                ref_tweet.data = Some(Box::new(referenced_tweet));
                            }
                            Err(e) => {
                                debug!(
                                    "Error loading referenced tweet {id} from cache: {e}",
                                    id = ref_tweet.id
                                );
                                // Continue without this referenced tweet
                            }
                        }
                    } else {
                        debug!(
                            "Referenced tweet {id} not found in cache, continuing without it",
                            id = ref_tweet.id
                        );
                    }
                }
            }
        }

        // No need to use TwitterClient for referenced tweets - we've already checked the cache
    }

    // Extract Twitter user ID from the tweet data
    let twitter_user_id = tweet.author.id.clone();
    ensure!(
        !twitter_user_id.is_empty(),
        "Tweet author ID is missing in the Twitter data"
    );

    debug!("Using Twitter user ID: {twitter_user_id}");

    // Initialize Nostr keys - either from provided private key or derive from Twitter user ID
    let keys = keys::get_keys_for_tweet(&twitter_user_id, mnemonic)?;

    debug!(
        "Using Nostr public key: {pubkey}",
        pubkey = keys.public_key().to_string()
    );

    // Extract all media URLs from tweet and referenced tweets
    let mut tweet_media_urls = media::extract_media_urls_from_tweet(&tweet);

    // Check if we might need to fetch extended media information (if we suspect a video but don't have a direct URL)
    let mut need_extended_media = false;
    if tweet_media_urls.is_empty()
        || tweet_media_urls
            .iter()
            .any(|url| url.contains("twitter.com") || url.contains("x.com"))
    {
        if let Some(entities) = &tweet.entities {
            if let Some(urls) = &entities.urls {
                for url_entity in urls {
                    let expanded_url = url_entity.expanded_url.as_ref().unwrap_or(&url_entity.url);
                    if expanded_url.contains("video")
                        && (expanded_url.contains("twitter.com") || expanded_url.contains("x.com"))
                    {
                        need_extended_media = true;
                        debug!(
                            "Tweet contains video reference but no direct media: {expanded_url}"
                        );
                        break;
                    }
                }
            }
        }
    }

    // If we suspect there's a video but don't have direct media URLs, fetch extended media
    if need_extended_media {
        debug!("Fetching extended media information for tweet {tweet_id}");
        if let Some(bearer) = bearer_token {
            let twitter_client_result = twitter::TwitterClient::new(data_dir, bearer);
            if let Ok(twitter_client_instance) = twitter_client_result {
                let extended_tweet = twitter_client_instance
                    .get_tweet_with_media(&tweet_id)
                    .await
                    .context("Failed to fetch tweet with extended media")?;

                // Re-extract media URLs from the extended tweet information
                tweet_media_urls = media::extract_media_urls_from_tweet(&extended_tweet);
                debug!(
                    "After fetching extended media, found {} media URLs",
                    tweet_media_urls.len()
                );
            } else {
                debug!("Failed to initialize Twitter client for extended media fetch");
            }
        }
    }

    // Log media URLs that were found
    if !tweet_media_urls.is_empty() {
        debug!("Found media URLs in tweet:");
        for (i, url) in tweet_media_urls.iter().enumerate() {
            debug!("  {i}: {url}");
        }
    } else {
        debug!("No media URLs found in tweet");
    }

    // Locate or download media files in flat data_dir, fallback to nested tweets dir
    let mut media_files = Vec::new();
    for url in &tweet_media_urls {
        let filename = url.split('/').next_back().unwrap_or("media");
        let flat_path = data_dir.join(filename);
        let nested_path = data_dir.join("tweets").join(&tweet_id).join(filename);
        let file_path = if flat_path.exists() {
            flat_path.clone()
        } else if nested_path.exists() {
            nested_path.clone()
        } else {
            // Download into flat data_dir
            let resp = reqwest::get(url).await?;
            let bytes = resp.bytes().await?;
            fs::write(&flat_path, &bytes).await?;
            debug!(
                "Downloaded media for tweet {tweet_id} to {path}",
                path = flat_path.display()
            );
            flat_path.clone()
        };
        media_files.push(file_path);
    }

    // Upload media if blossom servers provided, else skip and use original URLs
    let blossom_urls = if !media_files.is_empty() && !blossom_servers.is_empty() {
        info!(
            "Uploading {count} media files to Blossom servers",
            count = media_files.len()
        );
        nostr::upload_media_to_blossom(&media_files, blossom_servers, &keys).await?
    } else {
        Vec::new()
    };

    // choose which URLs to include in content: fallback to original if no uploads
    let media_urls = if blossom_urls.is_empty() {
        tweet_media_urls.clone()
    } else {
        blossom_urls.clone()
    };

    // Create a resolver for Twitter username to Nostr pubkey mapping
    let mut resolver = crate::nostr_linking::NostrLinkResolver::new(
        Some(data_dir.to_string_lossy().to_string()),
        mnemonic.map(|s| s.to_string()),
    );

    // Format tweet content for Nostr with mention resolution
    let (content, mentioned_pubkeys) =
        nostr::format_tweet_as_nostr_content_with_mentions(&tweet, &media_urls, &mut resolver)?;

    // Create Nostr client and connect to relays
    let client = nostr::initialize_nostr_client(&keys, relays).await?;

    // Check if we've already published this tweet to any of the relays
    let existing_event = nostr::find_existing_event(&client, &tweet_id, &keys).await?;

    // Determine whether to use existing event or create a new one
    let create_new_event = if let Some(existing) = &existing_event {
        if force {
            info!(
                "Force flag enabled: Creating new Nostr event to replace existing one for tweet {tweet_id}"
            );
            true
        } else {
            info!(
                "Found existing Nostr event for tweet {tweet_id} with ID: {id}",
                id = existing.id.to_hex()
            );
            info!("Use --force flag to overwrite the existing event");
            false
        }
    } else {
        // No existing event found, always create a new one
        debug!("No existing event found, creating new Nostr event for tweet {tweet_id}");
        true
    };

    // Initialize variables for event tracking
    let (event_id, event_json) = if create_new_event {
        // Create and publish new event
        debug!("Creating new Nostr event for tweet {tweet_id}");

        // Parse the tweet's creation date
        // First check if the tweet has a creation date
        if tweet.created_at.is_empty() {
            bail!(
                "No creation date found in tweet - cannot create Nostr event without a valid timestamp"
            );
        }

        // Parse the ISO 8601 date into a timestamp
        let tweet_created_at = parse_rfc3339(&tweet.created_at)?.timestamp() as u64;

        debug!("Using tweet creation timestamp: {tweet_created_at}");

        use nostr_sdk::Timestamp;
        let timestamp = Timestamp::from(tweet_created_at);

        // Create tags for the event
        let tags = create_nostr_event_tags(
            &tweet_id,
            &tweet_media_urls,
            &blossom_urls,
            &mentioned_pubkeys,
        )?;

        // Create a fresh builder with all the necessary components
        let mut final_builder =
            EventBuilder::new(Kind::TextNote, content.clone()).custom_created_at(timestamp);

        // Add all the tags - use a reference to avoid moving the tags
        for tag in &tags {
            final_builder = final_builder.tag(tag.clone());
        }

        // Now sign it all at once with the keys
        // We need to use the entire keys object as it implements NostrSigner
        // Since we're already in an async context, we can just await directly
        let event = final_builder.sign(&keys).await?;

        // Save the event locally before publishing
        storage::save_nostr_event(&event, data_dir)
            .context("Failed to save nostr event locally")?;

        debug!("Successfully created event with tweet's original timestamp");
        debug!(
            "Original tweet timestamp: {timestamp} (unix: {unix_timestamp})",
            unix_timestamp = timestamp.as_u64()
        );
        debug!(
            "Event timestamp: {event_timestamp}",
            event_timestamp = event.created_at
        );

        // Now we have proper idempotency - same tweet will produce the same event ID

        // Publish to all relays
        nostr::publish_nostr_event(&client, &event).await?;

        let event_id_hex = event.id.to_hex();

        // Serialize the event to JSON
        let json = serde_json::to_string_pretty(&event)
            .context("Failed to serialize Nostr event to JSON")?;

        (event_id_hex, Some(json))
    } else if let Some(existing) = existing_event {
        // Use existing event ID and serialize it
        let event_id_hex = existing.id.to_hex();

        // Serialize the existing event to JSON
        let json = serde_json::to_string_pretty(&existing)
            .context("Failed to serialize existing Nostr event to JSON")?;

        (event_id_hex, Some(json))
    } else {
        // This shouldn't happen, but handle it gracefully
        debug!("No event object available");
        (String::new(), None)
    };

    // Create a record of the event for future reference
    // Extract the original event's timestamp from the JSON if available, otherwise fail
    // This ensures the metadata matches the actual event timestamp for consistency
    let event_timestamp = match &event_json {
        Some(json_str) => {
            // Try to parse the JSON to get the timestamp
            let json_value = serde_json::from_str::<serde_json::Value>(json_str)
                .context("Failed to parse Nostr event JSON")?;

            // Extract the created_at field from the JSON
            json_value
                .get("created_at")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| anyhow::anyhow!("No timestamp found in Nostr event JSON"))?
        }
        None => {
            // If we get here, we must have an event object with a valid timestamp
            // Otherwise the process should have failed earlier
            bail!("No event JSON available to extract timestamp");
        }
    };

    let event_info = nostr::NostrEventInfo {
        tweet_id: tweet_id.to_string(),
        event_id: event_id.to_string(),
        pubkey: keys.public_key().to_string(),
        created_at: event_timestamp,
        // include both original and Blossom media URLs
        media_urls: if blossom_urls.is_empty() {
            tweet_media_urls.clone()
        } else {
            tweet_media_urls
                .iter()
                .chain(blossom_urls.iter())
                .cloned()
                .collect()
        },
        relays: relays.to_vec(),
        event_json,
    };

    // Save event info to file
    let event_info_path = nostr::save_nostr_event_info(&event_info, data_dir).await?;
    debug!(
        "Saved Nostr event info to {path}",
        path = event_info_path.display()
    );

    info!("Successfully posted tweet {tweet_id} to Nostr with event ID: {event_id}");

    // Post profiles for all referenced users (unless skipped)
    if !skip_profiles {
        // Collect all referenced usernames from the tweet
        let usernames = profile_collector::collect_usernames_from_tweet(&tweet);

        if !usernames.is_empty() {
            debug!(
                "Found {} referenced users to potentially post profiles for",
                usernames.len()
            );

            // Filter profiles that need to be posted
            let profiles_to_post = nostr_profile::filter_profiles_to_post(
                usernames, &client, data_dir, force, mnemonic,
            )
            .await?;

            if !profiles_to_post.is_empty() {
                // Post the profiles
                let posted_count = nostr_profile::post_referenced_profiles(
                    &profiles_to_post,
                    &client,
                    data_dir,
                    mnemonic,
                )
                .await?;

                if posted_count > 0 {
                    info!("Posted {posted_count} referenced user profiles to Nostr");
                }
            } else {
                debug!("All referenced user profiles already posted or not available");
            }
        }
    } else {
        debug!("Skipping profile posting (--skip-profiles flag set)");
    }

    Ok(())
}
