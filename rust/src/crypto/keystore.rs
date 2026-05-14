//! Provider-aware persistence for the user's age private key.
//!
//! The private key can live in two places, picked by `Config.key_provider`:
//!
//! - [`Disk`](config::KeyProvider::Disk) — a file at `data_dir/key`, with
//!   read-only fallback discovery for standard SOPS age key files such as
//!   `~/.config/sops/age/keys.txt` and
//!   `~/Library/Application Support/sops/age/keys.txt`.
//! - [`MacosKeychain`](config::KeyProvider::MacosKeychain) — the macOS
//!   Keychain (a `generic-password` entry under
//!   `io.darkmatter.himitsu.agekey.byfp.v1`).
//!
//! Either way the **public key** stays at `data_dir/key.pub` so other
//! commands can compute the fingerprint without unlocking the keychain.
//! The pubkey file is also the canonical "is himitsu initialized?" probe.
//!
//! Most callers don't reach in here directly — they go through
//! [`Context::load_identity`](crate::cli::Context::load_identity), which
//! is the chokepoint that resolves the active provider once.

use std::path::{Path, PathBuf};

use age::x25519::Identity;

use crate::config::KeyProvider as ProviderChoice;
use crate::crypto::age as crypto_age;
use crate::error::{HimitsuError, Result};
use crate::keyring::macos::MacOSKeychain;
use crate::keyring::{fingerprint, KeyProvider};

/// Path to the on-disk public-key file. Always written, regardless of
/// which provider holds the secret.
pub fn pubkey_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("key.pub")
}

/// Path to the on-disk secret-key file. Only populated when the active
/// provider is [`ProviderChoice::Disk`]; with the keychain provider this
/// path doesn't exist.
pub fn disk_secret_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("key")
}

/// Has the user run `himitsu init`? True when a public-key file is
/// present — the secret can live on disk or in the keychain, but the
/// pubkey is always materialised so this is a provider-agnostic probe.
pub fn is_initialized(data_dir: &Path) -> bool {
    pubkey_path(data_dir).exists()
}

/// Persist a freshly-generated keypair under the given provider.
///
/// `pubkey_path` always gets written. `secret`'s landing site depends on
/// `provider`: disk drops it next to the pubkey with the legacy
/// `# comment` header; keychain stores it under the public key's
/// fingerprint and never touches the disk secret file.
pub fn store_new_key(
    provider: &ProviderChoice,
    data_dir: &Path,
    secret: &str,
    pubkey: &str,
    timestamp: &str,
) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(pubkey_path(data_dir), format!("{pubkey}\n"))?;

    match provider {
        ProviderChoice::Disk => {
            std::fs::write(
                disk_secret_path(data_dir),
                format!("# created: {timestamp}\n# public key: {pubkey}\n{secret}\n"),
            )?;
        }
        ProviderChoice::MacosKeychain => {
            ensure_keychain_available()?;
            let fp = fingerprint(pubkey);
            MacOSKeychain.store_key(&fp, secret)?;
        }
    }
    Ok(())
}

/// Load the active identity for the configured provider.
///
/// Disk: parses the first identity from the available disk key files. Keychain:
/// looks up the secret indexed by the disk pubkey's fingerprint, then parses it.
pub fn load_identity(provider: &ProviderChoice, data_dir: &Path) -> Result<Identity> {
    load_identities(provider, data_dir)?
        .into_iter()
        .next()
        .ok_or_else(|| HimitsuError::DecryptionFailed("no age identities available".into()))
}

/// Load every available identity for the configured provider.
///
/// Decryption should use this rather than [`load_identity`] so any key in the
/// himitsu key file or the conventional SOPS age key files can unlock a secret.
pub fn load_identities(provider: &ProviderChoice, data_dir: &Path) -> Result<Vec<Identity>> {
    let mut identities = match provider {
        ProviderChoice::Disk => load_disk_identities(data_dir)?,
        ProviderChoice::MacosKeychain => {
            ensure_keychain_available()?;
            let pubkey = std::fs::read_to_string(pubkey_path(data_dir))
                .map_err(|e| {
                    HimitsuError::Keychain(format!(
                        "no public key file at {} (run `himitsu init`): {e}",
                        pubkey_path(data_dir).display()
                    ))
                })?
                .trim()
                .to_string();
            let fp = fingerprint(&pubkey);
            let secret = MacOSKeychain.load_key(&fp)?.ok_or_else(|| {
                HimitsuError::Keychain(format!(
                    "no key for fingerprint {fp} in macOS Keychain — \
                     run `himitsu init --key-provider macos-keychain` to migrate \
                     from disk, or check that the entry under \
                     io.darkmatter.himitsu.agekey.byfp.v1 / {fp} hasn't been deleted"
                ))
            })?;
            let mut identities = vec![crypto_age::parse_identity(&secret)?];
            identities.extend(load_disk_identities(data_dir).unwrap_or_default());
            identities
        }
    };

    dedupe_identities(&mut identities);
    if identities.is_empty() {
        return Err(HimitsuError::DecryptionFailed(
            "no age identities available".into(),
        ));
    }
    Ok(identities)
}

