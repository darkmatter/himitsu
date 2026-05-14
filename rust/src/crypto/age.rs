use std::io::{Read, Write};
use std::path::Path;

use age::x25519::{Identity, Recipient};
use age::{Identity as AgeIdentityTrait, Recipient as AgeRecipientTrait};
use secrecy::ExposeSecret;

use crate::error::{HimitsuError, Result};

/// Generate a new age x25519 keypair.
/// Returns (secret_key_string, public_key_string).
pub fn keygen() -> (String, String) {
    let identity = Identity::generate();
    let pubkey = identity.to_public();
    let secret = identity.to_string().expose_secret().to_string();
    let public = pubkey.to_string();
    (secret, public)
}

/// Encrypt plaintext for the given recipients.
pub fn encrypt(plaintext: &[u8], recipients: &[Recipient]) -> Result<Vec<u8>> {
    if recipients.is_empty() {
        return Err(HimitsuError::EncryptionFailed(
            "no recipients provided".into(),
        ));
    }

    let recipients_boxed: Vec<Box<dyn AgeRecipientTrait>> = recipients
        .iter()
        .map(|r| Box::new(r.clone()) as Box<dyn AgeRecipientTrait>)
        .collect();

    let encryptor = ::age::Encryptor::with_recipients(recipients_boxed.iter().map(|r| r.as_ref()))
        .map_err(|e| HimitsuError::EncryptionFailed(e.to_string()))?;

    let mut encrypted = vec![];
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .map_err(|e| HimitsuError::EncryptionFailed(e.to_string()))?;
    writer
        .write_all(plaintext)
        .map_err(|e| HimitsuError::EncryptionFailed(e.to_string()))?;
    writer
        .finish()
        .map_err(|e| HimitsuError::EncryptionFailed(e.to_string()))?;

    Ok(encrypted)
}

/// Decrypt ciphertext using the given identity (private key).
pub fn decrypt(ciphertext: &[u8], identity: &Identity) -> Result<Vec<u8>> {
    decrypt_with_identities(ciphertext, std::slice::from_ref(identity))
}

/// Decrypt ciphertext using any of the given identities.
pub fn decrypt_with_identities(ciphertext: &[u8], identities: &[Identity]) -> Result<Vec<u8>> {
    if identities.is_empty() {
        return Err(HimitsuError::DecryptionFailed(
            "no age identities available".into(),
        ));
    }

    let decryptor = ::age::Decryptor::new(ciphertext)
        .map_err(|e| HimitsuError::DecryptionFailed(e.to_string()))?;

    let identity_refs: Vec<&dyn AgeIdentityTrait> = identities
        .iter()
        .map(|identity| identity as &dyn AgeIdentityTrait)
        .collect();

    let mut plaintext = vec![];
    let mut reader = decryptor
        .decrypt(identity_refs.into_iter())
        .map_err(|e| HimitsuError::DecryptionFailed(e.to_string()))?;
    reader
        .read_to_end(&mut plaintext)
        .map_err(|e| HimitsuError::DecryptionFailed(e.to_string()))?;

    Ok(plaintext)
}

/// Parse a recipient public key string (e.g. "age1...").
pub fn parse_recipient(s: &str) -> Result<Recipient> {
    s.parse::<Recipient>()
        .map_err(|e| HimitsuError::Recipient(format!("invalid age public key: {e}")))
}

/// Parse an identity (private key) string (e.g. "AGE-SECRET-KEY-1...").
pub fn parse_identity(s: &str) -> Result<Identity> {
    s.parse::<Identity>()
        .map_err(|e| HimitsuError::DecryptionFailed(format!("invalid age secret key: {e}")))
}

/// Read the first age identity from a key file.
/// The file may contain comments (lines starting with #) and blank lines.
pub fn read_identity(path: &Path) -> Result<Identity> {
    read_identities(path)?.into_iter().next().ok_or_else(|| {
        HimitsuError::DecryptionFailed(format!("no secret key found in {}", path.display()))
    })
}

/// Read every age identity from a key file.
/// The file may contain comments (lines starting with #) and blank lines.
pub fn read_identities(path: &Path) -> Result<Vec<Identity>> {
    let contents = std::fs::read_to_string(path)?;
    let mut identities = Vec::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        identities.push(parse_identity(trimmed)?);
    }
    if identities.is_empty() {
        return Err(HimitsuError::DecryptionFailed(format!(
            "no secret key found in {}",
            path.display()
        )));
    }
    Ok(identities)
}

