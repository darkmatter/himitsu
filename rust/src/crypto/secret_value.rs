//! Helpers for round-tripping [`crate::proto::SecretValue`] through the
//! age ciphertext store.
//!
//! New-format secrets are stored as the protobuf-encoded `SecretValue`
//! inside the age envelope. Pre-existing secrets were stored as raw bytes
//! before `SecretValue` had any user-visible fields; [`decode`] preserves
//! backwards compatibility by falling back to raw bytes whenever the
//! payload is not a populated `SecretValue`.

use prost::Message;

use crate::cli::duration;
use crate::proto::SecretValue;

/// Decoded plaintext from an age envelope.
#[derive(Debug, Clone, Default)]
pub struct Decoded {
    /// The user-visible secret value (UTF-8 bytes for the common case).
    pub data: Vec<u8>,
    /// TOTP secret (otpauth URI or raw base32) — empty when unset.
    pub totp: String,
    /// Associated website / API URL — empty when unset.
    pub url: String,
    /// Human-readable description — empty when unset.
    pub description: String,
    /// Default env var name used when injecting this secret into a process
    /// environment (e.g. for `himitsu exec`). Empty = derive from path.
    pub env_key: String,
    /// Expiration timestamp — `None` when unset.
    pub expires_at: Option<pbjson_types::Timestamp>,
}

impl Decoded {
    /// Did this envelope carry any structured metadata at all?
    pub fn has_metadata(&self) -> bool {
        !self.totp.is_empty()
            || !self.url.is_empty()
            || !self.description.is_empty()
            || !self.env_key.is_empty()
            || self
                .expires_at
                .as_ref()
                .map(|t| !duration::is_unset(t))
                .unwrap_or(false)
    }
}

/// Encode a [`SecretValue`] into wire bytes suitable for age encryption.
pub fn encode(sv: &SecretValue) -> Vec<u8> {
    sv.encode_to_vec()
}

/// Decode a plaintext blob as either a [`SecretValue`] (new format) or
/// raw bytes (legacy format).
///
/// If the blob parses as a `SecretValue` AND at least one field is
/// populated, the structured form is returned. Otherwise the caller
/// gets the raw bytes placed in [`Decoded::data`] untouched.
pub fn decode(plaintext: &[u8]) -> Decoded {
    match SecretValue::decode(plaintext) {
        Ok(sv) if has_any_field(&sv) => Decoded {
            data: sv.data,
            totp: sv.totp,
            url: sv.url,
            description: sv.description,
            env_key: sv.env_key,
            expires_at: sv.expires_at,
        },
        _ => Decoded {
            data: plaintext.to_vec(),
            ..Default::default()
        },
    }
}

fn has_any_field(sv: &SecretValue) -> bool {
    !sv.data.is_empty()
        || !sv.content_type.is_empty()
        || !sv.annotations.is_empty()
        || !sv.totp.is_empty()
        || !sv.url.is_empty()
        || !sv.description.is_empty()
        || !sv.env_key.is_empty()
        || sv
            .expires_at
            .as_ref()
            .map(|t| !duration::is_unset(t))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_legacy_raw_bytes_roundtrip() {
        let legacy = b"hello world";
        let d = decode(legacy);
        assert_eq!(d.data, legacy);
        assert!(!d.has_metadata());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let sv = SecretValue {
            data: b"abc".to_vec(),
            content_type: String::new(),
            annotations: Default::default(),
            totp: "otpauth://totp/Ex?secret=JBSWY3DPEHPK3PXP".to_string(),
            url: "https://example.com".to_string(),
            expires_at: None,
            description: "db".to_string(),
            env_key: "DATABASE_URL".to_string(),
        };
        let bytes = encode(&sv);
        let d = decode(&bytes);
        assert_eq!(d.data, b"abc");
        assert_eq!(d.url, "https://example.com");
        assert_eq!(d.description, "db");
        assert_eq!(d.env_key, "DATABASE_URL");
        assert!(d.totp.starts_with("otpauth://"));
        assert!(d.has_metadata());
    }

    #[test]
    fn env_key_alone_counts_as_metadata() {
        let sv = SecretValue {
            data: b"xyz".to_vec(),
            env_key: "API_TOKEN".to_string(),
            ..Default::default()
        };
        let d = decode(&encode(&sv));
        assert_eq!(d.env_key, "API_TOKEN");
        assert!(d.has_metadata());
    }
}
