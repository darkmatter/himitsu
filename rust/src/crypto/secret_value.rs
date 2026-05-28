//! Helpers for round-tripping [`crate::proto::SecretValue`] through the
//! age ciphertext store.
//!
//! New-format secrets are stored as the protobuf-encoded `SecretValue`
//! inside the age envelope. Pre-existing secrets were stored as raw bytes
//! before `SecretValue` had any user-visible fields; [`decode`] preserves
//! backwards compatibility by falling back to raw bytes whenever the
//! payload is not a populated `SecretValue`.

use std::collections::HashMap;

use prost::Message;

use crate::cli::duration;
use crate::crypto::tags::validate_tag;
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
    /// Arbitrary user-defined key-value pairs.
    pub annotations: HashMap<String, String>,
    /// Free-form tags for filtering and env composition. Empty when unset.
    /// See [`crate::crypto::tags::validate_tag`] for the grammar.
    pub tags: Vec<String>,
    /// Whether a legacy envelope environment field was present and valid.
    pub legacy_environment_detected: bool,
}

impl Decoded {
    /// Did this envelope carry any structured metadata at all?
    pub fn has_metadata(&self) -> bool {
        !self.totp.is_empty()
            || !self.url.is_empty()
            || !self.description.is_empty()
            || !self.env_key.is_empty()
            || !self.annotations.is_empty()
            || !self.tags.is_empty()
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
    decode_with_legacy_environment(plaintext, None)
}

/// Decode plaintext while folding a valid legacy envelope environment into
/// the returned tags without mutating the original envelope.
pub fn decode_with_legacy_environment(
    plaintext: &[u8],
    legacy_environment: Option<&str>,
) -> Decoded {
    match SecretValue::decode(plaintext) {
        Ok(sv) if has_any_field(&sv) => decoded_from_secret_value(sv, legacy_environment),
        _ => Decoded {
            data: plaintext.to_vec(),
            ..Default::default()
        },
    }
}

fn decoded_from_secret_value(sv: SecretValue, legacy_environment: Option<&str>) -> Decoded {
    let SecretValue {
        data,
        totp,
        url,
        description,
        env_key,
        expires_at,
        annotations,
        mut tags,
        ..
    } = sv;

    let legacy_environment_detected = fold_legacy_environment(&mut tags, legacy_environment);

    Decoded {
        data,
        totp,
        url,
        description,
        env_key,
        expires_at,
        annotations,
        tags,
        legacy_environment_detected,
    }
}

fn fold_legacy_environment(tags: &mut Vec<String>, legacy_environment: Option<&str>) -> bool {
    let Some(env_val) = legacy_environment.filter(|env| !env.is_empty()) else {
        return false;
    };

    if validate_tag(env_val).is_err() {
        tracing::warn!(
            env_value = %env_val,
            "legacy environment field contains invalid tag characters; skipping fold"
        );
        return false;
    }

    if !tags.iter().any(|tag| tag == env_val) {
        tags.push(env_val.to_string());
    }
    true
}

fn has_any_field(sv: &SecretValue) -> bool {
    !sv.data.is_empty()
        || !sv.content_type.is_empty()
        || !sv.annotations.is_empty()
        || !sv.totp.is_empty()
        || !sv.url.is_empty()
        || !sv.description.is_empty()
        || !sv.env_key.is_empty()
        || !sv.tags.is_empty()
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
            tags: vec!["pci".to_string(), "stripe".to_string()],
        };
        let bytes = encode(&sv);
        let d = decode(&bytes);
        assert_eq!(d.data, b"abc");
        assert_eq!(d.url, "https://example.com");
        assert_eq!(d.description, "db");
        assert_eq!(d.env_key, "DATABASE_URL");
        assert!(d.totp.starts_with("otpauth://"));
        assert_eq!(d.tags, vec!["pci".to_string(), "stripe".to_string()]);
        assert!(d.has_metadata());
    }

    #[test]
    fn tags_alone_count_as_metadata() {
        let sv = SecretValue {
            data: b"xyz".to_vec(),
            tags: vec!["mobile".to_string()],
            ..Default::default()
        };
        let d = decode(&encode(&sv));
        assert_eq!(d.tags, vec!["mobile".to_string()]);
        assert!(d.has_metadata());
    }

    #[test]
    fn legacy_payload_decodes_to_empty_tags() {
        // A legacy raw-bytes payload that doesn't parse as a populated
        // SecretValue should round-trip with no tags.
        let raw = b"plain old password".to_vec();
        let d = decode(&raw);
        assert_eq!(d.data, raw);
        assert!(d.tags.is_empty());
        assert!(!d.has_metadata());
    }

    #[test]
    fn annotations_round_trip() {
        let mut annotations = HashMap::new();
        annotations.insert("team".to_string(), "backend".to_string());
        annotations.insert("rotation".to_string(), "90d".to_string());
        let sv = SecretValue {
            data: b"pw".to_vec(),
            annotations,
            ..Default::default()
        };
        let d = decode(&encode(&sv));
        assert_eq!(d.annotations.get("team").unwrap(), "backend");
        assert_eq!(d.annotations.get("rotation").unwrap(), "90d");
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

    #[test]
    fn legacy_environment_folds_into_tags() {
        let sv = SecretValue {
            data: b"xyz".to_vec(),
            ..Default::default()
        };
        let d = decode_with_legacy_environment(&encode(&sv), Some("prod"));
        assert_eq!(d.tags, vec!["prod".to_string()]);
        assert!(d.legacy_environment_detected);
    }

    #[test]
    fn invalid_legacy_environment_is_skipped() {
        let sv = SecretValue {
            data: b"xyz".to_vec(),
            ..Default::default()
        };
        let d = decode_with_legacy_environment(&encode(&sv), Some("prod env"));
        assert!(d.tags.is_empty());
        assert!(!d.legacy_environment_detected);
    }
}