/// Read recipient public keys from a directory.
///
/// Walks the directory recursively so both the flat layout
/// (`recipients/<name>.pub`) and any legacy group subdirectories
/// (`recipients/<group>/<name>.pub`) are collected.
pub fn read_recipients_from_dir(dir: &Path) -> Result<Vec<Recipient>> {
    let mut recipients = vec![];
    collect_recipients_recursive(dir, &mut recipients)?;
    Ok(recipients)
}

/// Collect all recipients in a store, respecting an optional
/// `recipients_path` override.
///
/// When `recipients_path` is `Some(p)`, the recipients directory is resolved
/// as `store_path.join(p)` instead of the default `.himitsu/recipients/`.
/// See [`crate::remote::store::recipients_dir_with_override`].
pub fn collect_recipients(
    store_path: &Path,
    recipients_path: Option<&str>,
) -> Result<Vec<Recipient>> {
    let dir = crate::remote::store::recipients_dir_with_override(store_path, recipients_path);
    let mut all = vec![];
    collect_recipients_recursive(&dir, &mut all)?;
    // Deduplicate by string representation.
    all.sort_by_key(|a| a.to_string());
    all.dedup_by(|a, b| a.to_string() == b.to_string());
    Ok(all)
}

/// Collect all recipients in a store's `.himitsu/recipients/` directory.
#[allow(dead_code)]
pub fn collect_all_recipients(store_path: &Path) -> Result<Vec<Recipient>> {
    collect_recipients(store_path, None)
}

/// Collect recipients whose path-based name matches a glob-like pattern.
///
/// Supported patterns:
/// - `alice`          — exact match (equivalent to a single recipient)
/// - `ops/*`          — all recipients directly under `ops/`
/// - `ops/**`         — all recipients recursively under `ops/`
/// - `*`              — all recipients (same as `collect_recipients`)
///
/// The recipients directory is resolved the same way as [`collect_recipients`].
pub fn collect_recipients_matching(
    store_path: &Path,
    recipients_path: Option<&str>,
    patterns: &[&str],
) -> Result<Vec<Recipient>> {
    let dir = crate::remote::store::recipients_dir_with_override(store_path, recipients_path);
    if !dir.exists() {
        return Ok(vec![]);
    }

    // Collect all (name, recipient) pairs first.
    let mut all_named: Vec<(String, Recipient)> = vec![];
    collect_named_recipients_recursive(&dir, &dir, &mut all_named)?;

    // Filter by patterns.
    let mut matched: Vec<Recipient> = vec![];
    for (name, recipient) in &all_named {
        for pat in patterns {
            if recipient_matches(name, pat) {
                matched.push(recipient.clone());
                break;
            }
        }
    }

    // Deduplicate.
    matched.sort_by_key(|a| a.to_string());
    matched.dedup_by(|a, b| a.to_string() == b.to_string());
    Ok(matched)
}

/// Check whether a recipient name matches a glob-like pattern.
fn recipient_matches(name: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern == "**" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        // Recursive: anything under prefix/
        return name.starts_with(&format!("{prefix}/")) || name == prefix;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        // Direct children only: prefix/<something> but not prefix/<sub>/<something>
        if let Some(rest) = name.strip_prefix(&format!("{prefix}/")) {
            return !rest.contains('/');
        }
        return false;
    }
    // Exact match.
    name == pattern
}

/// Walk `dir` recursively and collect (path-based-name, Recipient) pairs.
fn collect_named_recipients_recursive(
    base: &Path,
    dir: &Path,
    out: &mut Vec<(String, Recipient)>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_named_recipients_recursive(base, &path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "pub") {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .with_extension("")
                .to_string_lossy()
                .to_string();
            let contents = std::fs::read_to_string(&path)?;
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                out.push((rel, parse_recipient(trimmed)?));
            }
        }
    }
    Ok(())
}

