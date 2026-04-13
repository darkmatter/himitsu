use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HimitsuError, Result};

// ── Store-internal layout ──────────────────────────────────────────────────
//
//   store/.himitsu/secrets/<path>.yaml
//   store/.himitsu/recipients/<group>/*.pub
//   store/.himitsu/config.yaml
//
// Secret files use a SOPS-inspired YAML envelope:
//
//   value: ENC[age,<base64-encoded-age-ciphertext>]
//   sops:
//     created_at: '2024-01-10'
//     lastmodified: '2024-01-15T10:23:45Z'
//     age:
//       - recipient: age1abc...
//     history:
//       - modified_at: '2024-01-10T08:00:00Z'
//         value: ENC[age,<base64-encoded-age-ciphertext>]
//
// The `sops:` block is always plaintext (metadata); only the `value` fields
// inside `ENC[age,...]` wrappers are encrypted.  Binary values are handled
// transparently: age operates on raw bytes, and the ciphertext is base64-
// encoded for storage.
//
// On read, `.age` (legacy binary) files are still accepted so that existing
// stores do not break.  On write, only `.yaml` is produced.

/// Maximum number of historical versions retained per secret.
const MAX_HISTORY: usize = 10;

// ── Envelope types ─────────────────────────────────────────────────────────

/// An age-encrypted value serialised as `ENC[age,<base64>]`.
///
/// The inner bytes are the raw age binary ciphertext.  Base64 encoding makes
/// the value safe to embed in YAML without escaping.
#[derive(Debug, Clone)]
pub struct EncValue(pub Vec<u8>);

impl Serialize for EncValue {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let b64 = STANDARD.encode(&self.0);
        s.serialize_str(&format!("ENC[age,{b64}]"))
    }
}

impl<'de> Deserialize<'de> for EncValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let s = String::deserialize(d)?;
        let inner = s
            .strip_prefix("ENC[age,")
            .and_then(|s| s.strip_suffix(']'))
            .ok_or_else(|| serde::de::Error::custom(format!("expected ENC[age,...], got {s:?}")))?;
        let bytes = STANDARD
            .decode(inner)
            .map_err(|e| serde::de::Error::custom(format!("base64 decode failed: {e}")))?;
        Ok(EncValue(bytes))
    }
}

/// One entry in the version history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// ISO 8601 datetime of the previous write.
    pub modified_at: String,
    /// The previous ciphertext.
    pub value: EncValue,
}

/// Age recipient entry in the `sops:` metadata block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgeRecipientMeta {
    /// The age public key of the recipient.
    pub recipient: String,
}

/// The `himitsu:` metadata block — always stored in plaintext.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HimitsuMeta {
    /// ISO 8601 date when this secret was first created.
    pub created_at: String,
    /// ISO 8601 datetime of the most recent write.
    pub lastmodified: String,
    /// Age recipients that can decrypt this secret.
    pub age: Vec<AgeRecipientMeta>,
    /// Previous versions, newest first, capped at [`MAX_HISTORY`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<HistoryEntry>,
}

/// The full YAML envelope stored as one `.yaml` file per secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretEnvelope {
    /// The current encrypted value.
    pub value: EncValue,
    /// Plaintext metadata block.
    pub himitsu: HimitsuMeta,
}

/// Lightweight metadata returned without loading the ciphertext.
#[derive(Debug, Clone, Default)]
pub struct SecretMeta {
    pub created_at: Option<String>,
    pub lastmodified: Option<String>,
    pub recipients: Vec<String>,
    pub version: Option<u64>,
}

// ── Path helpers ───────────────────────────────────────────────────────────

/// Path to the secrets directory inside a store.
pub fn secrets_dir(store: &Path) -> PathBuf {
    store.join(".himitsu").join("secrets")
}

/// Path to the recipients directory inside a store.
pub fn recipients_dir(store: &Path) -> PathBuf {
    store.join(".himitsu").join("recipients")
}

/// Path to the recipients directory, respecting an optional override.
pub fn recipients_dir_with_override(store: &Path, override_path: Option<&str>) -> PathBuf {
    match override_path {
        Some(p) => store.join(p),
        None => recipients_dir(store),
    }
}

/// Path to the store's own config file.
pub fn store_config_path(store: &Path) -> PathBuf {
    store.join(".himitsu").join("config.yaml")
}

// ── Secret I/O ─────────────────────────────────────────────────────────────

