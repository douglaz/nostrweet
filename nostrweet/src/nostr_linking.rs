use crate::keys::derive_key_for_twitter_user;
use crate::storage::{find_latest_user_profile, load_user_from_file};
use anyhow::{Context, Result};
use nostr_sdk::PublicKey;
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

/// Cache for Twitter username to Nostr pubkey mappings
/// This helps avoid repeated lookups and key derivations
pub struct NostrLinkResolver {
    /// Maps Twitter username to Nostr public key
    username_to_pubkey: HashMap<String, PublicKey>,
    /// Maps Twitter user ID to Nostr public key
    user_id_to_pubkey: HashMap<String, PublicKey>,
    /// Cache directory to search for user profiles
    cache_dir: Option<String>,
}

impl NostrLinkResolver {
    /// Create a new resolver with optional cache directory
    pub fn new(cache_dir: Option<String>) -> Self {
        Self {
            username_to_pubkey: HashMap::new(),
            user_id_to_pubkey: HashMap::new(),
            cache_dir,
        }
    }

    /// Resolve a Twitter username to a Nostr public key
    /// First checks the cache, then looks for a cached user profile,
    /// and finally derives the key if we have the user ID
    pub fn resolve_username(&mut self, username: &str) -> Result<Option<PublicKey>> {
        // Check if we already have this username in cache
        if let Some(pubkey) = self.username_to_pubkey.get(username) {
            return Ok(Some(*pubkey));
        }

        // Try to find the user profile in cache
        if let Some(cache_dir) = &self.cache_dir {
            let cache_path = Path::new(cache_dir);
            if let Ok(Some(profile_path)) = find_latest_user_profile(username, cache_path) {
                debug!(
                    "Found cached profile for @{username} at {profile_path}",
                    profile_path = profile_path.display()
                );

                // Load the user profile to get the user ID
                if let Ok(user) = load_user_from_file(&profile_path) {
                    // Derive the Nostr key from the Twitter user ID
                    let keys = derive_key_for_twitter_user(&user.id).with_context(|| {
                        format!("Failed to derive key for Twitter user {username}")
                    })?;
                    let pubkey = keys.public_key();

                    // Cache both username and user ID mappings
                    self.username_to_pubkey.insert(username.to_string(), pubkey);
                    self.user_id_to_pubkey.insert(user.id.clone(), pubkey);

                    return Ok(Some(pubkey));
                }
            }
        }

        // We couldn't find the user profile
        debug!("Could not resolve Twitter username @{username} to Nostr pubkey");
        Ok(None)
    }

    /// Resolve a Twitter user ID to a Nostr public key
    #[allow(dead_code)] // Used in tests
    pub fn resolve_user_id(&mut self, user_id: &str) -> Result<PublicKey> {
        // Check if we already have this user ID in cache
        if let Some(pubkey) = self.user_id_to_pubkey.get(user_id) {
            return Ok(*pubkey);
        }

        // Derive the key
        let keys = derive_key_for_twitter_user(user_id)
            .with_context(|| format!("Failed to derive key for Twitter user ID {user_id}"))?;
        let pubkey = keys.public_key();

        // Cache the mapping
        self.user_id_to_pubkey.insert(user_id.to_string(), pubkey);

        Ok(pubkey)
    }

    /// Add a known mapping between Twitter username and user ID
    /// This is useful when processing tweets where we know the author
    pub fn add_known_user(&mut self, username: &str, user_id: &str) -> Result<()> {
        let keys = derive_key_for_twitter_user(user_id)
            .with_context(|| format!("Failed to derive key for Twitter user {username}"))?;
        let pubkey = keys.public_key();

        self.username_to_pubkey.insert(username.to_string(), pubkey);
        self.user_id_to_pubkey.insert(user_id.to_string(), pubkey);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::twitter::User;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_nostr_link_resolver() -> Result<()> {
        let mut resolver = NostrLinkResolver::new(None);

        // Test user ID resolution
        let pubkey1 = resolver.resolve_user_id("12345")?;
        let pubkey2 = resolver.resolve_user_id("12345")?;
        assert_eq!(pubkey1, pubkey2); // Should be cached

        // Test adding known user
        resolver.add_known_user("testuser", "12345")?;
        let pubkey3 = resolver.resolve_username("testuser")?;
        assert_eq!(Some(pubkey1), pubkey3);

        Ok(())
    }

    #[test]
    fn test_resolver_with_cache_dir() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let cache_dir = temp_dir.path().to_str().unwrap().to_string();

        // Create a mock user profile in the cache
        let user = User {
            id: "98765".to_string(),
            username: "cacheduser".to_string(),
            name: Some("Cached User".to_string()),
            ..Default::default()
        };

        // Save user profile with expected filename format
        // Note: filename needs exactly 14 characters before username (YYYYMMDDHHMMSS format, no underscores)
        let filename = format!("20240101120000_{}_profile.json", user.username);
        let file_path = temp_dir.path().join(&filename);
        let json = serde_json::to_string(&user)?;
        fs::write(&file_path, json)?;

        // Create resolver with cache directory
        let mut resolver = NostrLinkResolver::new(Some(cache_dir));

        // Resolve username should find cached profile
        let pubkey = resolver.resolve_username("cacheduser")?;
        assert!(pubkey.is_some());

        // Verify it derives the same key as direct user_id resolution
        let direct_pubkey = resolver.resolve_user_id("98765")?;
        assert_eq!(pubkey, Some(direct_pubkey));

        Ok(())
    }

    #[test]
    fn test_deterministic_key_derivation() -> Result<()> {
        let mut resolver1 = NostrLinkResolver::new(None);
        let mut resolver2 = NostrLinkResolver::new(None);

        // Same user ID should produce same pubkey across different resolvers
        let pubkey1 = resolver1.resolve_user_id("555555")?;
        let pubkey2 = resolver2.resolve_user_id("555555")?;
        assert_eq!(pubkey1, pubkey2);

        // Different user IDs should produce different pubkeys
        let pubkey3 = resolver1.resolve_user_id("666666")?;
        assert_ne!(pubkey1, pubkey3);

        Ok(())
    }
}
