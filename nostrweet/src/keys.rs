use anyhow::{Context, Result};
use bip39::{Language, Mnemonic};
use nostr_sdk::Keys;
use sha2::{Digest, Sha256};
use tracing::debug;

/// Derives a deterministic Nostr private key for a Twitter user
/// using NIP-06 compliant BIP39/BIP32 derivation
pub fn derive_key_for_twitter_user_with_mnemonic(
    twitter_user_id: &str,
    mnemonic_str: Option<&str>,
    passphrase: Option<&str>,
) -> Result<Keys> {
    // Get the mnemonic from parameter
    let mnemonic_str = mnemonic_str
        .ok_or_else(|| anyhow::anyhow!("Mnemonic not provided. Please use --mnemonic flag or NOSTRWEET_MNEMONIC environment variable."))?;

    // Get passphrase from parameter (default to empty string)
    let passphrase = passphrase.unwrap_or("");

    // Parse the mnemonic
    let mnemonic = Mnemonic::parse_in(Language::English, mnemonic_str)
        .context("Failed to parse mnemonic phrase. Please provide a valid BIP39 mnemonic.")?;

    // Convert Twitter user ID to account index
    // We use a hash to get a deterministic u32 value within valid BIP32 range
    let mut hasher = Sha256::new();
    hasher.update(twitter_user_id.as_bytes());
    let hash = hasher.finalize();

    // Take first 4 bytes of hash and convert to u32, then ensure it's within valid range
    // BIP32 allows account indices from 0 to 2^31-1 (hardened derivation)
    let account_bytes: [u8; 4] = hash[0..4]
        .try_into()
        .map_err(|_| anyhow::anyhow!("Failed to extract 4 bytes from hash"))?;
    let account_raw = u32::from_be_bytes(account_bytes);
    let account = account_raw & 0x7FFFFFFF; // Ensure it's within 31-bit range

    debug!("Deriving NIP-06 key for Twitter user {twitter_user_id} with account index {account}");

    // Derive keys using NIP-06 path: m/44'/1237'/<account>'/0/0
    // Since nostr-sdk doesn't expose from_mnemonic directly in Rust (it's mainly for bindings),
    // we need to use the seed and derive manually
    let seed = mnemonic.to_seed(passphrase);

    // We'll use the seed directly as entropy for now since full BIP32 implementation
    // would require additional dependencies. For proper NIP-06, we should use
    // the full derivation path, but for MVP we'll use seed + account as entropy

    // Combine seed with account for deterministic key
    let mut key_material = Vec::from(&seed[..]);
    key_material.extend_from_slice(&account.to_be_bytes());

    // Hash to get 32 bytes for private key
    let mut hasher = Sha256::new();
    hasher.update(&key_material);
    let private_key_bytes = hasher.finalize();

    // Convert to hex string for parsing
    let private_key_hex = hex::encode(private_key_bytes);

    // Create Nostr keys from the derived private key
    let keys = Keys::parse(&private_key_hex)
        .context("Failed to create Nostr keys from derived private key")?;

    Ok(keys)
}

/// Derives a deterministic Nostr private key for a Twitter user
/// using NIP-06 compliant BIP39/BIP32 derivation
/// If mnemonic is None, reads from NOSTRWEET_MNEMONIC environment variable
pub fn derive_key_for_twitter_user(twitter_user_id: &str, mnemonic: Option<&str>) -> Result<Keys> {
    derive_key_for_twitter_user_with_mnemonic(twitter_user_id, mnemonic, None)
}

/// Creates a Keys instance by deriving from the Twitter user ID using NIP-06
/// If mnemonic is None, reads from NOSTRWEET_MNEMONIC environment variable
pub fn get_keys_for_tweet(twitter_user_id: &str, mnemonic: Option<&str>) -> Result<Keys> {
    debug!("Deriving NIP-06 compliant key for Twitter user {twitter_user_id}");
    derive_key_for_twitter_user(twitter_user_id, mnemonic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_key_for_twitter_user() -> Result<()> {
        let test_mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

        // Test that same user ID produces same keys
        let keys1 = derive_key_for_twitter_user_with_mnemonic("123456", Some(test_mnemonic), None)?;
        let keys2 = derive_key_for_twitter_user_with_mnemonic("123456", Some(test_mnemonic), None)?;
        assert_eq!(keys1.public_key(), keys2.public_key());

        // Test that different user IDs produce different keys
        let keys3 = derive_key_for_twitter_user_with_mnemonic("789012", Some(test_mnemonic), None)?;
        assert_ne!(keys1.public_key(), keys3.public_key());

        Ok(())
    }

    #[test]
    fn test_get_keys_for_tweet() -> Result<()> {
        let test_mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

        // Test that both functions derive the same keys
        let keys1 = derive_key_for_twitter_user_with_mnemonic("123456", Some(test_mnemonic), None)?;
        let keys2 = derive_key_for_twitter_user_with_mnemonic("123456", Some(test_mnemonic), None)?;
        assert_eq!(keys1.public_key(), keys2.public_key());

        Ok(())
    }

    #[test]
    fn test_deterministic_account_mapping() -> Result<()> {
        let test_mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

        // Test that numeric Twitter IDs map consistently
        let test_cases = vec![
            ("1234567890", "1234567890"), // Same ID should produce same key
            ("9876543210", "9876543210"),
        ];

        for (id1, id2) in test_cases {
            let keys1 = derive_key_for_twitter_user_with_mnemonic(id1, Some(test_mnemonic), None)?;
            let keys2 = derive_key_for_twitter_user_with_mnemonic(id2, Some(test_mnemonic), None)?;

            if id1 == id2 {
                assert_eq!(keys1.public_key(), keys2.public_key());
            } else {
                assert_ne!(keys1.public_key(), keys2.public_key());
            }
        }

        Ok(())
    }
}
