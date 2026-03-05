use std::io::{Read, Write};
use std::path::Path;

use ::age::x25519::{Identity, Recipient};
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

    let recipients_boxed: Vec<Box<dyn ::age::Recipient>> = recipients
        .iter()
        .map(|r| Box::new(r.clone()) as Box<dyn ::age::Recipient>)
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
    let decryptor = ::age::Decryptor::new(ciphertext)
        .map_err(|e| HimitsuError::DecryptionFailed(e.to_string()))?;

    let mut plaintext = vec![];
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn ::age::Identity))
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

/// Read an age identity from a key file.
/// The file may contain comments (lines starting with #) and blank lines.
/// The first non-comment, non-blank line is parsed as the secret key.
pub fn read_identity(path: &Path) -> Result<Identity> {
    let contents = std::fs::read_to_string(path)?;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        return parse_identity(trimmed);
    }
    Err(HimitsuError::DecryptionFailed(format!(
        "no secret key found in {}",
        path.display()
    )))
}

/// Read recipient public keys from a directory.
/// Reads all .pub files and parses each as an age recipient.
pub fn read_recipients_from_dir(dir: &Path) -> Result<Vec<Recipient>> {
    let mut recipients = vec![];
    if !dir.exists() {
        return Ok(recipients);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "pub") {
            let contents = std::fs::read_to_string(&path)?;
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                recipients.push(parse_recipient(trimmed)?);
            }
        }
    }
    Ok(recipients)
}

/// Collect all recipients across all groups in a remote's recipients/ directory.
pub fn collect_all_recipients(remote_path: &Path) -> Result<Vec<Recipient>> {
    let recipients_dir = remote_path.join("recipients");
    let mut all = vec![];
    if !recipients_dir.exists() {
        return Ok(all);
    }
    for entry in std::fs::read_dir(&recipients_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let group_recipients = read_recipients_from_dir(&entry.path())?;
            all.extend(group_recipients);
        }
    }
    // Deduplicate by string representation
    all.sort_by_key(|a| a.to_string());
    all.dedup_by(|a, b| a.to_string() == b.to_string());
    Ok(all)
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
}