/// (Internal) Walk `dir` recursively and collect every `.pub` file as a
/// recipient, tolerating both the flat layout and legacy group subdirectories.
fn collect_recipients_recursive(dir: &Path, out: &mut Vec<Recipient>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_recipients_recursive(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "pub") {
            let contents = std::fs::read_to_string(&path)?;
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                out.push(parse_recipient(trimmed)?);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keygen_produces_valid_keypair() {
        let (secret, public) = keygen();
        assert!(secret.starts_with("AGE-SECRET-KEY-"));
        assert!(public.starts_with("age1"));
        // Verify they parse back
        parse_identity(&secret).unwrap();
        parse_recipient(&public).unwrap();
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let (secret, public) = keygen();
        let identity = parse_identity(&secret).unwrap();
        let recipient = parse_recipient(&public).unwrap();

        let plaintext = b"hello world";
        let encrypted = encrypt(plaintext, &[recipient]).unwrap();
        let decrypted = decrypt(&encrypted, &identity).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_with_multiple_recipients() {
        let (secret1, public1) = keygen();
        let (_secret2, public2) = keygen();
        let identity1 = parse_identity(&secret1).unwrap();
        let recipient1 = parse_recipient(&public1).unwrap();
        let recipient2 = parse_recipient(&public2).unwrap();

        let plaintext = b"shared secret";
        let encrypted = encrypt(plaintext, &[recipient1, recipient2]).unwrap();
        let decrypted = decrypt(&encrypted, &identity1).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let (_secret1, public1) = keygen();
        let (secret2, _public2) = keygen();
        let wrong_identity = parse_identity(&secret2).unwrap();
        let recipient = parse_recipient(&public1).unwrap();

        let plaintext = b"secret";
        let encrypted = encrypt(plaintext, &[recipient]).unwrap();
        let result = decrypt(&encrypted, &wrong_identity);
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_no_recipients_fails() {
        let result = encrypt(b"test", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn recipient_matches_exact() {
        assert!(super::recipient_matches("alice", "alice"));
        assert!(!super::recipient_matches("alice", "bob"));
        assert!(!super::recipient_matches("ops/alice", "alice"));
    }

    #[test]
    fn recipient_matches_star_glob() {
        assert!(super::recipient_matches("ops/alice", "ops/*"));
        assert!(super::recipient_matches("ops/bob", "ops/*"));
        assert!(!super::recipient_matches("ops/sub/alice", "ops/*"));
        assert!(!super::recipient_matches("dev/alice", "ops/*"));
    }

    #[test]
    fn recipient_matches_double_star_glob() {
        assert!(super::recipient_matches("ops/alice", "ops/**"));
        assert!(super::recipient_matches("ops/sub/alice", "ops/**"));
        assert!(!super::recipient_matches("dev/alice", "ops/**"));
    }

    #[test]
    fn recipient_matches_wildcard_all() {
        assert!(super::recipient_matches("alice", "*"));
        assert!(super::recipient_matches("ops/alice", "*"));
        assert!(super::recipient_matches("alice", "**"));
        assert!(super::recipient_matches("ops/sub/alice", "**"));
    }

    #[test]
    fn collect_recipients_matching_filters_by_pattern() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = tmp.path().to_path_buf();
        let rdir = crate::remote::store::recipients_dir(&store);

        // Create path-based recipients.
        std::fs::create_dir_all(rdir.join("ops")).unwrap();
        std::fs::create_dir_all(rdir.join("dev")).unwrap();

        let (_, pub1) = keygen();
        let (_, pub2) = keygen();
        let (_, pub3) = keygen();

        std::fs::write(rdir.join("ops").join("alice.pub"), format!("{pub1}\n")).unwrap();
        std::fs::write(rdir.join("ops").join("bob.pub"), format!("{pub2}\n")).unwrap();
        std::fs::write(rdir.join("dev").join("carol.pub"), format!("{pub3}\n")).unwrap();

        // Match ops/*
        let ops = collect_recipients_matching(&store, None, &["ops/*"]).unwrap();
        assert_eq!(ops.len(), 2);

        // Match dev/*
        let dev = collect_recipients_matching(&store, None, &["dev/*"]).unwrap();
        assert_eq!(dev.len(), 1);

        // Match all
        let all = collect_recipients_matching(&store, None, &["*"]).unwrap();
        assert_eq!(all.len(), 3);

        // Exact match
        let exact = collect_recipients_matching(&store, None, &["ops/alice"]).unwrap();
        assert_eq!(exact.len(), 1);
    }
}
