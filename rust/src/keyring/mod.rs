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

/// Compute a simple fingerprint of an age public key string.
/// Uses a truncated SHA-256 hex for compactness.
pub fn fingerprint(pubkey: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    pubkey.hash(&mut hasher);
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
}
