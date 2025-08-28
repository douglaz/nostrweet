use anyhow::{Context, Result};
use clap::Args;
use nostr_sdk::{EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::json;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

use crate::{
    datetime_utils, media,
    nostr::{self, format_tweet_as_nostr_content},
    storage, twitter,
};

#[derive(Args, Debug)]
pub struct ShowTweetCommand {
    /// Tweet ID or URL to show
    #[arg(value_name = "TWEET_ID_OR_URL")]
    tweet: String,

    /// Show pretty-printed JSON (default: true)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pretty: bool,

    /// Show compact JSON (opposite of --pretty)
    #[arg(long, action = clap::ArgAction::SetTrue)]
    compact: bool,
}

impl ShowTweetCommand {
    pub async fn execute(self, output_dir: &Path, bearer_token: Option<&str>) -> Result<()> {
        // Parse tweet ID from input (could be ID or URL)
        let tweet_id = twitter::parse_tweet_id(&self.tweet).with_context(|| {
            format!("Failed to parse tweet ID from: {tweet}", tweet = self.tweet)
        })?;

        info!("Showing tweet {tweet_id}");

        // Check if tweet already exists in cache
        let tweet = if let Some(existing_path) =
            storage::find_existing_tweet_json(&tweet_id, output_dir)
        {
            info!(
                "Found cached tweet at: {path}",
                path = existing_path.display()
            );
            storage::load_tweet_from_file(&existing_path)?
        } else {
            info!("Tweet not in cache, downloading...");

            // Create Twitter client and fetch tweet
            let bearer = bearer_token
                .ok_or_else(|| anyhow::anyhow!("Bearer token required for downloading tweets"))?;
            let client = twitter::TwitterClient::new(output_dir, bearer)
                .context("Failed to initialize Twitter client")?;

            let mut downloaded_tweet = client
                .get_tweet(&tweet_id)
                .await
                .context("Failed to download tweet")?;

            // Enrich the tweet with referenced tweet data
            if let Err(e) = client
                .enrich_referenced_tweets(&mut downloaded_tweet, Some(output_dir))
                .await
            {
                debug!("Failed to enrich referenced tweets: {e}");
            }

            // Save to cache
            let save_path = storage::save_tweet(&downloaded_tweet, output_dir)?;
            info!("Saved tweet to: {path}", path = save_path.display());

            downloaded_tweet
        };

        // Extract media URLs from the tweet
        let media_urls = media::extract_media_urls_from_tweet(&tweet);

        // Generate Nostr event
        // Create a temporary key for demonstration (in real usage, user would provide keys)
        let keys = Keys::generate();

        // Format tweet content for Nostr
        let content = format_tweet_as_nostr_content(&tweet, &media_urls);

        // Parse tweet timestamp
        let timestamp = if let Ok(parsed) = datetime_utils::parse_rfc3339(&tweet.created_at) {
            Timestamp::from(parsed.timestamp() as u64)
        } else {
            Timestamp::from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
        };

        // Create tags
        let mut tags = Vec::new();
        let twitter_url = nostr::build_twitter_status_url(&tweet.id);
        tags.push(Tag::parse(vec!["r", &twitter_url])?);

        // Build event
        let mut builder = EventBuilder::new(Kind::TextNote, content).custom_created_at(timestamp);

        for tag in tags {
            builder = builder.tag(tag);
        }

        // Sign the event
        let event = builder.sign(&keys).await?;

        // Create a combined JSON output with both Twitter and Nostr data
        let combined_output = json!({
            "twitter": tweet,
            "nostr": {
                "event": event,
                "metadata": {
                    "original_tweet_id": tweet.id,
                    "original_author": tweet.author.username,
                    "created_at_human": tweet.created_at.clone(),
                    "content_preview": event.content.chars().take(100).collect::<String>() + "...",
                    "tags_count": event.tags.len(),
                    "pubkey_hex": event.pubkey.to_hex(),
                    "event_id_hex": event.id.to_hex()
                }
            }
        });

        let use_pretty = !self.compact; // Default to pretty unless compact is specified

        if use_pretty {
            println!(
                "{json}",
                json = serde_json::to_string_pretty(&combined_output)?
            );
        } else {
            println!("{json}", json = serde_json::to_string(&combined_output)?);
        }

        debug!("Successfully displayed tweet and Nostr event JSON");

        Ok(())
    }
}
