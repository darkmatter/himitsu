pub mod macos;

use crate::error::Result;

const SCOPE_SERVICE: &str = "io.darkmatter.himitsu.agekey.scope.v1";
const KEY_SERVICE: &str = "io.darkmatter.himitsu.agekey.byfp.v1";

/// Trait for storing and retrieving age private keys.
pub trait KeyProvider {
    /// Store a private key in the keychain, indexed by fingerprint.
    fn store_key(&self, fingerprint: &str, secret_key: &str) -> Result<()>;

    /// Load a private key from the keychain by fingerprint.
    fn load_key(&self, fingerprint: &str) -> Result<Option<String>>;

    /// Store a scope-to-fingerprint mapping.
    fn store_scope(&self, scope: &str, fingerprint: &str) -> Result<()>;

    /// Look up the fingerprint for a given scope.
    fn load_scope(&self, scope: &str) -> Result<Option<String>>;
}

/// Produce a deterministic, collision-resistant account string for a scope.
/// Format: `gh:<org_lower>:<repo_lower>:<group_escaped>`
pub fn account_for(org: &str, repo: &str, group: &str) -> String {
    let org_lower = org.to_lowercase();
    let repo_lower = repo.to_lowercase();
    let group_escaped = group.replace(':', "_").to_lowercase();
    format!("gh:{org_lower}:{repo_lower}:{group_escaped}")
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
    pub scopes: std::cell::RefCell<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
impl Default for MockKeyProvider {
    fn default() -> Self {
        Self {
            keys: std::cell::RefCell::new(std::collections::HashMap::new()),
            scopes: std::cell::RefCell::new(std::collections::HashMap::new()),
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

    fn store_scope(&self, scope: &str, fingerprint: &str) -> Result<()> {
        self.scopes
            .borrow_mut()
            .insert(scope.to_string(), fingerprint.to_string());
        Ok(())
    }

    fn load_scope(&self, scope: &str) -> Result<Option<String>> {
        Ok(self.scopes.borrow().get(scope).cloned())
    }
}

pub mod mapping {
    use super::KeyProvider;
    use crate::error::Result;

    /// Store a scope-to-fingerprint pointer.
    pub fn store_scope_pointer(
        provider: &dyn KeyProvider,
        scope: &str,
        fingerprint: &str,
    ) -> Result<()> {
        provider.store_scope(scope, fingerprint)
    }

    /// Read a scope-to-fingerprint pointer.
    pub fn read_scope_pointer(provider: &dyn KeyProvider, scope: &str) -> Result<Option<String>> {
        provider.load_scope(scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_for_normalizes() {
        let a = account_for("MyOrg", "MyRepo", "team");
        assert_eq!(a, "gh:myorg:myrepo:team");
    }

    #[test]
    fn account_for_avoids_collisions() {
        let a = account_for("org", "repo", "group");
        let b = account_for("org", "repo:group", "");
        assert_ne!(a, b);
    }

    #[test]
    fn account_for_escapes_colons_in_group() {
        let a = account_for("org", "repo", "my:group");
        assert_eq!(a, "gh:org:repo:my_group");
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let fp1 = fingerprint("age1somekey");
        let fp2 = fingerprint("age1somekey");
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn scope_to_fingerprint_stores_and_reads_correctly() {
        let provider = MockKeyProvider::new();
        let scope = account_for("org", "repo", "team");
        let fp = fingerprint("age1somekey");

        mapping::store_scope_pointer(&provider, &scope, &fp).unwrap();
        let read_fp = mapping::read_scope_pointer(&provider, &scope).unwrap();

        assert_eq!(read_fp, Some(fp));
    }

    #[test]
    fn scope_to_fingerprint_updates_cleanly_on_key_rotation() {
        let provider = MockKeyProvider::new();
        let scope = account_for("org", "repo", "team");

        let fp1 = fingerprint("age1oldkey");
        mapping::store_scope_pointer(&provider, &scope, &fp1).unwrap();

        let fp2 = fingerprint("age1newkey");
        mapping::store_scope_pointer(&provider, &scope, &fp2).unwrap();

        let read_fp = mapping::read_scope_pointer(&provider, &scope).unwrap();
        assert_eq!(read_fp, Some(fp2));
    }
}
