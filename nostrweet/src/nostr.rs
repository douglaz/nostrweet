use crate::nostr_linking::NostrLinkResolver;
use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use nostr_sdk::nips::nip65::RelayMetadata;
use nostr_sdk::ToBech32;
use nostr_sdk::{
    Alphabet, Client, Event, EventBuilder, Filter, Keys, Kind, PublicKey, RelayUrl,
    SingleLetterTag, SubscriptionId, Tag, Timestamp, Url,
};
use reqwest::{header::RETRY_AFTER, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::time::sleep;
use tokio::time::timeout;
use tracing::{debug, info, warn};
use url::Url as UrlParser;

/// Helper struct for formatting tweet content
struct TweetFormatter<'a> {
    tweet: &'a crate::twitter::Tweet,
    media_urls: &'a [String],
    resolver: &'a mut NostrLinkResolver,
}

/// Result of formatting tweet content
struct FormattedContent {
    text: String,
    used_media_urls: Vec<String>,
    mentioned_pubkeys: Vec<PublicKey>,
}

impl TweetFormatter<'_> {
    /// Process tweet content with mention resolution
    fn process_content_with_mentions(&mut self) -> Result<FormattedContent> {
        // Extract media URLs from the tweet
        let tweet_media_urls = crate::media::extract_media_urls_from_tweet(self.tweet);

        // Get the appropriate text (note_tweet has full text, regular text may be truncated)
        let (raw_base_text, has_note_tweet) = if let Some(note) = &self.tweet.note_tweet {
            (&note.text as &str, true)
        } else {
            (&self.tweet.text as &str, false)
        };

        // Decode HTML entities in the text
        let decoded_base_text = decode_html_entities(raw_base_text);
        let base_text = decoded_base_text.as_str();

        // For URL expansion, we need the text that contains t.co URLs
        // Also decode HTML entities in the expansion text
        let decoded_expansion_text = decode_html_entities(&self.tweet.text);
        let text_for_expansion = decoded_expansion_text.as_str();

        // Expand URLs in the text
        let (expanded_text, mut used_media_urls) = expand_urls_in_text(
            text_for_expansion,
            self.tweet.entities.as_ref(),
            &tweet_media_urls,
            self.tweet,
        );

        // Process mentions in the expanded text
        let (text_with_mentions, mentioned_pubkeys) =
            process_mentions_in_text(&expanded_text, self.tweet.entities.as_ref(), self.resolver)?;

        // For note_tweet, we need to apply the same expansions to the full text
        let final_text = if has_note_tweet {
            // Apply URL expansion to the full note_tweet text
            // Note: note_tweet doesn't have its own entities, so we use the main tweet's entities
            let (note_expanded_text, note_used_media) = expand_urls_in_text(
                base_text,
                self.tweet.entities.as_ref(),
                &tweet_media_urls,
                self.tweet,
            );

            // Apply mention processing to the expanded note text
            let (note_with_mentions, note_mentioned_pubkeys) = process_mentions_in_text(
                &note_expanded_text,
                self.tweet.entities.as_ref(),
                self.resolver,
            )?;

            // Update used media URLs if any were used in note_tweet expansion
            used_media_urls.extend(note_used_media);

            // Combine mentioned pubkeys
            let mut all_mentioned_pubkeys = mentioned_pubkeys;
            all_mentioned_pubkeys.extend(note_mentioned_pubkeys);

            FormattedContent {
                text: note_with_mentions,
                used_media_urls,
                mentioned_pubkeys: all_mentioned_pubkeys,
            }
        } else {
            FormattedContent {
                text: text_with_mentions,
                used_media_urls,
                mentioned_pubkeys,
            }
        };

        Ok(final_text)
    }

    /// Process tweet content: extract media, expand URLs, handle note_tweet
    fn process_content(&self) -> FormattedContent {
        // Extract media URLs from the tweet
        let tweet_media_urls = crate::media::extract_media_urls_from_tweet(self.tweet);

        // Get the appropriate text (note_tweet has full text, regular text may be truncated)
        let (raw_base_text, has_note_tweet) = if let Some(note) = &self.tweet.note_tweet {
            (&note.text as &str, true)
        } else {
            (&self.tweet.text as &str, false)
        };

        // Decode HTML entities in the text
        let decoded_base_text = decode_html_entities(raw_base_text);
        let base_text = decoded_base_text.as_str();

        // For URL expansion, we need the text that contains t.co URLs
        // Also decode HTML entities in the expansion text
        let decoded_expansion_text = decode_html_entities(&self.tweet.text);
        let text_for_expansion = decoded_expansion_text.as_str();

        // Expand URLs in the text
        let (expanded_text, mut used_media_urls) = expand_urls_in_text(
            text_for_expansion,
            self.tweet.entities.as_ref(),
            &tweet_media_urls,
            self.tweet,
        );

        // If we have a note_tweet, we need to merge the expanded URLs into the full text
        let final_text = if has_note_tweet && !used_media_urls.is_empty() {
            merge_expanded_urls_into_full_text(base_text, text_for_expansion, &used_media_urls)
        } else if has_note_tweet {
            base_text.to_string()
        } else {
            expanded_text
        };

        // Combine with any external media URLs passed in
        for url in self.media_urls {
            if !used_media_urls.contains(url) && !tweet_media_urls.contains(url) {
                used_media_urls.push(url.clone());
            }
        }

        FormattedContent {
            text: final_text,
            used_media_urls,
            mentioned_pubkeys: Vec::new(),
        }
    }
}

/// Merge expanded URLs from truncated text into the full note_tweet text
fn merge_expanded_urls_into_full_text(
    full_text: &str,
    truncated_text: &str,
    media_urls: &[String],
) -> String {
    // If no media URLs were used, return the full text as-is
    if media_urls.is_empty() {
        return full_text.to_string();
    }

    // Find the position where truncation occurred
    // The truncated text should be a prefix of the full text (minus the t.co URL)
    let truncation_point = truncated_text.rfind("https://t.co/").and_then(|pos| {
        let before_url = &truncated_text[..pos];
        full_text
            .find(before_url.trim_end())
            .map(|p| p + before_url.trim_end().len())
    });

    if let Some(pos) = truncation_point {
        // Insert the media URL at the truncation point
        let mut result = full_text.to_string();

        // Check if we need spacing
        let before_char = result.chars().nth(pos.saturating_sub(1));
        let after_char = result.chars().nth(pos);

        let needs_space_before = before_char.is_some_and(|c| !c.is_whitespace());
        let needs_space_after = after_char.is_some_and(|c| !c.is_whitespace());

        let url_with_spacing = match (needs_space_before, needs_space_after) {
            (true, true) => [" ", &media_urls[0], " "].concat(),
            (true, false) => [" ", &media_urls[0]].concat(),
            (false, true) => [&media_urls[0], " "].concat(),
            (false, false) => media_urls[0].clone(),
        };

        result.insert_str(pos, &url_with_spacing);
        result
    } else {
        // Fallback: append at the end
        [full_text.trim_end(), " ", &media_urls[0]].concat()
    }
}

/// Builds a Twitter status URL from a tweet ID
pub fn build_twitter_status_url(tweet_id: &str) -> String {
    format!("https://twitter.com/i/status/{tweet_id}")
}

/// Structure to track Nostr event details
#[derive(Debug, Serialize, Deserialize)]
pub struct NostrEventInfo {
    /// Original tweet ID
    pub tweet_id: String,
    /// Generated Nostr event ID (hex)
    pub event_id: String,
    /// Public key of the event author (hex)
    pub pubkey: String,
    /// Creation time (UNIX timestamp)
    pub created_at: u64,
    /// Media URLs from Blossom, if any
    pub media_urls: Vec<String>,
    /// Relays the event was sent to
    pub relays: Vec<String>,
    /// Complete Nostr event JSON
    pub event_json: Option<String>,
}

