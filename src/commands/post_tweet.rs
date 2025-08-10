use anyhow::{Context, Result};
use std::path::Path;

pub async fn execute(
    tweet_url_or_id: &str,
    relays: &[String],
    blossom_servers: &[String],
    output_dir: &Path,
    force: bool,
    skip_profiles: bool,
) -> Result<()> {
    super::post_tweet_to_nostr::execute(
        tweet_url_or_id,
        relays,
        blossom_servers,
        output_dir,
        force,
        skip_profiles,
    )
    .await
    .context("Failed to post tweet to Nostr")
}
