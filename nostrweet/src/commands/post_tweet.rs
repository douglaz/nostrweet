use anyhow::{Context, Result};
use std::path::Path;

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
    super::post_tweet_to_nostr::execute(
        tweet_url_or_id,
        relays,
        blossom_servers,
        data_dir,
        force,
        skip_profiles,
        mnemonic,
        bearer_token,
    )
    .await
    .context("Failed to post tweet to Nostr")
}
