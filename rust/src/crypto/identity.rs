//! Identity resolution across himitsu's key sources.
//!
//! [`IdentityResolver`] is the one place that bridges the key cluster
//! (provider + data dir) and the store cluster (store path + recipients
//! override): decryption must try himitsu's primary age key AND any SOPS age
//! keys discoverable in the store's recipients directory before reporting a
//! decrypt failure.

use std::path::Path;

use crate::config::KeyProvider;
use crate::error::Result;

/// Resolves age identities from himitsu's configured key sources.
pub struct IdentityResolver<'a> {
    data_dir: &'a Path,
    key_provider: &'a KeyProvider,
    store: &'a Path,
    recipients_path: Option<&'a str>,
}

impl<'a> IdentityResolver<'a> {
    pub fn new(
        data_dir: &'a Path,
        key_provider: &'a KeyProvider,
        store: &'a Path,
        recipients_path: Option<&'a str>,
    ) -> Self {
        Self {
            data_dir,
            key_provider,
            store,
            recipients_path,
        }
    }

    /// Load the primary age identity through the active provider.
    pub fn load_primary(&self) -> Result<::age::x25519::Identity> {
        crate::crypto::keystore::load_identity(self.key_provider, self.data_dir)
    }

    /// Load every available identity: the primary key plus any SOPS age keys
    /// discoverable in the store's recipients directory.
    pub fn load_all(&self) -> Result<Vec<::age::x25519::Identity>> {
        let rdir =
            crate::remote::store::recipients_dir_with_override(self.store, self.recipients_path);
        let recipients_dir = if rdir.exists() { Some(rdir) } else { None };
        crate::crypto::keystore::load_identities(
            self.key_provider,
            self.data_dir,
            recipients_dir.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeyProvider as ProviderChoice;
    use std::path::PathBuf;

    /// Serialize env mutation and restore HOME/XDG so disk-fallback SOPS key
    /// discovery can't pick up the developer's real keys during these tests.
    struct EnvRestore {
        home: Option<std::ffi::OsString>,
        xdg_config_home: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn capture() -> Self {
            Self {
                home: std::env::var_os("HOME"),
                xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            if let Some(home) = &self.home {
                crate::test_env::set_var("HOME", home);
            } else {
                crate::test_env::remove_var("HOME");
            }
            if let Some(xdg_config_home) = &self.xdg_config_home {
                crate::test_env::set_var("XDG_CONFIG_HOME", xdg_config_home);
            } else {
                crate::test_env::remove_var("XDG_CONFIG_HOME");
            }
        }
    }

    fn isolate_home() -> (tempfile::TempDir, EnvRestore) {
        let env = EnvRestore::capture();
        let home = tempfile::tempdir().unwrap();
        crate::test_env::set_var("HOME", home.path());
        crate::test_env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));
        (home, env)
    }

    #[test]
    fn load_primary_returns_disk_key() {
        let _guard = crate::config::outputs::outputs_mut::HIMITSU_CONFIG_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _env) = isolate_home();

        let data_dir = tempfile::tempdir().unwrap();
        let (secret, public) = crate::crypto::age::keygen();
        std::fs::write(
            crate::crypto::keystore::disk_secret_path(data_dir.path()),
            format!("{secret}\n"),
        )
        .unwrap();

        let provider = ProviderChoice::Disk;
        let store = PathBuf::from("/nonexistent/store");
        let resolver = IdentityResolver::new(data_dir.path(), &provider, &store, None);

        let identity = resolver.load_primary().unwrap();
        assert_eq!(identity.to_public().to_string(), public);
    }

    #[test]
    fn load_all_with_missing_store_returns_primary_identity() {
        let _guard = crate::config::outputs::outputs_mut::HIMITSU_CONFIG_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (_home, _env) = isolate_home();

        let data_dir = tempfile::tempdir().unwrap();
        let (secret, public) = crate::crypto::age::keygen();
        std::fs::write(
            crate::crypto::keystore::disk_secret_path(data_dir.path()),
            format!("{secret}\n"),
        )
        .unwrap();

        let provider = ProviderChoice::Disk;
        // A store path that does not exist -> recipients_dir resolves to None.
        let store = PathBuf::from("/nonexistent/store");
        let resolver = IdentityResolver::new(data_dir.path(), &provider, &store, None);

        let identities = resolver.load_all().unwrap();
        assert!(
            identities
                .iter()
                .any(|id| id.to_public().to_string() == public)
        );
    }
}
