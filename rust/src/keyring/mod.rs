pub mod macos;

use crate::error::Result;

const KEY_SERVICE: &str = "io.darkmatter.himitsu.agekey.byfp.v1";

/// Trait for storing and retrieving age private keys.
pub trait KeyProvider {
    /// Store a private key in the keychain, indexed by fingerprint.
    fn store_key(&self, fingerprint: &str, secret_key: &str) -> Result<()>;

    /// Load a private key from the keychain by fingerprint.
    fn load_key(&self, fingerprint: &str) -> Result<Option<String>>;
}

/// Compute a stable fingerprint of an age public key string.
///
/// Takes the first 8 bytes of SHA-256(pubkey.trim()) and encodes them as
/// 16 lowercase hex characters. This is stable across Rust releases and
/// toolchain versions, unlike `DefaultHasher`.
pub fn fingerprint(pubkey: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(pubkey.trim().as_bytes());
    hash[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Legacy fingerprint using `DefaultHasher` — used only for one-shot
/// migration of keychain entries written before the SHA-256 switch.
///
/// **Do not use for new entries.** `DefaultHasher` is not guaranteed to be
/// stable across Rust releases; existing entries may become unreadable after
/// a toolchain upgrade.
pub(crate) fn fingerprint_v1_legacy(pubkey: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    pubkey.trim().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
pub struct MockKeyProvider {
    pub keys: std::cell::RefCell<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
impl Default for MockKeyProvider {
    fn default() -> Self {
        Self {
            keys: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }
}

#[cfg(test)]
impl MockKeyProvider {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
impl KeyProvider for MockKeyProvider {
    fn store_key(&self, fingerprint: &str, secret_key: &str) -> Result<()> {
        self.keys
            .borrow_mut()
            .insert(fingerprint.to_string(), secret_key.to_string());
        Ok(())
    }

    fn load_key(&self, fingerprint: &str) -> Result<Option<String>> {
        Ok(self.keys.borrow().get(fingerprint).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_deterministic() {
        let fp1 = fingerprint("age1somekey");
        let fp2 = fingerprint("age1somekey");
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn fingerprint_is_sha256_based_and_16_hex_chars() {
        // SHA-256("age1somekey") first 8 bytes → 16 hex chars.
        let fp = fingerprint("age1somekey");
        assert_eq!(fp.len(), 16);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_trims_whitespace() {
        assert_eq!(fingerprint("age1somekey"), fingerprint("age1somekey\n"));
        assert_eq!(fingerprint("age1somekey"), fingerprint("  age1somekey  "));
    }

    #[test]
    fn fingerprint_v1_legacy_is_deterministic() {
        let fp1 = fingerprint_v1_legacy("age1somekey");
        let fp2 = fingerprint_v1_legacy("age1somekey");
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn fingerprint_v1_legacy_differs_from_sha256() {
        // The legacy hash must differ from the new one so migration is meaningful.
        let pubkey = "age1somekey";
        assert_ne!(fingerprint(pubkey), fingerprint_v1_legacy(pubkey));
    }
}
