use anyhow::{Context, Result};
use clap::Args;
use nostr_sdk::{EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::json;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

use crate::{
    datetime_utils, media,
    nostr::{self, format_tweet_as_nostr_content_with_mentions},
    nostr_linking::NostrLinkResolver,
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
    pub async fn execute(
        self,
        data_dir: &Path,
        bearer_token: Option<&str>,
        mnemonic: Option<&str>,
    ) -> Result<()> {
        // Parse tweet ID from input (could be ID or URL)
        let tweet_id = twitter::parse_tweet_id(&self.tweet).with_context(|| {
            format!("Failed to parse tweet ID from: {tweet}", tweet = self.tweet)
        })?;

        info!("Showing tweet {tweet_id}");

        // Use the new helper function that handles loading from cache or fetching from API
        // with automatic enrichment of referenced tweets
        let tweet = storage::load_or_fetch_tweet(&tweet_id, data_dir, bearer_token)
            .await
            .with_context(|| format!("Failed to load or fetch tweet {tweet_id}"))?;

        // Extract media URLs from the tweet
        let media_urls = media::extract_media_urls_from_tweet(&tweet);

        // Generate Nostr event
        // Create a temporary key for demonstration (in real usage, user would provide keys)
        let keys = Keys::generate();

        // Format tweet content for Nostr with resolver (including mnemonic for mention resolution)
        let data_dir_str = Some(data_dir.to_string_lossy().to_string());
        let mut resolver = NostrLinkResolver::new(data_dir_str, mnemonic.map(|s| s.to_string()));
        let (content, _mentioned_pubkeys) =
            format_tweet_as_nostr_content_with_mentions(&tweet, &media_urls, &mut resolver)?;

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
