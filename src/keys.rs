use anyhow::bail;
use anyhow::{Context, Result};
use nostr_sdk::Keys;
use sha2::{Digest, Sha256};
use std::env;
use tracing::debug;

/// Loads keys from a private key string or the environment.
pub fn load_keys(private_key: Option<&str>) -> Result<Keys> {
    let key_str = match private_key {
        Some(k) => Some(k.to_string()),
        None => env::var("NOSTR_PRIVATE_KEY").ok(),
    };

    match key_str {
        Some(k) => Keys::parse(&k).context("Failed to parse private key"),
        None => bail!("No private key provided. Use the --private-key flag or set NOSTR_PRIVATE_KEY environment variable."),
    }
}

/// Derives a deterministic Nostr private key for a Twitter user
/// based on the master seed and the Twitter user ID
pub fn derive_key_for_twitter_user(twitter_user_id: &str) -> Result<Keys> {
    // Get the master seed from environment variable or use a default (only for development)
    let master_seed = env::var("NOSTRWEET_MASTER_SEED").unwrap_or_else(|_| {
        // This is just a fallback for development. In production, always set NOSTRWEET_MASTER_SEED
        debug!("NOSTRWEET_MASTER_SEED not found, using default seed (NOT SECURE FOR PRODUCTION)");
        "NOSTRWEET_DEFAULT_DEVELOPMENT_SEED_DO_NOT_USE_IN_PRODUCTION".to_string()
    });

    // Combine master seed with Twitter user ID
    let seed_material = format!("{master_seed}:{twitter_user_id}");

    // Hash the combined seed to get a deterministic private key
    let mut hasher = Sha256::new();
    hasher.update(seed_material.as_bytes());
    let result = hasher.finalize();

    // Convert to hex string - this will be our private key
    let private_key_hex = hex::encode(result);

    // Create Nostr keys from the derived private key using parse method in nostr-sdk 0.42
    let keys = Keys::parse(&private_key_hex)
        .context("Failed to create Nostr keys from derived private key")?;
    Ok(keys)
}

/// Creates a Keys instance either from a provided private key,
/// or derives one for the Twitter user if no key is provided
pub fn get_keys_for_tweet(twitter_user_id: &str, private_key: Option<&str>) -> Result<Keys> {
    match private_key {
        Some(hex_key) => {
            // Use the explicitly provided private key with parse method in nostr-sdk 0.42
            debug!("Using provided private key");
            let keys =
                Keys::parse(hex_key).context("Failed to create keys from provided private key")?;
            Ok(keys)
        }
        None => {
            // Derive a deterministic key based on the Twitter user ID
            debug!("Deriving deterministic key for Twitter user {twitter_user_id}");
            derive_key_for_twitter_user(twitter_user_id)
        }
    }
}
