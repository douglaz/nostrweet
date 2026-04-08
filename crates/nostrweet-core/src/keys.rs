use anyhow::{Context, Result};
use bip39::{Language, Mnemonic};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::UserId;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MnemonicPhrase(String);

impl MnemonicPhrase {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        Mnemonic::parse_in(Language::English, trimmed).context("Invalid BIP39 mnemonic phrase")?;
        Ok(Self(trimmed.to_string()))
    }

    fn to_mnemonic(&self) -> Result<Mnemonic> {
        Mnemonic::parse_in(Language::English, &self.0).context("Invalid BIP39 mnemonic phrase")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MnemonicPhrase {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        MnemonicPhrase::parse(&value).map_err(|err| err.to_string())
    }
}

impl From<MnemonicPhrase> for String {
    fn from(value: MnemonicPhrase) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NostrSecretKey([u8; 32]);

impl NostrSecretKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

pub fn account_index_for_user_id(user_id: &UserId) -> u32 {
    let mut hasher = Sha256::new();
    hasher.update(user_id.as_str().as_bytes());
    let hash = hasher.finalize();
    let account_bytes: [u8; 4] = hash[0..4].try_into().unwrap_or([0u8; 4]);
    let account_raw = u32::from_be_bytes(account_bytes);
    account_raw & 0x7FFFFFFF
}

pub fn derive_nostr_secret_key(
    user_id: &UserId,
    mnemonic: &MnemonicPhrase,
    passphrase: Option<&str>,
) -> Result<NostrSecretKey> {
    let mnemonic = mnemonic.to_mnemonic()?;
    let passphrase = passphrase.unwrap_or("");
    let seed = mnemonic.to_seed(passphrase);

    let account = account_index_for_user_id(user_id);

    let mut key_material = Vec::from(&seed[..]);
    key_material.extend_from_slice(&account.to_be_bytes());

    let mut hasher = Sha256::new();
    hasher.update(&key_material);
    let private_key_bytes = hasher.finalize();

    let bytes: [u8; 32] = private_key_bytes
        .as_slice()
        .try_into()
        .context("Failed to derive 32-byte private key")?;

    Ok(NostrSecretKey(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::UserId;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn deterministic_secret_key() -> Result<()> {
        let mnemonic = MnemonicPhrase::parse(TEST_MNEMONIC)?;
        let user_id = UserId::parse("123456")?;
        let key1 = derive_nostr_secret_key(&user_id, &mnemonic, None)?;
        let key2 = derive_nostr_secret_key(&user_id, &mnemonic, None)?;
        assert_eq!(key1, key2);

        let other_user_id = UserId::parse("987654")?;
        let key3 = derive_nostr_secret_key(&other_user_id, &mnemonic, None)?;
        assert_ne!(key1, key3);
        Ok(())
    }

    #[test]
    fn account_index_is_stable() -> Result<()> {
        let user_id = UserId::parse("123456")?;
        let index1 = account_index_for_user_id(&user_id);
        let index2 = account_index_for_user_id(&user_id);
        assert_eq!(index1, index2);
        Ok(())
    }
}