/// Upload media files to Blossom servers
pub async fn upload_media_to_blossom(
    media_files: &[PathBuf],
    blossom_servers: &[String],
    keys: &Keys,
) -> Result<Vec<String>> {
    if blossom_servers.is_empty() {
        bail!("No Blossom servers provided for media upload");
    }

    let mut uploaded_urls = Vec::new();
    let client = reqwest::Client::new();

    for media_file in media_files {
        let file_name = media_file
            .file_name()
            .and_then(|n| n.to_str())
            .context("Invalid file name")?;

        // Try to determine MIME type from extension
        let mime_type = mime_type_from_path(media_file)?;

        debug!("Uploading media file: {file_name} ({mime_type})");

        // Try upload with retries on 429 Too Many Requests
        let mut upload_success = false;
        let mut upload_url = String::new();
        const MAX_RETRIES: usize = 3;
        const RETRY_DELAY_MS: u64 = 500;
        for blossom_server in blossom_servers {
            let server_url = if blossom_server.ends_with('/') {
                blossom_server.clone()
            } else {
                format!("{blossom_server}/")
            };

            // Read file content
            let file_content = fs::read(media_file).await.with_context(|| {
                format!(
                    "Failed to read media file {path}",
                    path = media_file.display()
                )
            })?;

            // Compute SHA-256 for HEAD
            let mut hasher = Sha256::new();
            hasher.update(&file_content);
            let sha256_hex = format!("{:x}", hasher.finalize());

            // Initial HEAD request to get invoice or authorization challenge
            let head_resp = client
                .head(format!("{server_url}upload"))
                .header("X-Content-Length", file_content.len().to_string())
                .header("X-Content-Type", mime_type.clone())
                .header("X-SHA-256", sha256_hex.clone())
                .send()
                .await?;
            let invoice_opt = head_resp
                .headers()
                .get("X-Lightning")
                .and_then(|v| v.to_str().ok())
                .map(String::from);
            // Build Nostr auth event
            let now: Timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_secs()
                .into();
            let auth_builder = EventBuilder::new(Kind::Custom(27235), String::new())
                .custom_created_at(now)
                .tag(Tag::parse(vec!["u", &format!("{server_url}upload")])?)
                .tag(Tag::parse(vec!["method", "PUT"])?);
            let auth_event = auth_builder.sign(keys).await?;
            let auth_header = format!(
                "Nostr {}",
                STANDARD.encode(serde_json::to_string(&auth_event)?)
            );

            // Upload with retry on HTTP 429
            let mut attempts = 0;
            while attempts < MAX_RETRIES {
                attempts += 1;
                let mut req = client
                    .put(format!("{server_url}upload"))
                    .header("Content-Type", mime_type.clone())
                    .header("Authorization", auth_header.clone());
                if let Some(invoice) = &invoice_opt {
                    req = req.header("X-Lightning", invoice);
                }
                let resp = req.body(file_content.clone()).send().await;
                match resp {
                    Ok(r) if r.status() == StatusCode::TOO_MANY_REQUESTS => {
                        warn!(
                            "429 Too Many Requests from {server_url}, retry {}/{}",
                            attempts, MAX_RETRIES
                        );
                        if attempts < MAX_RETRIES {
                            // respect Retry-After header if provided
                            let wait_dur = if let Some(val) = r.headers().get(RETRY_AFTER) {
                                if let Ok(s) = val.to_str() {
                                    if let Ok(secs) = s.parse::<u64>() {
                                        Duration::from_secs(secs)
                                    } else {
                                        Duration::from_millis(RETRY_DELAY_MS)
                                    }
                                } else {
                                    Duration::from_millis(RETRY_DELAY_MS)
                                }
                            } else {
                                Duration::from_millis(RETRY_DELAY_MS)
                            };
                            warn!("Waiting {wait_dur:?} before retry");
                            sleep(wait_dur).await;
                            continue;
                        } else {
                            warn!("Exceeded retries for {server_url}");
                            break;
                        }
                    }
                    Ok(r) => {
                        if r.status().is_success() {
                            // Parse response JSON once
                            let json: serde_json::Value = r.json().await.with_context(|| {
                                format!("Failed to parse Blossom response JSON for {server_url}")
                            })?;
                            // Try top-level "url" or fallback to nip94_event tags
                            let url_opt = json
                                .get("url")
                                .and_then(|u| u.as_str())
                                .map(String::from)
                                .or_else(|| {
                                    json.get("nip94_event")
                                        .and_then(|ev| ev.get("tags"))
                                        .and_then(|tags| tags.as_array())
                                        .and_then(|tags_arr| {
                                            tags_arr.iter().find_map(|tag| {
                                                tag.as_array().and_then(|arr| {
                                                    if arr.first()?.as_str()? == "url" {
                                                        arr.get(1)?.as_str().map(String::from)
                                                    } else {
                                                        None
                                                    }
                                                })
                                            })
                                        })
                                });
                            if let Some(u) = url_opt {
                                upload_url = u;
                                upload_success = true;
                                debug!("Successfully uploaded to Blossom server: {server_url}");
                            } else {
                                warn!("Blossom response missing URL field and nip94_event url tag: {json}");
                            }
                        } else {
                            warn!("Blossom server error {status}", status = r.status());
                        }
                        break;
                    }
                    Err(e) => {
                        warn!("Upload error to Blossom server {server_url}: {e}");
                        break;
                    }
                }
            }
            if !upload_success {
                bail!(
                    "Failed to upload media file {file_name} to any Blossom server",
                    file_name = media_file.display()
                );
            }
        }

        uploaded_urls.push(upload_url);
    }

    Ok(uploaded_urls)
}

/// Determine MIME type from file path
fn mime_type_from_path(path: &Path) -> Result<String> {
    // Extract extension and strip any query parameters (e.g. mp4?tag=12)
    let ext_raw = path
        .extension()
        .and_then(|ext| ext.to_str())
        .context("File has no extension")?;
    let extension = ext_raw.split('?').next().unwrap_or(ext_raw).to_lowercase();

    let mime_type = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    };

    Ok(mime_type.to_string())
}

pub async fn initialize_nostr_client(keys: &Keys, relays: &[String]) -> Result<Client> {
    // In nostr-sdk 0.42, Client::new() requires a signer
    let client = Client::new(keys.clone());

    // Keys are already set in the constructor

    // Add relays
    for relay_url in relays {
        if let Ok(url) = relay_url.parse::<Url>() {
            // In nostr-sdk 0.42 we need to provide the URL as a string
            client.add_relay(url.to_string()).await?;
            debug!("Added relay: {relay_url}");
        } else {
            warn!("Invalid relay URL: {relay_url}");
        }
    }

    // Connect to all relays
    client.connect().await;
    debug!("Connected to Nostr relays");

    Ok(client)
}

/// Check if a tweet has already been posted to Nostr
pub async fn check_existing_nostr_event(
    output_dir: &Path,
    tweet_id: &str,
) -> Result<Option<NostrEventInfo>> {
    let nostr_dir = output_dir.join("nostr");
    let event_info_path = nostr_dir.join(format!("{tweet_id}.json"));

    if event_info_path.exists() {
        let content = fs::read_to_string(&event_info_path)
            .await
            .with_context(|| format!("Failed to read Nostr event info for tweet {tweet_id}"))?;

        let event_info: NostrEventInfo = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse Nostr event info for tweet {tweet_id}"))?;

        Ok(Some(event_info))
    } else {
        Ok(None)
    }
}

/// Save Nostr event information to file
pub async fn save_nostr_event_info(
    event_info: &NostrEventInfo,
    output_dir: &Path,
) -> Result<PathBuf> {
    let nostr_dir = output_dir.join("nostr");

    if !nostr_dir.exists() {
        fs::create_dir_all(&nostr_dir).await.with_context(|| {
            format!(
                "Failed to create Nostr directory at {path}",
                path = nostr_dir.display()
            )
        })?;
    }

    let event_info_path =
        nostr_dir.join(format!("{tweet_id}.json", tweet_id = event_info.tweet_id));

    let json = serde_json::to_string_pretty(event_info)
        .context("Failed to serialize Nostr event info to JSON")?;

    fs::write(&event_info_path, json).await.with_context(|| {
        format!(
            "Failed to write Nostr event info to {path}",
            path = event_info_path.display()
        )
    })?;

    Ok(event_info_path)
}

