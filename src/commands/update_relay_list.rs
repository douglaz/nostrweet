use anyhow::Result;
use tracing::info;

use crate::{keys, nostr};

/// Executes the relay list update command.
pub async fn execute(relays: &[String], private_key: Option<&str>) -> Result<()> {
    // Load keys from the configured private key or generate new ones.
    let keys = keys::load_keys(private_key)?;
    let public_key = keys.public_key();
    info!("Using public key: {public_key}");

    // Initialize the Nostr client and connect to the relays.
    let client = nostr::initialize_nostr_client(&keys, relays).await?;

    // Update the relay list on the Nostr network.
    nostr::update_relay_list(&client, &keys, relays).await?;

    // Disconnect from relays
    client.disconnect().await;

    Ok(())
}