/// Write an encrypted secret to `.himitsu/secrets/<path>.yaml`.
///
/// - On first write: sets `sops.created_at` to today's UTC date.
/// - On subsequent writes: preserves `created_at` and pushes the previous
///   value into `sops.history` (capped at [`MAX_HISTORY`] entries).
/// - Also removes any legacy `.age` file at the same path, if present.
pub fn write_secret(store: &Path, secret_path: &str, ciphertext: &[u8]) -> Result<()> {
    let yaml_path = secrets_dir(store).join(format!("{secret_path}.yaml"));

    // Preserve metadata from any existing envelope.
    let (created_at, history) = if yaml_path.exists() {
        let existing = read_envelope(&yaml_path)?;
        let mut h = existing.himitsu.history;
        h.insert(
            0,
            HistoryEntry {
                modified_at: existing.himitsu.lastmodified.clone(),
                value: existing.value,
            },
        );
        h.truncate(MAX_HISTORY);
        (existing.himitsu.created_at, h)
    } else {
        (today(), vec![])
    };

    // Collect recipient public keys for the sops metadata (informational).
    let age = collect_recipient_strings(store)
        .into_iter()
        .map(|r| AgeRecipientMeta { recipient: r })
        .collect();

    let envelope = SecretEnvelope {
        value: EncValue(ciphertext.to_vec()),
        himitsu: HimitsuMeta {
            created_at,
            lastmodified: utc_datetime(),
            age,
            history,
        },
    };

    if let Some(parent) = yaml_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&yaml_path, serde_yaml::to_string(&envelope)?)?;

    // Remove legacy binary .age file at the same path if it exists.
    let legacy = secrets_dir(store).join(format!("{secret_path}.age"));
    if legacy.exists() {
        let _ = std::fs::remove_file(&legacy);
    }

    Ok(())
}

/// Read the raw age ciphertext from `.himitsu/secrets/<path>.yaml`.
///
/// Falls back to the legacy binary `.age` format for backward compatibility.
pub fn read_secret(store: &Path, secret_path: &str) -> Result<Vec<u8>> {
    let yaml_path = secrets_dir(store).join(format!("{secret_path}.yaml"));
    if yaml_path.exists() {
        return Ok(read_envelope(&yaml_path)?.value.0);
    }

    // Legacy binary age format.
    let age_path = secrets_dir(store).join(format!("{secret_path}.age"));
    if age_path.exists() {
        return Ok(std::fs::read(&age_path)?);
    }

    Err(HimitsuError::SecretNotFound(secret_path.to_string()))
}

/// Read only the plaintext metadata from a secret file (no decryption needed).
///
/// Returns a default `SecretMeta` if the file does not exist or is legacy.
pub fn read_secret_meta(store: &Path, secret_path: &str) -> Result<SecretMeta> {
    let yaml_path = secrets_dir(store).join(format!("{secret_path}.yaml"));
    if yaml_path.exists() {
        let envelope = read_envelope(&yaml_path)?;
        return Ok(SecretMeta {
            created_at: Some(envelope.himitsu.created_at),
            lastmodified: Some(envelope.himitsu.lastmodified),
            recipients: envelope
                .himitsu
                .age
                .into_iter()
                .map(|r| r.recipient)
                .collect(),
            version: None,
        });
    }
    Ok(SecretMeta::default())
}

/// Delete a secret file.
pub fn delete_secret(store: &Path, secret_path: &str) -> Result<()> {
    let yaml_path = secrets_dir(store).join(format!("{secret_path}.yaml"));
    if yaml_path.exists() {
        std::fs::remove_file(&yaml_path)?;
        return Ok(());
    }
    let age_path = secrets_dir(store).join(format!("{secret_path}.age"));
    if age_path.exists() {
        std::fs::remove_file(&age_path)?;
        return Ok(());
    }
    Err(HimitsuError::SecretNotFound(secret_path.to_string()))
}

