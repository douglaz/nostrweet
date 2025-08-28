use anyhow::{Context, Result};
use tracing::info;

use crate::keys;
use crate::nostr;

/// Executes the relay list update command.
/// Updates the relay list for the master/root key derived from the mnemonic.
pub async fn execute(relays: &[String], mnemonic: Option<&str>) -> Result<()> {
    info!("Updating relay list for master key");

    // Get the master key from mnemonic/private key (using None for root derivation)
    let keys = keys::get_keys_for_tweet("", mnemonic)?; // Empty string gives us the root key

    // Initialize Nostr client with keys and relays
    let client = nostr::initialize_nostr_client(&keys, relays)
        .await
        .context("Failed to initialize Nostr client")?;

    // Update the relay list
    nostr::update_relay_list(&client, &keys, relays)
        .await
        .context("Failed to update relay list")?;

    info!("Successfully updated relay list for master key");
    info!("Public key: {public_key}", public_key = keys.public_key());

    Ok(())
}
