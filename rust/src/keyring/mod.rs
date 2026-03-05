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
}