/// List all secret paths in the store, optionally filtered by a path prefix.
///
/// Returns paths relative to `secrets_dir` without the file extension.
/// Recognises both `.yaml` (current) and `.age` (legacy) files.
pub fn list_secrets(store: &Path, prefix: Option<&str>) -> Result<Vec<String>> {
    let base = secrets_dir(store);
    let mut paths = vec![];
    if !base.exists() {
        return Ok(paths);
    }
    collect_paths_recursive(&base, "", &mut paths)?;
    paths.sort();

    if let Some(pfx) = prefix {
        let pfx_slash = format!("{pfx}/");
        paths.retain(|p| p == pfx || p.starts_with(&pfx_slash));
    }

    Ok(paths)
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn read_envelope(path: &Path) -> Result<SecretEnvelope> {
    let contents = std::fs::read_to_string(path)?;
    serde_yaml::from_str(&contents).map_err(|e| {
        HimitsuError::InvalidConfig(format!(
            "failed to parse secret envelope at {}: {e}",
            path.display()
        ))
    })
}

/// Collect recipient public key strings from all `.pub` files in the store.
fn collect_recipient_strings(store: &Path) -> Vec<String> {
    let mut out = vec![];
    collect_pub_strings_recursive(&recipients_dir(store), &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_pub_strings_recursive(dir: &Path, out: &mut Vec<String>) {
    if !dir.exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = entry.file_type().ok();
        if ft.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
            collect_pub_strings_recursive(&path, out);
        } else if path.extension().is_some_and(|e| e == "pub") {
            if let Ok(s) = std::fs::read_to_string(&path) {
                let trimmed = s.trim().to_string();
                if !trimmed.is_empty() {
                    out.push(trimmed);
                }
            }
        }
    }
}

fn collect_paths_recursive(dir: &Path, prefix: &str, paths: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            let new_prefix = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            collect_paths_recursive(&entry.path(), &new_prefix, paths)?;
        } else if ft.is_file() {
            let key = if let Some(n) = name.strip_suffix(".yaml") {
                n.to_string()
            } else if let Some(n) = name.strip_suffix(".age") {
                n.to_string()
            } else {
                continue;
            };
            let full = if prefix.is_empty() {
                key
            } else {
                format!("{prefix}/{key}")
            };
            paths.push(full);
        }
    }
    Ok(())
}

// ── Date/time helpers (no external date library) ───────────────────────────

/// Current UTC date as `YYYY-MM-DD`.
fn today() -> String {
    civil_date(epoch_secs())
}

