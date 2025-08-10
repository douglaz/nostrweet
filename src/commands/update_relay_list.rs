use anyhow::{bail, Result};

/// Executes the relay list update command.
pub async fn execute(_relays: &[String]) -> Result<()> {
    // This command doesn't make sense without a specific key
    // since relay list is per-pubkey. We should either:
    // 1. Remove this command entirely, or
    // 2. Use a master key derived from mnemonic

    bail!("update-relay-list command is not supported with mnemonic-based key derivation. Each Twitter user has their own derived key and relay list.");
}