/// Load disk-backed age identities from himitsu and standard SOPS locations.
fn load_disk_identities(data_dir: &Path) -> Result<Vec<Identity>> {
    let mut identities = Vec::new();
    let mut found_key_file = false;

    for path in disk_identity_paths(data_dir) {
        if path.exists() {
            found_key_file = true;
            identities.extend(crypto_age::read_identities(&path)?);
        }
    }

    if found_key_file {
        Ok(identities)
    } else {
        crypto_age::read_identities(&disk_secret_path(data_dir))
    }
}

fn disk_identity_paths(data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![disk_secret_path(data_dir)];
    paths.extend(disk_identity_fallback_paths());
    paths
}

/// Standard read-only fallback locations for SOPS age key files.
fn disk_identity_fallback_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(config_dir) = dirs::config_dir() {
        push_unique(
            &mut paths,
            config_dir.join("sops").join("age").join("keys.txt"),
        );
    }

    if let Some(home_dir) = dirs::home_dir() {
        push_unique(
            &mut paths,
            home_dir
                .join(".config")
                .join("sops")
                .join("age")
                .join("keys.txt"),
        );
        push_unique(
            &mut paths,
            home_dir
                .join("Library")
                .join("Application Support")
                .join("sops")
                .join("age")
                .join("keys.txt"),
        );
    }

    paths
}

