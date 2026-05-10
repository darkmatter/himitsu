//! Provider-aware persistence for the user's age private key.
//!
//! The private key can live in two places, picked by `Config.key_provider`:
//!
//! - [`Disk`](config::KeyProvider::Disk) — a file at `data_dir/key`.
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

use std::path::Path;

use ::age::x25519::Identity;

use crate::config::KeyProvider as ProviderChoice;
use crate::crypto::age;
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
/// Disk: parses `data_dir/key`.  Keychain: looks up the secret indexed
/// by the disk pubkey's fingerprint, then parses it.
pub fn load_identity(provider: &ProviderChoice, data_dir: &Path) -> Result<Identity> {
    match provider {
        ProviderChoice::Disk => age::read_identity(&disk_secret_path(data_dir)),
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
            age::parse_identity(&secret)
        }
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
    let identity = age::read_identity(&secret_path)?;
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