/// Find an existing event for the tweet
pub async fn find_existing_event(
    client: &Client,
    tweet_id: &str,
    keys: &Keys,
) -> Result<Option<Event>> {
    // Get the public key directly from the keys
    let pubkey = keys.public_key();
    let subscription_id = SubscriptionId::generate();

    // Look for the author's events (us) that reference the tweet URL
    let twitter_url = build_twitter_status_url(tweet_id);

    debug!("Looking for events with Twitter URL reference: {twitter_url}");

    // Build a filter to find events authored by our public key that contain the Twitter URL as a 'r' tag
    // Use custom_tag to filter by the 'r' tag with the specific Twitter URL
    let filter = Filter::new()
        .author(pubkey)
        .kind(Kind::TextNote)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::R), twitter_url.clone())
        .limit(10);

    debug!("Subscribing to events with filter: {filter:?}");

    // Create a subscription
    client.subscribe(filter.clone(), None).await?;

    // Setup a timeout for event search
    let search_timeout = Duration::from_secs(30); // 30 seconds max for search
    debug!("Subscribed to events for tweet {tweet_id}");

    // Try to receive events with a timeout
    let result = timeout(search_timeout, async {
        // Get all relays the client is connected to
        let relays = client.relays().await;

        // Fetch events with a reasonable timeout
        let fetch_timeout = Duration::from_secs(5);

        // Get events from all connected relays
        match client
            .fetch_events_from(relays.keys(), filter.clone(), fetch_timeout)
            .await
        {
            Ok(events) => {
                debug!(
                    "Fetched {count} events from subscription",
                    count = events.len()
                );

                // Since we're filtering by the r tag in the query, any event returned should be a match
                // Return the first event if any exist
                events.into_iter().next()
            }
            Err(e) => {
                debug!("Error fetching events: {e}");
                None
            }
        }
    })
    .await;

    // Unsubscribe from the subscription
    client.unsubscribe(&subscription_id).await;

    match result {
        // If we got a result within the timeout
        Ok(Some(event)) => {
            debug!("Found existing event for tweet {tweet_id}");
            Ok(Some(event))
        }
        // No event found, but search completed successfully
        Ok(None) => {
            debug!("No matching event found for tweet {tweet_id}");
            Ok(None)
        }
        // Timeout occurred while searching
        Err(e) => {
            debug!("Timed out while searching for events for tweet {tweet_id}: {e}");
            Ok(None) // Return None instead of propagating the timeout error
        }
    }
}

/// Replace shortened URLs in tweet text with their expanded versions
/// If media_urls are provided, media-related t.co URLs will be replaced with actual media URLs
/// Returns the expanded text and a list of media URLs that were used inline
fn expand_urls_in_text(
    text: &str,
    entities: Option<&crate::twitter::Entities>,
    media_urls: &[String],
    tweet: &crate::twitter::Tweet,
) -> (String, Vec<String>) {
    let mut result = text.to_string();
    let mut used_media_urls = Vec::new();

    if let Some(entities) = entities {
        if let Some(urls) = &entities.urls {
            // Process URLs in reverse order by length to handle overlapping replacements
            let mut sorted_urls: Vec<_> = urls.iter().collect();
            sorted_urls.sort_by(|a, b| b.url.len().cmp(&a.url.len()));

            for url_entity in sorted_urls {
                // Only replace if the URL is actually shortened (expanded URL is different)
                if url_entity.url != url_entity.expanded_url {
                    // Enhanced media URL detection
                    let is_media_url =
                        is_twitter_media_url(&url_entity.expanded_url, &url_entity.display_url);

                    if is_media_url {
                        // Find the corresponding media URL from the tweet's media
                        if let Some(media_url) =
                            find_media_url_for_shortened_url(&url_entity.url, tweet, media_urls)
                        {
                            // Replace with the actual media URL (no markdown formatting for direct media)
                            result = safe_replace_url(&result, &url_entity.url, &media_url);
                            used_media_urls.push(media_url);
                        } else {
                            // Fallback: use the original markdown link format if no media URL found
                            let fallback_link = format!(
                                "[{}]({})",
                                sanitize_display_url(&url_entity.display_url),
                                &url_entity.expanded_url
                            );
                            result = safe_replace_url(&result, &url_entity.url, &fallback_link);
                        }
                    } else {
                        // Non-media URL: use regular expansion with markdown format
                        if is_valid_url(&url_entity.expanded_url) {
                            let markdown_link = format!(
                                "[{}]({})",
                                sanitize_display_url(&url_entity.display_url),
                                &url_entity.expanded_url
                            );
                            result = safe_replace_url(&result, &url_entity.url, &markdown_link);
                        } else {
                            debug!(
                                "Invalid expanded URL {}, keeping original",
                                url_entity.expanded_url
                            );
                        }
                    }
                } else {
                    debug!(
                        "URL {url} is not shortened, keeping as-is",
                        url = url_entity.url
                    );
                }
            }
        }
    }

    (result, used_media_urls)
}

/// Find the actual media URL that corresponds to a shortened t.co URL
fn find_media_url_for_shortened_url(
    shortened_url: &str,
    tweet: &crate::twitter::Tweet,
    media_urls: &[String],
) -> Option<String> {
    // For video tweets, the media_urls should contain the actual video URLs
    // We need to find the best quality video variant to replace the t.co URL
    if media_urls.is_empty() || !shortened_url.contains("t.co") {
        return None;
    }

    // First, try to match using URL entities
    if let Some(entities) = &tweet.entities {
        if let Some(urls) = &entities.urls {
            for url_entity in urls {
                if url_entity.url == shortened_url {
                    // This is the matching t.co URL
                    // If it's a video or photo URL, return the best media URL
                    if is_twitter_media_url(&url_entity.expanded_url, &url_entity.display_url) {
                        // For videos, prefer the highest quality variant which is typically last in media_urls
                        // For images, any media URL should work
                        if url_entity.expanded_url.contains("/video/") {
                            return media_urls.last().cloned();
                        } else {
                            return media_urls.first().cloned();
                        }
                    }
                }
            }
        }
    }

    // Fallback: if we have exactly one media URL and this looks like a media t.co URL,
    // assume they correspond
    if media_urls.len() == 1 {
        return Some(media_urls[0].clone());
    }

    None
}

/// Process mentions in the text and convert Twitter usernames to Nostr npub links
/// Returns the processed text and a list of mentioned pubkeys
fn process_mentions_in_text(
    text: &str,
    entities: Option<&crate::twitter::Entities>,
    resolver: &mut NostrLinkResolver,
) -> Result<(String, Vec<PublicKey>)> {
    let mut result = text.to_string();
    let mut mentioned_pubkeys = Vec::new();
    let mut processed_usernames = std::collections::HashSet::new();

    if let Some(entities) = entities {
        if let Some(mentions) = &entities.mentions {
            // Process each mention
            for mention in mentions {
                let username = &mention.username;

                // Skip if we've already processed this username (avoid duplicates)
                if processed_usernames.contains(username) {
                    continue;
                }
                processed_usernames.insert(username.clone());

                // Try to resolve the Twitter username to a Nostr pubkey
                match resolver.resolve_username(username) {
                    Ok(Some(pubkey)) => {
                        // Convert pubkey to npub format
                        match pubkey.to_bech32() {
                            Ok(npub) => {
                                // Replace @username with nostr:npub... link
                                let old_mention = format!("@{username}");
                                let new_mention = format!("nostr:{npub}");
                                result = result.replace(&old_mention, &new_mention);

                                // Add to mentioned pubkeys list
                                mentioned_pubkeys.push(pubkey);

                                debug!("Converted @{username} to {npub}");
                            }
                            Err(e) => {
                                warn!("Failed to convert pubkey to bech32 for @{username}: {e}");
                            }
                        }
                    }
                    Ok(None) => {
                        debug!("Could not resolve @{username} to Nostr pubkey, keeping as-is");
                    }
                    Err(e) => {
                        warn!("Error resolving @{username}: {e}");
                    }
                }
            }
        }
    }

    // Also try to process any @mentions that weren't captured in entities
    // This handles edge cases where Twitter's entity extraction might miss some mentions
    let additional_mentions = extract_additional_mentions(&result, &processed_usernames);
    for username in additional_mentions {
        if let Ok(Some(pubkey)) = resolver.resolve_username(&username) {
            let npub = pubkey.to_bech32().expect("to_bech32 should never fail");
            let old_mention = format!("@{username}");
            let new_mention = format!("nostr:{npub}");
            result = result.replace(&old_mention, &new_mention);
            mentioned_pubkeys.push(pubkey);
            debug!("Converted additional @{username} to {npub}");
        }
    }

    Ok((result, mentioned_pubkeys))
}