fn dedupe_identities(identities: &mut Vec<Identity>) {
    let mut seen = std::collections::HashSet::new();
    identities.retain(|identity: &Identity| seen.insert(identity.to_public().to_string()));
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

/// Detect a disk-based key that should be migrated to the keychain.
///
/// Returns `true` only when the active provider is keychain, the disk
/// secret file exists, and the keychain has no entry for the same
/// fingerprint yet — i.e. a one-shot migration is wanted, not idempotent
/// rewrites of an already-migrated key.
pub fn needs_disk_to_keychain_migration(
    provider: &ProviderChoice,
    data_dir: &Path,
) -> Result<bool> {
    if !matches!(provider, ProviderChoice::MacosKeychain) {
        return Ok(false);
    }
    if !disk_secret_path(data_dir).exists() {
        return Ok(false);
    }
    if !MacOSKeychain::is_available() {
        return Ok(false);
    }
    let pubkey = std::fs::read_to_string(pubkey_path(data_dir))?
        .trim()
        .to_string();
    let fp = fingerprint(&pubkey);
    Ok(MacOSKeychain.load_key(&fp)?.is_none())
}

/// Move an existing on-disk secret into the keychain, then delete the
/// disk file. The pubkey file is left in place (still needed for
/// fingerprint discovery).
///
/// No-op if the disk secret has already been migrated. Errors out
/// without touching state if the keychain write fails — callers can
/// safely retry.
pub fn migrate_disk_to_keychain(data_dir: &Path) -> Result<()> {
    ensure_keychain_available()?;
    let secret_path = disk_secret_path(data_dir);
    if !secret_path.exists() {
        return Ok(());
    }
    let pubkey = std::fs::read_to_string(pubkey_path(data_dir))?
        .trim()
        .to_string();
    let fp = fingerprint(&pubkey);
    let identity = crypto_age::read_identity(&secret_path)?;
    let secret_str = secrecy::ExposeSecret::expose_secret(&identity.to_string()).to_string();
    MacOSKeychain.store_key(&fp, &secret_str)?;
    // Only remove the disk file once the keychain write succeeded.
    std::fs::remove_file(&secret_path)?;
    Ok(())
}

fn ensure_keychain_available() -> Result<()> {
    if !MacOSKeychain::is_available() {
        return Err(HimitsuError::Keychain(
            "macOS Keychain provider selected but this isn't macOS — \
             switch `key_provider` to `disk` in ~/.config/himitsu/config.yaml"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_initialized_tracks_pubkey_file_only() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_initialized(dir.path()));

        // Secret-only is NOT initialized — pubkey is the canonical probe.
        std::fs::write(disk_secret_path(dir.path()), "AGE-SECRET-KEY-...").unwrap();
        assert!(!is_initialized(dir.path()));

        std::fs::write(pubkey_path(dir.path()), "age1pub").unwrap();
        assert!(is_initialized(dir.path()));
    }

    #[test]
    fn store_new_key_disk_writes_both_files() {
        let dir = tempfile::tempdir().unwrap();
        store_new_key(
            &ProviderChoice::Disk,
            dir.path(),
            "AGE-SECRET-KEY-1ABCDEF",
            "age1publicfake",
            "2026-05-09T12:00:00Z",
        )
        .unwrap();

        let pub_contents = std::fs::read_to_string(pubkey_path(dir.path())).unwrap();
        assert!(pub_contents.contains("age1publicfake"));

        let secret_contents = std::fs::read_to_string(disk_secret_path(dir.path())).unwrap();
        assert!(secret_contents.contains("AGE-SECRET-KEY-1ABCDEF"));
        assert!(secret_contents.contains("# created: 2026-05-09T12:00:00Z"));
        assert!(secret_contents.contains("# public key: age1publicfake"));
    }

    #[test]
    fn fallback_paths_include_common_sops_age_locations() {
        let _guard = crate::config::envs_mut::HIMITSU_CONFIG_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _env = EnvRestore::capture();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));

        let paths = disk_identity_fallback_paths();

        assert!(paths.contains(
            &home
                .path()
                .join(".config")
                .join("sops")
                .join("age")
                .join("keys.txt")
        ));
        assert!(paths.contains(
            &home
                .path()
                .join("Library")
                .join("Application Support")
                .join("sops")
                .join("age")
                .join("keys.txt")
        ));
    }

    #[test]
    fn load_identities_falls_back_to_all_sops_age_keys() {
        let _guard = crate::config::envs_mut::HIMITSU_CONFIG_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _env = EnvRestore::capture();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));

        let data_dir = tempfile::tempdir().unwrap();
        let (secret1, public1) = crate::crypto::age::keygen();
        let (secret2, public2) = crate::crypto::age::keygen();
        let sops_key_path = home
            .path()
            .join(".config")
            .join("sops")
            .join("age")
            .join("keys.txt");
        std::fs::create_dir_all(sops_key_path.parent().unwrap()).unwrap();
        std::fs::write(
            &sops_key_path,
            format!("# sops age keys\n{secret1}\n{secret2}\n"),
        )
        .unwrap();

        let identities = load_identities(&ProviderChoice::Disk, data_dir.path()).unwrap();
        let recipients = vec![crate::crypto::age::parse_recipient(&public2).unwrap()];
        let ciphertext = crate::crypto::age::encrypt(b"secret", &recipients).unwrap();
        let plaintext =
            crate::crypto::age::decrypt_with_identities(&ciphertext, &identities).unwrap();

        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0].to_public().to_string(), public1);
        assert_eq!(plaintext, b"secret");
    }

    #[test]
    fn himitsu_disk_key_takes_precedence_over_sops_fallback() {
        let _guard = crate::config::envs_mut::HIMITSU_CONFIG_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _env = EnvRestore::capture();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", home.path().join(".config"));

        let data_dir = tempfile::tempdir().unwrap();
        let (himitsu_secret, himitsu_public) = crate::crypto::age::keygen();
        let (sops_secret, _sops_public) = crate::crypto::age::keygen();
        std::fs::write(
            disk_secret_path(data_dir.path()),
            format!("{himitsu_secret}\n"),
        )
        .unwrap();

        let sops_key_path = home
            .path()
            .join(".config")
            .join("sops")
            .join("age")
            .join("keys.txt");
        std::fs::create_dir_all(sops_key_path.parent().unwrap()).unwrap();
        std::fs::write(&sops_key_path, format!("{sops_secret}\n")).unwrap();

        let identity = load_identity(&ProviderChoice::Disk, data_dir.path()).unwrap();
        let identities = load_identities(&ProviderChoice::Disk, data_dir.path()).unwrap();
        let recipients = vec![crate::crypto::age::parse_recipient(&_sops_public).unwrap()];
        let ciphertext = crate::crypto::age::encrypt(b"sops-only", &recipients).unwrap();
        let plaintext =
            crate::crypto::age::decrypt_with_identities(&ciphertext, &identities).unwrap();

        assert_eq!(identity.to_public().to_string(), himitsu_public);
        assert_eq!(plaintext, b"sops-only");
    }

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
                std::env::set_var("HOME", home);
            } else {
                std::env::remove_var("HOME");
            }

            if let Some(xdg_config_home) = &self.xdg_config_home {
                std::env::set_var("XDG_CONFIG_HOME", xdg_config_home);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }
        }
    }

    #[test]
    fn migration_predicate_false_for_disk_provider() {
        let dir = tempfile::tempdir().unwrap();
        // Disk provider never migrates regardless of disk state.
        std::fs::write(pubkey_path(dir.path()), "age1pub").unwrap();
        std::fs::write(disk_secret_path(dir.path()), "secret").unwrap();
        assert!(!needs_disk_to_keychain_migration(&ProviderChoice::Disk, dir.path()).unwrap());
    }

    #[test]
    fn migration_predicate_false_when_disk_secret_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(pubkey_path(dir.path()), "age1pub").unwrap();
        // Keychain provider + no on-disk secret = nothing to migrate.
        assert!(
            !needs_disk_to_keychain_migration(&ProviderChoice::MacosKeychain, dir.path()).unwrap()
        );
    }
}