/// Current UTC datetime as `YYYY-MM-DDTHH:MM:SSZ`.
fn utc_datetime() -> String {
    let secs = epoch_secs();
    let date = civil_date(secs);
    let hms = secs % 86400;
    let h = hms / 3600;
    let m = (hms % 3600) / 60;
    let s = hms % 60;
    format!("{date}T{h:02}:{m:02}:{s:02}Z")
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Convert Unix epoch seconds to a `YYYY-MM-DD` string.
///
/// Uses Howard Hinnant's civil_from_days algorithm.
/// <https://howardhinnant.github.io/date_algorithms.html>
fn civil_date(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86400) as i32;
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i32 + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".himitsu/secrets")).unwrap();
        std::fs::create_dir_all(dir.path().join(".himitsu/recipients/common")).unwrap();
        dir
    }

    #[test]
    fn write_and_read_secret() {
        let store = make_store();
        let data = b"hello world";
        write_secret(store.path(), "prod/API_KEY", data).unwrap();
        let read_back = read_secret(store.path(), "prod/API_KEY").unwrap();
        assert_eq!(read_back, data);
    }

    #[test]
    fn write_creates_yaml_file() {
        let store = make_store();
        write_secret(store.path(), "prod/API_KEY", b"secret").unwrap();
        assert!(store
            .path()
            .join(".himitsu/secrets/prod/API_KEY.yaml")
            .exists());
    }

    #[test]
    fn yaml_file_contains_enc_prefix() {
        let store = make_store();
        write_secret(store.path(), "prod/KEY", b"val").unwrap();
        let content =
            std::fs::read_to_string(store.path().join(".himitsu/secrets/prod/KEY.yaml")).unwrap();
        assert!(content.contains("ENC[age,"), "content: {content}");
        assert!(content.contains("created_at:"), "content: {content}");
        assert!(content.contains("lastmodified:"), "content: {content}");
    }

    #[test]
    fn second_write_preserves_created_at_and_adds_history() {
        let store = make_store();
        write_secret(store.path(), "prod/KEY", b"v1").unwrap();
        let after_first =
            read_envelope(&store.path().join(".himitsu/secrets/prod/KEY.yaml")).unwrap();
        let created = after_first.himitsu.created_at.clone();

        write_secret(store.path(), "prod/KEY", b"v2").unwrap();
        let after_second =
            read_envelope(&store.path().join(".himitsu/secrets/prod/KEY.yaml")).unwrap();

        assert_eq!(
            after_second.himitsu.created_at, created,
            "created_at must not change"
        );
        assert_eq!(
            after_second.himitsu.history.len(),
            1,
            "should have 1 history entry"
        );
        assert_eq!(after_second.himitsu.history[0].value.0, after_first.value.0);
    }

    #[test]
    fn history_is_capped_at_max() {
        let store = make_store();
        for i in 0..=(MAX_HISTORY + 2) {
            write_secret(store.path(), "k", format!("v{i}").as_bytes()).unwrap();
        }
        let env = read_envelope(&store.path().join(".himitsu/secrets/k.yaml")).unwrap();
        assert_eq!(env.himitsu.history.len(), MAX_HISTORY);
    }

    #[test]
    fn read_nonexistent_secret_fails() {
        let store = make_store();
        let result = read_secret(store.path(), "prod/MISSING");
        assert!(result.is_err());
    }

    #[test]
    fn list_secrets_returns_all_paths() {
        let store = make_store();
        let base = secrets_dir(store.path());
        std::fs::create_dir_all(base.join("prod")).unwrap();
        std::fs::write(base.join("prod/API_KEY.yaml"), "fake: yaml").unwrap();
        std::fs::write(base.join("prod/DB_PASS.yaml"), "fake: yaml").unwrap();
        std::fs::create_dir_all(base.join("dev")).unwrap();
        std::fs::write(base.join("dev/API_KEY.yaml"), "fake: yaml").unwrap();
        let paths = list_secrets(store.path(), None).unwrap();
        assert_eq!(paths, vec!["dev/API_KEY", "prod/API_KEY", "prod/DB_PASS"]);
    }

    #[test]
    fn list_secrets_filters_by_prefix() {
        let store = make_store();
        let base = secrets_dir(store.path());
        std::fs::create_dir_all(base.join("prod")).unwrap();
        std::fs::write(base.join("prod/API_KEY.yaml"), "fake: yaml").unwrap();
        std::fs::create_dir_all(base.join("dev")).unwrap();
        std::fs::write(base.join("dev/OTHER.yaml"), "fake: yaml").unwrap();
        let paths = list_secrets(store.path(), Some("prod")).unwrap();
        assert_eq!(paths, vec!["prod/API_KEY"]);
    }

    #[test]
    fn list_secrets_handles_legacy_age_files() {
        let store = make_store();
        let base = secrets_dir(store.path());
        std::fs::create_dir_all(base.join("prod")).unwrap();
        std::fs::write(base.join("prod/OLD.age"), b"binary").unwrap();
        std::fs::write(base.join("prod/NEW.yaml"), "fake: yaml").unwrap();
        let paths = list_secrets(store.path(), None).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"prod/OLD".to_string()));
        assert!(paths.contains(&"prod/NEW".to_string()));
    }

    #[test]
    fn delete_secret_removes_yaml_file() {
        let store = make_store();
        write_secret(store.path(), "test/KEY", b"data").unwrap();
        assert!(read_secret(store.path(), "test/KEY").is_ok());
        delete_secret(store.path(), "test/KEY").unwrap();
        assert!(read_secret(store.path(), "test/KEY").is_err());
    }

    #[test]
    fn read_secret_meta_returns_created_at() {
        let store = make_store();
        write_secret(store.path(), "prod/KEY", b"val").unwrap();
        let meta = read_secret_meta(store.path(), "prod/KEY").unwrap();
        assert!(meta.created_at.is_some());
        let date = meta.created_at.unwrap();
        assert_eq!(date.len(), 10, "date should be YYYY-MM-DD, got: {date}");
    }

    #[test]
    fn recipients_dir_with_override_uses_custom_path() {
        let tmp = tempfile::tempdir().unwrap();
        let custom = recipients_dir_with_override(tmp.path(), Some("custom/recipients"));
        assert_eq!(custom, tmp.path().join("custom/recipients"));
    }

    #[test]
    fn recipients_dir_with_override_none_uses_default() {
        let tmp = tempfile::tempdir().unwrap();
        let default = recipients_dir_with_override(tmp.path(), None);
        assert_eq!(default, recipients_dir(tmp.path()));
    }

    #[test]
    fn civil_date_known_values() {
        // 2024-01-15 = 19737 days since epoch
        assert_eq!(civil_date(19737 * 86400), "2024-01-15");
        // Unix epoch itself
        assert_eq!(civil_date(0), "1970-01-01");
    }
}