/// Decode HTML entities in text (e.g., &gt; to >, &lt; to <, &amp; to &)
fn decode_html_entities(text: &str) -> String {
    html_escape::decode_html_entities(text).to_string()
}

/// Enhanced detection of Twitter media URLs
fn is_twitter_media_url(expanded_url: &str, display_url: &str) -> bool {
    expanded_url.contains("/photo/")
        || expanded_url.contains("/video/")
        || expanded_url.contains("/status/") && display_url.starts_with("pic.")
        || display_url.starts_with("pic.twitter.com")
        || display_url.starts_with("video.twimg.com")
}

/// Safely replace a URL in text, handling edge cases
fn safe_replace_url(text: &str, old_url: &str, new_url: &str) -> String {
    // Only replace if the old URL exists in the text
    if text.contains(old_url) {
        text.replace(old_url, new_url)
    } else {
        debug!("URL {old_url} not found in text, skipping replacement");
        text.to_string()
    }
}

/// Sanitize display URL to prevent markdown injection
fn sanitize_display_url(display_url: &str) -> String {
    display_url
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Validate if a URL is properly formatted
fn is_valid_url(url: &str) -> bool {
    UrlParser::parse(url).is_ok() && (url.starts_with("http://") || url.starts_with("https://"))
}

/// Extract additional @mentions that might not be in Twitter entities
fn extract_additional_mentions(
    text: &str,
    already_processed: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut mentions = Vec::new();

    // Simple regex to find @username patterns
    for word in text.split_whitespace() {
        if word.starts_with('@') && word.len() > 1 {
            let username = word[1..].trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_');
            if !username.is_empty()
                && username.len() <= 15 // Twitter username length limit
                && !already_processed.contains(username)
            {
                mentions.push(username.to_string());
            }
        }
    }

    mentions
}

/// Format a tweet as Nostr content with mention resolution
pub fn format_tweet_as_nostr_content_with_mentions(
    tweet: &crate::twitter::Tweet,
    media_urls: &[String],
    resolver: &mut NostrLinkResolver,
) -> Result<(String, Vec<PublicKey>)> {
    let mut content = String::new();
    let mut all_mentioned_pubkeys = Vec::new();

    let (is_simple_retweet, rt_username) = analyze_retweet(tweet);

    add_author_info(&mut content, tweet, is_simple_retweet);
    let (used_media_urls, mentioned_pubkeys) = add_tweet_content_with_mentions(
        &mut content,
        tweet,
        is_simple_retweet,
        media_urls,
        resolver,
    )?;
    all_mentioned_pubkeys.extend(mentioned_pubkeys);

    let ref_mentioned_pubkeys = add_referenced_tweets_with_mentions(
        &mut content,
        tweet,
        is_simple_retweet,
        &rt_username,
        resolver,
    )?;
    all_mentioned_pubkeys.extend(ref_mentioned_pubkeys);

    // For simple retweets, don't add media URLs since they belong to the retweeted content
    if !is_simple_retweet {
        add_media_urls(&mut content, media_urls, &used_media_urls);
    }
    add_original_tweet_url(&mut content, &tweet.id);

    Ok((content, all_mentioned_pubkeys))
}

/// Format a tweet as Nostr content (legacy version without mention resolution)
pub fn format_tweet_as_nostr_content(
    tweet: &crate::twitter::Tweet,
    media_urls: &[String],
) -> String {
    let mut content = String::new();

    let (is_simple_retweet, rt_username) = analyze_retweet(tweet);

    add_author_info(&mut content, tweet, is_simple_retweet);
    let used_media_urls = add_tweet_content(&mut content, tweet, is_simple_retweet, media_urls);
    add_referenced_tweets(&mut content, tweet, is_simple_retweet, &rt_username);
    // For simple retweets, don't add media URLs since they belong to the retweeted content
    if !is_simple_retweet {
        add_media_urls(&mut content, media_urls, &used_media_urls);
    }
    add_original_tweet_url(&mut content, &tweet.id);

    content
}

/// Check if a tweet is a simple retweet and extract username if possible
fn analyze_retweet(tweet: &crate::twitter::Tweet) -> (bool, Option<String>) {
    let Some(ref_tweets) = &tweet.referenced_tweets else {
        return (false, None);
    };

    if !ref_tweets.iter().any(|rt| rt.type_field == "retweeted") {
        return (false, None);
    }

    // Pure retweets typically start with "RT @username:"
    let raw_text = if let Some(note) = &tweet.note_tweet {
        &note.text
    } else {
        &tweet.text
    };

    // Check if text is just "RT @username:" followed by the retweeted content
    let is_rt = raw_text.starts_with("RT @")
        && raw_text.contains(":")
        && !raw_text.contains("\n")
        && !raw_text.contains(" // ");

    // Try to extract the username from the RT text
    let username = if is_rt {
        raw_text.find(":").and_then(|end_idx| {
            raw_text.find("@").map(|start_idx| {
                // Extract username without the @ symbol
                raw_text[(start_idx + 1)..end_idx].trim().to_string()
            })
        })
    } else {
        None
    };

    (is_rt, username)
}

/// Add author information to the content
fn add_author_info(content: &mut String, tweet: &crate::twitter::Tweet, is_simple_retweet: bool) {
    if is_simple_retweet {
        return;
    }

    if !tweet.author.username.is_empty() {
        content.push_str(&format!(
            "🐦 @{username}: ",
            username = tweet.author.username
        ));
    } else if let Some(author_id) = &tweet.author_id {
        content.push_str(&format!("🐦 User {author_id}: "));
    } else {
        content.push_str("🐦 Tweet: ");
    }
}

/// Add the main tweet content with mention resolution
/// Returns the list of media URLs that were used inline and mentioned pubkeys
fn add_tweet_content_with_mentions(
    content: &mut String,
    tweet: &crate::twitter::Tweet,
    is_simple_retweet: bool,
    media_urls: &[String],
    resolver: &mut NostrLinkResolver,
) -> Result<(Vec<String>, Vec<PublicKey>)> {
    if is_simple_retweet {
        return Ok((Vec::new(), Vec::new()));
    }

    // Add the tweet author to the resolver so mentions can find them
    resolver.add_known_user(&tweet.author.username, &tweet.author.id)?;

    // Add tweet text with expanded URLs
    // Prefer extended text when available
    let raw_text = if let Some(note) = &tweet.note_tweet {
        &note.text
    } else {
        &tweet.text
    };

    // Decode HTML entities first
    let decoded_text = decode_html_entities(raw_text);

    // Then expand URLs
    let (expanded_text, used_media_urls) =
        expand_urls_in_text(&decoded_text, tweet.entities.as_ref(), media_urls, tweet);

    // Then process mentions
    let (text_with_mentions, mentioned_pubkeys) =
        process_mentions_in_text(&expanded_text, tweet.entities.as_ref(), resolver)?;

    content.push_str(&text_with_mentions);
    content.push_str("\n\n");

    Ok((used_media_urls, mentioned_pubkeys))
}

/// Add the main tweet content
/// Returns the list of media URLs that were used inline
fn add_tweet_content(
    content: &mut String,
    tweet: &crate::twitter::Tweet,
    is_simple_retweet: bool,
    media_urls: &[String],
) -> Vec<String> {
    if is_simple_retweet {
        return Vec::new();
    }

    // Add tweet text with expanded URLs
    // Prefer extended text when available
    let raw_text = if let Some(note) = &tweet.note_tweet {
        &note.text
    } else {
        &tweet.text
    };

    // Decode HTML entities first
    let decoded_text = decode_html_entities(raw_text);

    let (expanded_text, used_media_urls) =
        expand_urls_in_text(&decoded_text, tweet.entities.as_ref(), media_urls, tweet);
    content.push_str(&expanded_text);
    content.push_str("\n\n");

    used_media_urls
}

/// Format a reply tweet with mention resolution
fn format_reply_tweet_with_mentions(
    content: &mut String,
    ref_tweet: &crate::twitter::ReferencedTweet,
    tweet_url: &str,
    resolver: &mut NostrLinkResolver,
) -> Result<Vec<PublicKey>> {
    let mut mentioned_pubkeys = Vec::new();

    if let Some(ref_data) = &ref_tweet.data {
        // Add the referenced tweet author to resolver
        resolver.add_known_user(&ref_data.author.username, &ref_data.author.id)?;

        // Try to resolve the referenced author to a Nostr pubkey
        let author_npub =
            if let Some(pubkey) = resolver.resolve_username(&ref_data.author.username)? {
                mentioned_pubkeys.push(pubkey);
                pubkey
                    .to_bech32()
                    .map_err(|e| anyhow::anyhow!("Failed to convert to bech32: {e}"))?
            } else {
                // Fallback to Twitter username if we can't resolve
                format!("@{}", ref_data.author.username)
            };

        let mut formatter = TweetFormatter {
            tweet: ref_data,
            media_urls: &[],
            resolver,
        };
        let formatted = formatter.process_content_with_mentions()?;
        mentioned_pubkeys.extend(formatted.mentioned_pubkeys);

        // Add reply header with Nostr link if resolved
        if author_npub.starts_with("npub") {
            content.push_str(&format!("↩️ Reply to nostr:{author_npub}:\n"));
        } else {
            content.push_str(&format!("↩️ Reply to {author_npub}:\n"));
        }

        // Add content
        content.push_str(&formatted.text);
        content.push('\n');

        // Add any unused media URLs
        let tweet_media_urls = crate::media::extract_media_urls_from_tweet(ref_data);
        for url in &tweet_media_urls {
            if !formatted.used_media_urls.contains(url) {
                content.push_str(&format!("{url}\n"));
            }
        }

        // Add link to original tweet
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        // Fallback: simple link if data not available
        content.push_str(&format!(
            "↩️ Reply to Tweet {id}\n{tweet_url}\n",
            id = ref_tweet.id
        ));
    }

    Ok(mentioned_pubkeys)
}

/// Format a reply tweet
fn format_reply_tweet(
    content: &mut String,
    ref_tweet: &crate::twitter::ReferencedTweet,
    tweet_url: &str,
) {
    if let Some(ref_data) = &ref_tweet.data {
        // Legacy formatter without mention resolution
        let mut dummy_resolver = NostrLinkResolver::new(None);
        let formatter = TweetFormatter {
            tweet: ref_data,
            media_urls: &[],
            resolver: &mut dummy_resolver,
        };
        let formatted = formatter.process_content();

        // Add reply header
        content.push_str(&format!(
            "↩️ Reply to @{username}:\n",
            username = ref_data.author.username
        ));

        // Add content
        content.push_str(&formatted.text);
        content.push('\n');

        // Add any unused media URLs
        let tweet_media_urls = crate::media::extract_media_urls_from_tweet(ref_data);
        for url in &tweet_media_urls {
            if !formatted.used_media_urls.contains(url) {
                content.push_str(&format!("{url}\n"));
            }
        }

        // Add link to original tweet
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        // Fallback: simple link if data not available
        content.push_str(&format!(
            "↩️ Reply to Tweet {id}\n{tweet_url}\n",
            id = ref_tweet.id
        ));
    }
}

/// Format a quoted tweet with mention resolution
fn format_quote_tweet_with_mentions(
    content: &mut String,
    ref_tweet: &crate::twitter::ReferencedTweet,
    tweet_url: &str,
    resolver: &mut NostrLinkResolver,
) -> Result<Vec<PublicKey>> {
    let mut mentioned_pubkeys = Vec::new();

    if let Some(ref_data) = &ref_tweet.data {
        // Add the quoted tweet author to resolver
        resolver.add_known_user(&ref_data.author.username, &ref_data.author.id)?;

        // Try to resolve the quoted author to a Nostr pubkey
        let author_npub =
            if let Some(pubkey) = resolver.resolve_username(&ref_data.author.username)? {
                mentioned_pubkeys.push(pubkey);
                pubkey
                    .to_bech32()
                    .map_err(|e| anyhow::anyhow!("Failed to convert to bech32: {e}"))?
            } else {
                // Fallback to Twitter username if we can't resolve
                format!("@{}", ref_data.author.username)
            };

        let mut formatter = TweetFormatter {
            tweet: ref_data,
            media_urls: &[],
            resolver,
        };
        let formatted = formatter.process_content_with_mentions()?;
        mentioned_pubkeys.extend(formatted.mentioned_pubkeys);

        // Add quote header with Nostr link if resolved
        if author_npub.starts_with("npub") {
            content.push_str(&format!("💬 Quote of nostr:{author_npub}:\n"));
        } else {
            content.push_str(&format!("💬 Quote of {author_npub}:\n"));
        }

        // Add content
        content.push_str(&formatted.text);
        content.push('\n');

        // Add any unused media URLs
        let tweet_media_urls = crate::media::extract_media_urls_from_tweet(ref_data);
        for url in &tweet_media_urls {
            if !formatted.used_media_urls.contains(url) {
                content.push_str(&format!("{url}\n"));
            }
        }

        // Add link to original tweet
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        // Fallback: simple link if data not available
        content.push_str(&format!(
            "💬 Quote of Tweet {id}\n{tweet_url}\n",
            id = ref_tweet.id
        ));
    }

    Ok(mentioned_pubkeys)
}

/// Format a quoted tweet
fn format_quote_tweet(
    content: &mut String,
    ref_tweet: &crate::twitter::ReferencedTweet,
    tweet_url: &str,
) {
    if let Some(ref_data) = &ref_tweet.data {
        // Legacy formatter without mention resolution
        let mut dummy_resolver = NostrLinkResolver::new(None);
        let formatter = TweetFormatter {
            tweet: ref_data,
            media_urls: &[],
            resolver: &mut dummy_resolver,
        };
        let formatted = formatter.process_content();

        // Add quote header
        content.push_str(&format!(
            "💬 Quote of @{username}:\n",
            username = ref_data.author.username
        ));

        // Add content
        content.push_str(&formatted.text);
        content.push('\n');

        // Add any unused media URLs
        let tweet_media_urls = crate::media::extract_media_urls_from_tweet(ref_data);
        for url in &tweet_media_urls {
            if !formatted.used_media_urls.contains(url) {
                content.push_str(&format!("{url}\n"));
            }
        }

        // Add link to original tweet
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        // Fallback: simple link if data not available
        content.push_str(&format!(
            "💬 Quote of Tweet {id}\n{tweet_url}\n",
            id = ref_tweet.id
        ));
    }
}

/// Format a retweet with mention resolution
fn format_retweet_with_mentions(
    content: &mut String,
    ref_tweet: &crate::twitter::ReferencedTweet,
    tweet_url: &str,
    tweet: &crate::twitter::Tweet,
    is_simple_retweet: bool,
    rt_username: &Option<String>,
    resolver: &mut NostrLinkResolver,
) -> Result<Vec<PublicKey>> {
    let mut mentioned_pubkeys = Vec::new();

    if let Some(ref_data) = &ref_tweet.data {
        // Add the retweeted author to resolver
        resolver.add_known_user(&ref_data.author.username, &ref_data.author.id)?;

        // Try to resolve the retweeted author to a Nostr pubkey
        let author_npub =
            if let Some(pubkey) = resolver.resolve_username(&ref_data.author.username)? {
                mentioned_pubkeys.push(pubkey);
                pubkey
                    .to_bech32()
                    .map_err(|e| anyhow::anyhow!("Failed to convert to bech32: {e}"))?
            } else {
                // Fallback to Twitter username if we can't resolve
                format!("@{}", ref_data.author.username)
            };

        // Add retweet header
        let prefix = if is_simple_retweet {
            let base = format!("🔁 @{username} retweeted", username = tweet.author.username);
            if let Some(username) = rt_username {
                // Try to resolve the rt_username too
                if let Some(pubkey) = resolver.resolve_username(username)? {
                    mentioned_pubkeys.push(pubkey);
                    let npub = pubkey
                        .to_bech32()
                        .map_err(|e| anyhow::anyhow!("Failed to convert to bech32: {e}"))?;
                    format!("{base} nostr:{npub}:\n")
                } else {
                    format!("{base} @{username}:\n")
                }
            } else if author_npub.starts_with("npub") {
                format!("{base} nostr:{author_npub}:\n")
            } else {
                format!("{base} {author_npub}:\n")
            }
        } else if author_npub.starts_with("npub") {
            format!("🔄 Retweet of nostr:{author_npub}:\n")
        } else {
            format!("🔄 Retweet of {author_npub}:\n")
        };
        content.push_str(&prefix);

        // Process the retweeted content
        let mut formatter = TweetFormatter {
            tweet: ref_data,
            media_urls: &[],
            resolver,
        };
        let formatted = formatter.process_content_with_mentions()?;
        mentioned_pubkeys.extend(formatted.mentioned_pubkeys);

        // Add content
        content.push_str(&formatted.text);
        content.push('\n');

        // For non-note_tweet cases, add unused media URLs
        if ref_data.note_tweet.is_none() {
            let tweet_media_urls = crate::media::extract_media_urls_from_tweet(ref_data);
            for url in &tweet_media_urls {
                if !formatted.used_media_urls.contains(url) {
                    content.push_str(&format!("{url}\n"));
                }
            }
        }

        // Add link to original tweet
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        // Fallback for simple retweets without data
        if is_simple_retweet {
            if let Some(username) = rt_username.as_ref() {
                // Try to resolve the username
                if let Some(pubkey) = resolver.resolve_username(username)? {
                    mentioned_pubkeys.push(pubkey);
                    let npub = pubkey
                        .to_bech32()
                        .map_err(|e| anyhow::anyhow!("Failed to convert to bech32: {e}"))?;
                    content.push_str(&format!(
                        "🔁 @{author} retweeted nostr:{npub}:\n{tweet_url}\n",
                        author = tweet.author.username
                    ));
                } else {
                    content.push_str(&format!(
                        "🔁 @{author} retweeted @{username}:\n{tweet_url}\n",
                        author = tweet.author.username
                    ));
                }
            }
        } else {
            content.push_str(&format!(
                "🔄 Retweet of Tweet {id}\n{tweet_url}\n",
                id = ref_tweet.id
            ));
        }
    }

    Ok(mentioned_pubkeys)
}

/// Format a retweet
fn format_retweet(
    content: &mut String,
    ref_tweet: &crate::twitter::ReferencedTweet,
    tweet_url: &str,
    tweet: &crate::twitter::Tweet,
    is_simple_retweet: bool,
    rt_username: &Option<String>,
) {
    if let Some(ref_data) = &ref_tweet.data {
        // Add retweet header
        let prefix = if is_simple_retweet {
            let base = format!("🔁 @{username} retweeted", username = tweet.author.username);
            match rt_username {
                Some(username) => format!("{base} @{username}:\n"),
                None => format!("{base}:\n"),
            }
        } else {
            format!(
                "🔄 Retweet of @{username}:\n",
                username = ref_data.author.username
            )
        };
        content.push_str(&prefix);

        // Process the retweeted content
        // Legacy formatter without mention resolution
        let mut dummy_resolver = NostrLinkResolver::new(None);
        let formatter = TweetFormatter {
            tweet: ref_data,
            media_urls: &[],
            resolver: &mut dummy_resolver,
        };
        let formatted = formatter.process_content();

        // Add content
        content.push_str(&formatted.text);
        content.push('\n');

        // For non-note_tweet cases, add unused media URLs
        if ref_data.note_tweet.is_none() {
            let tweet_media_urls = crate::media::extract_media_urls_from_tweet(ref_data);
            for url in &tweet_media_urls {
                if !formatted.used_media_urls.contains(url) {
                    content.push_str(&format!("{url}\n"));
                }
            }
        }

        // Add link to original tweet
        content.push_str(&format!("{tweet_url}\n"));
    } else {
        // Fallback for simple retweets without data
        if is_simple_retweet && rt_username.is_some() {
            if let Some(username) = rt_username {
                content.push_str(&format!(
                    "🔁 @{} retweeted @{username}:\n{tweet_url}\n",
                    tweet.author.username
                ));
            }
        } else {
            content.push_str(&format!(
                "🔄 Retweet of Tweet {id}\n{tweet_url}\n",
                id = ref_tweet.id
            ));
        }
    }
}

/// Add referenced tweets with mention resolution
fn add_referenced_tweets_with_mentions(
    content: &mut String,
    tweet: &crate::twitter::Tweet,
    is_simple_retweet: bool,
    rt_username: &Option<String>,
    resolver: &mut NostrLinkResolver,
) -> Result<Vec<PublicKey>> {
    let Some(referenced_tweets) = &tweet.referenced_tweets else {
        return Ok(Vec::new());
    };

    let mut all_mentioned_pubkeys = Vec::new();

    for ref_tweet in referenced_tweets {
        let tweet_url = build_twitter_status_url(&ref_tweet.id);

        let mentioned_pubkeys = match ref_tweet.type_field.as_str() {
            "replied_to" => {
                format_reply_tweet_with_mentions(content, ref_tweet, &tweet_url, resolver)?
            }
            "quoted" => format_quote_tweet_with_mentions(content, ref_tweet, &tweet_url, resolver)?,
            "retweeted" => format_retweet_with_mentions(
                content,
                ref_tweet,
                &tweet_url,
                tweet,
                is_simple_retweet,
                rt_username,
                resolver,
            )?,
            _ => {
                // Generic reference format for unknown types
                if let Some(ref_data) = &ref_tweet.data {
                    content.push_str(&format!(
                        "📎 Reference to @{username}'s tweet ({type_field})\n{tweet_url}\n",
                        username = ref_data.author.username,
                        type_field = ref_tweet.type_field
                    ));
                } else {
                    content.push_str(&format!(
                        "📎 Reference to tweet ({type_field})\n{tweet_url}\n",
                        type_field = ref_tweet.type_field
                    ));
                }
                Vec::new()
            }
        };

        all_mentioned_pubkeys.extend(mentioned_pubkeys);
    }

    Ok(all_mentioned_pubkeys)
}

/// Add referenced tweets (replies, quotes, retweets)
fn add_referenced_tweets(
    content: &mut String,
    tweet: &crate::twitter::Tweet,
    is_simple_retweet: bool,
    rt_username: &Option<String>,
) {
    let Some(referenced_tweets) = &tweet.referenced_tweets else {
        return;
    };

    for ref_tweet in referenced_tweets {
        let tweet_url = build_twitter_status_url(&ref_tweet.id);

        match ref_tweet.type_field.as_str() {
            "replied_to" => format_reply_tweet(content, ref_tweet, &tweet_url),
            "quoted" => format_quote_tweet(content, ref_tweet, &tweet_url),
            "retweeted" => format_retweet(
                content,
                ref_tweet,
                &tweet_url,
                tweet,
                is_simple_retweet,
                rt_username,
            ),
            _ => {
                // Generic reference format for unknown types
                if let Some(ref_data) = &ref_tweet.data {
                    content.push_str(&format!(
                        "🔗 Reference to @{username}:\n",
                        username = ref_data.author.username
                    ));
                    // Legacy formatter without mention resolution
                    let mut dummy_resolver = NostrLinkResolver::new(None);
                    let formatter = TweetFormatter {
                        tweet: ref_data,
                        media_urls: &[],
                        resolver: &mut dummy_resolver,
                    };
                    let formatted = formatter.process_content();
                    content.push_str(&formatted.text);
                    content.push('\n');
                    content.push_str(&format!("{tweet_url}\n"));
                } else {
                    content.push_str(&format!(
                        "🔗 Reference to Tweet {}\n{tweet_url}\n",
                        ref_tweet.id
                    ));
                }
            }
        }
    }
}

/// Add media URLs to the content (only those not already used inline)
fn add_media_urls(content: &mut String, media_urls: &[String], used_media_urls: &[String]) {
    // Add media URLs if present, but skip those already used inline
    for url in media_urls {
        if !used_media_urls.contains(url) {
            content.push_str(&format!("{url}\n"));
        }
    }
}

/// Add original tweet URL to the content
fn add_original_tweet_url(content: &mut String, tweet_id: &str) {
    // Add link to original tweet
    content.push_str(&format!(
        "\nOriginal tweet: {}",
        build_twitter_status_url(tweet_id)
    ));
}

/// Publish a Nostr event to the specified relays
pub async fn publish_nostr_event(client: &Client, event: &Event) -> Result<()> {
    let event_id_hex = event.id.to_hex();
    match client.send_event(event).await {
        Ok(returned_event_id) => {
            info!(
                "Published Nostr event with ID: {id}, expected: {event_id_hex}",
                id = returned_event_id.to_hex()
            );
            Ok(())
        }
        Err(e) => {
            bail!("Failed to publish Nostr event {event_id_hex}: {e}");
        }
    }
}

/// Update the user's relay list on Nostr (Kind 10002)
pub async fn update_relay_list(client: &Client, keys: &Keys, relays: &[String]) -> Result<()> {
    info!("Updating Nostr relay list");

    let relay_list: Vec<(RelayUrl, Option<RelayMetadata>)> = relays
        .iter()
        .filter_map(|url| match RelayUrl::parse(url) {
            Ok(relay_url) => Some((relay_url, None)),
            Err(e) => {
                warn!("Failed to parse relay url '{url}': {e}");
                None
            }
        })
        .collect();

    if relay_list.is_empty() && !relays.is_empty() {
        bail!("No valid relay URLs found to update list.");
    }

    let event = EventBuilder::relay_list(relay_list).sign(keys).await?;

    publish_nostr_event(client, &event).await?;

    info!("Successfully updated Nostr relay list");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::twitter::{Entities, NoteTweet, ReferencedTweet, Tweet, UrlEntity, User};
    use pretty_assertions::assert_eq;

    fn create_test_tweet() -> Tweet {
        Tweet {
            id: "123456789".to_string(),
            text: "This is a test tweet with a link https://t.co/abc123".to_string(),
            author: User {
                id: "987654321".to_string(),
                name: Some("Test User".to_string()),
                username: "testuser".to_string(),
                profile_image_url: None,
                description: None,
                url: None,
                entities: None,
            },
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: Some(Entities {
                urls: Some(vec![UrlEntity {
                    url: "https://t.co/abc123".to_string(),
                    expanded_url: "https://example.com/article".to_string(),
                    display_url: "example.com/article".to_string(),
                }]),
                hashtags: None,
                mentions: None,
            }),
            includes: None,
            author_id: Some("987654321".to_string()),
            note_tweet: None,
        }
    }

    fn create_test_tweet_with_mentions() -> Tweet {
        Tweet {
            id: "123456789".to_string(),
            text: "Hello @alice and @bob! Check this out https://t.co/abc123".to_string(),
            author: User {
                id: "987654321".to_string(),
                name: Some("Test User".to_string()),
                username: "testuser".to_string(),
                profile_image_url: None,
                description: None,
                url: None,
                entities: None,
            },
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: Some(Entities {
                urls: Some(vec![UrlEntity {
                    url: "https://t.co/abc123".to_string(),
                    expanded_url: "https://example.com/article".to_string(),
                    display_url: "example.com/article".to_string(),
                }]),
                hashtags: None,
                mentions: Some(vec![
                    crate::twitter::Mention {
                        username: "alice".to_string(),
                    },
                    crate::twitter::Mention {
                        username: "bob".to_string(),
                    },
                ]),
            }),
            includes: None,
            author_id: Some("987654321".to_string()),
            note_tweet: None,
        }
    }

    #[test]
    fn test_build_twitter_status_url() {
        assert_eq!(
            build_twitter_status_url("123456789"),
            "https://twitter.com/i/status/123456789"
        );
    }

    #[test]
    fn test_decode_html_entities() {
        // Test common HTML entities
        assert_eq!(decode_html_entities("&gt;"), ">");
        assert_eq!(decode_html_entities("&lt;"), "<");
        assert_eq!(decode_html_entities("&amp;"), "&");
        assert_eq!(decode_html_entities("&quot;"), "\"");
        assert_eq!(decode_html_entities("&#39;"), "'");

        // Test in context
        assert_eq!(
            decode_html_entities("Boa ditadura &gt; Democracia mediana"),
            "Boa ditadura > Democracia mediana"
        );
        assert_eq!(decode_html_entities("A &amp; B &lt; C"), "A & B < C");

        // Test multiple entities
        assert_eq!(
            decode_html_entities("&lt;html&gt;&amp;&quot;test&quot;&lt;/html&gt;"),
            "<html>&\"test\"</html>"
        );

        // Test no entities
        assert_eq!(
            decode_html_entities("Normal text without entities"),
            "Normal text without entities"
        );
    }

    #[test]
    fn test_process_mentions_in_text() -> Result<()> {
        let tweet = create_test_tweet_with_mentions();
        let mut resolver = NostrLinkResolver::new(None);

        // Add known users to resolver
        resolver.add_known_user("alice", "111111")?;
        resolver.add_known_user("bob", "222222")?;

        let (processed_text, mentioned_pubkeys) =
            process_mentions_in_text(&tweet.text, tweet.entities.as_ref(), &mut resolver)?;

        // Check that mentions were converted to nostr: links
        assert!(processed_text.contains("nostr:npub"));
        assert!(!processed_text.contains("@alice"));
        assert!(!processed_text.contains("@bob"));

        // Check that we have 2 mentioned pubkeys
        assert_eq!(mentioned_pubkeys.len(), 2);

        Ok(())
    }

    #[test]
    fn test_process_mentions_unknown_users() -> Result<()> {
        let tweet = create_test_tweet_with_mentions();
        let mut resolver = NostrLinkResolver::new(None);

        // Don't add users to resolver - they should remain as @mentions
        let (processed_text, mentioned_pubkeys) =
            process_mentions_in_text(&tweet.text, tweet.entities.as_ref(), &mut resolver)?;

        // Check that mentions remain as @mentions
        assert!(processed_text.contains("@alice"));
        assert!(processed_text.contains("@bob"));
        assert!(!processed_text.contains("nostr:"));

        // Check that we have no mentioned pubkeys
        assert_eq!(mentioned_pubkeys.len(), 0);

        Ok(())
    }

    #[test]
    fn test_format_tweet_with_mentions() -> Result<()> {
        let tweet = create_test_tweet_with_mentions();
        let mut resolver = NostrLinkResolver::new(None);

        // Add known users
        resolver.add_known_user("alice", "111111")?;
        resolver.add_known_user("bob", "222222")?;

        let (content, mentioned_pubkeys) =
            format_tweet_as_nostr_content_with_mentions(&tweet, &[], &mut resolver)?;

        // Check content structure
        assert!(content.contains("🐦 @testuser:"));
        assert!(content.contains("nostr:npub")); // Should have converted mentions
        assert!(content.contains("[example.com/article](https://example.com/article)")); // URL expansion
        assert!(content.contains("https://twitter.com/i/status/123456789")); // Original tweet URL

        // Check mentioned pubkeys
        assert_eq!(mentioned_pubkeys.len(), 2);

        Ok(())
    }

    #[test]
    fn test_format_reply_with_mentions() -> Result<()> {
        let mut main_tweet = create_test_tweet_with_mentions();

        // Create a reply reference
        let reply_user = User {
            id: "333333".to_string(),
            username: "replyuser".to_string(),
            name: Some("Reply User".to_string()),
            ..Default::default()
        };

        let reply_tweet = Tweet {
            id: "987654321".to_string(),
            text: "Original tweet text".to_string(),
            author: reply_user,
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: None,
            includes: None,
            author_id: Some("333333".to_string()),
            note_tweet: None,
        };

        main_tweet.referenced_tweets = Some(vec![ReferencedTweet {
            type_field: "replied_to".to_string(),
            id: "987654321".to_string(),
            data: Some(Box::new(reply_tweet)),
        }]);

        let mut resolver = NostrLinkResolver::new(None);

        // Add known users including the replied-to user
        resolver.add_known_user("alice", "111111")?;
        resolver.add_known_user("replyuser", "333333")?;

        let (content, mentioned_pubkeys) =
            format_tweet_as_nostr_content_with_mentions(&main_tweet, &[], &mut resolver)?;

        // Check that reply header contains nostr link
        assert!(content.contains("↩️ Reply to nostr:npub"));

        // Check that we have mentioned pubkeys from both the main tweet and the reply
        assert!(mentioned_pubkeys.len() >= 2); // alice + replyuser at minimum

        Ok(())
    }

    #[test]
    fn test_format_tweet_with_html_entities() {
        let mut tweet = create_test_tweet();
        tweet.text = "This tweet has &gt; and &lt; and &amp; symbols".to_string();

        let content = format_tweet_as_nostr_content(&tweet, &[]);

        let expected = "🐦 @testuser: This tweet has > and < and & symbols\n\n\nOriginal tweet: https://twitter.com/i/status/123456789";

        pretty_assertions::assert_eq!(content, expected);
    }

    #[test]
    fn test_format_simple_tweet() {
        let tweet = create_test_tweet();
        let content = format_tweet_as_nostr_content(&tweet, &[]);

        let expected = "🐦 @testuser: This is a test tweet with a link [example.com/article](https://example.com/article)\n\n\nOriginal tweet: https://twitter.com/i/status/123456789";

        pretty_assertions::assert_eq!(content, expected);
    }

    #[test]
    fn test_format_tweet_with_media() {
        let tweet = create_test_tweet();
        let media_urls = vec![
            "https://media.example.com/image1.jpg".to_string(),
            "https://media.example.com/video1.mp4".to_string(),
        ];
        let content = format_tweet_as_nostr_content(&tweet, &media_urls);

        let expected = "🐦 @testuser: This is a test tweet with a link [example.com/article](https://example.com/article)\n\nhttps://media.example.com/image1.jpg\nhttps://media.example.com/video1.mp4\n\nOriginal tweet: https://twitter.com/i/status/123456789";

        pretty_assertions::assert_eq!(content, expected);
    }

    #[test]
    fn test_format_retweet() {
        let mut tweet = create_test_tweet();
        tweet.text = "RT @originaluser: Original tweet content".to_string();
        tweet.referenced_tweets = Some(vec![ReferencedTweet {
            id: "111111111".to_string(),
            type_field: "retweeted".to_string(),
            data: Some(Box::new(Tweet {
                id: "111111111".to_string(),
                text: "Original tweet content".to_string(),
                author: User {
                    id: "888888888".to_string(),
                    name: Some("Original User".to_string()),
                    username: "originaluser".to_string(),
                    profile_image_url: None,
                    description: None,
                    url: None,
                    entities: None,
                },
                referenced_tweets: None,
                attachments: None,
                created_at: "2023-01-01T00:00:00Z".to_string(),
                entities: None,
                includes: None,
                author_id: Some("888888888".to_string()),
                note_tweet: None,
            })),
        }]);

        let content = format_tweet_as_nostr_content(&tweet, &[]);

        let expected = "🔁 @testuser retweeted @originaluser:\nOriginal tweet content\nhttps://twitter.com/i/status/111111111\n\nOriginal tweet: https://twitter.com/i/status/123456789";

        pretty_assertions::assert_eq!(content, expected);
    }

    #[test]
    fn test_format_reply() {
        let mut tweet = create_test_tweet();
        tweet.text = "This is a reply to another tweet".to_string();
        tweet.referenced_tweets = Some(vec![ReferencedTweet {
            id: "222222222".to_string(),
            type_field: "replied_to".to_string(),
            data: Some(Box::new(Tweet {
                id: "222222222".to_string(),
                text: "Original tweet I'm replying to".to_string(),
                author: User {
                    id: "777777777".to_string(),
                    name: Some("Original Author".to_string()),
                    username: "originalauthor".to_string(),
                    profile_image_url: None,
                    description: None,
                    url: None,
                    entities: None,
                },
                referenced_tweets: None,
                attachments: None,
                created_at: "2023-01-01T00:00:00Z".to_string(),
                entities: None,
                includes: None,
                author_id: Some("777777777".to_string()),
                note_tweet: None,
            })),
        }]);

        let content = format_tweet_as_nostr_content(&tweet, &[]);

        assert!(content.contains("🐦 @testuser:"));
        assert!(content.contains("This is a reply to another tweet"));
        assert!(content.contains("↩️ Reply to @originalauthor:"));
        assert!(content.contains("Original tweet I'm replying to"));
    }

    #[test]
    fn test_format_quoted_tweet() {
        let mut tweet = create_test_tweet();
        tweet.text = "Check out this tweet!".to_string();
        tweet.referenced_tweets = Some(vec![ReferencedTweet {
            id: "333333333".to_string(),
            type_field: "quoted".to_string(),
            data: Some(Box::new(Tweet {
                id: "333333333".to_string(),
                text: "The quoted tweet content".to_string(),
                author: User {
                    id: "666666666".to_string(),
                    name: Some("Quoted Author".to_string()),
                    username: "quotedauthor".to_string(),
                    profile_image_url: None,
                    description: None,
                    url: None,
                    entities: None,
                },
                referenced_tweets: None,
                attachments: None,
                created_at: "2023-01-01T00:00:00Z".to_string(),
                entities: None,
                includes: None,
                author_id: Some("666666666".to_string()),
                note_tweet: None,
            })),
        }]);

        let content = format_tweet_as_nostr_content(&tweet, &[]);

        let expected = "🐦 @testuser: Check out this tweet!\n\n💬 Quote of @quotedauthor:\nThe quoted tweet content\nhttps://twitter.com/i/status/333333333\n\nOriginal tweet: https://twitter.com/i/status/123456789";

        pretty_assertions::assert_eq!(content, expected);
    }

    #[test]
    fn test_format_note_tweet() {
        let mut tweet = create_test_tweet();
        tweet.text = "This is a preview...".to_string();
        let long_text =
            "This is a very long tweet that exceeds the normal character limit. ".repeat(10);
        tweet.note_tweet = Some(NoteTweet {
            text: long_text.clone(),
        });

        let content = format_tweet_as_nostr_content(&tweet, &[]);

        let expected = format!(
            "🐦 @testuser: {long_text}\n\n\nOriginal tweet: https://twitter.com/i/status/123456789"
        );

        pretty_assertions::assert_eq!(content, expected);
    }

    #[test]
    fn test_expand_urls_in_text() {
        let text = "Check this out: https://t.co/abc123 and https://t.co/xyz789";
        let entities = Entities {
            urls: Some(vec![
                UrlEntity {
                    url: "https://t.co/abc123".to_string(),
                    expanded_url: "https://example.com/article1".to_string(),
                    display_url: "example.com/article1".to_string(),
                },
                UrlEntity {
                    url: "https://t.co/xyz789".to_string(),
                    expanded_url: "https://example.com/article2".to_string(),
                    display_url: "example.com/article2".to_string(),
                },
            ]),
            hashtags: None,
            mentions: None,
        };

        // Create a simple test tweet for the function call
        let test_tweet = Tweet {
            id: "123".to_string(),
            text: text.to_string(),
            author: User {
                id: "456".to_string(),
                name: Some("Test".to_string()),
                username: "test".to_string(),
                profile_image_url: None,
                description: None,
                url: None,
                entities: None,
            },
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: Some(entities.clone()),
            includes: None,
            author_id: Some("456".to_string()),
            note_tweet: None,
        };

        let (expanded, _) = expand_urls_in_text(text, Some(&entities), &[], &test_tweet);

        assert_eq!(
            expanded,
            "Check this out: [example.com/article1](https://example.com/article1) and [example.com/article2](https://example.com/article2)"
        );
    }

    #[test]
    fn test_expand_urls_no_entities() {
        let text = "No URLs to expand here";
        // Create a simple test tweet for the function call
        let test_tweet = Tweet {
            id: "123".to_string(),
            text: text.to_string(),
            author: User {
                id: "456".to_string(),
                name: Some("Test".to_string()),
                username: "test".to_string(),
                profile_image_url: None,
                description: None,
                url: None,
                entities: None,
            },
            referenced_tweets: None,
            attachments: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            entities: None,
            includes: None,
            author_id: Some("456".to_string()),
            note_tweet: None,
        };
        let (expanded, _) = expand_urls_in_text(text, None, &[], &test_tweet);
        assert_eq!(expanded, text);
    }

    #[test]
    fn test_mime_type_from_path() {
        let jpg_path = Path::new("test.jpg");
        assert_eq!(mime_type_from_path(jpg_path).unwrap(), "image/jpeg");

        let png_path = Path::new("test.png");
        assert_eq!(mime_type_from_path(png_path).unwrap(), "image/png");

        let mp4_path = Path::new("test.mp4");
        assert_eq!(mime_type_from_path(mp4_path).unwrap(), "video/mp4");

        let txt_path = Path::new("test.txt");
        assert_eq!(
            mime_type_from_path(txt_path).unwrap(),
            "application/octet-stream"
        );
    }
}
