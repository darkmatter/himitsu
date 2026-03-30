use std::path::{Path, PathBuf};

use crate::error::{HimitsuError, Result};

// ── Store-internal layout ──────────────────────────────────────────────────
//
//   store/.himitsu/secrets/<path>.age
//   store/.himitsu/recipients/<group>/*.pub
//   store/.himitsu/config.yaml

/// Path to the secrets directory inside a store.
pub fn secrets_dir(store: &Path) -> PathBuf {
    store.join(".himitsu").join("secrets")
}

/// Path to the recipients directory inside a store.
pub fn recipients_dir(store: &Path) -> PathBuf {
    store.join(".himitsu").join("recipients")
}

/// Path to the recipients directory, respecting an optional override.
///
/// When `override_path` is `Some(p)`, joins `store` with `p` (making it
/// relative to the store root). When `None`, falls back to the default
/// `.himitsu/recipients/` layout returned by [`recipients_dir`].
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

/// Write an encrypted secret to `.himitsu/secrets/<path>.age`.
pub fn write_secret(store: &Path, secret_path: &str, ciphertext: &[u8]) -> Result<()> {
    let file = secrets_dir(store).join(format!("{secret_path}.age"));
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&file, ciphertext)?;
    Ok(())
}

/// Read an encrypted secret from `.himitsu/secrets/<path>.age`.
pub fn read_secret(store: &Path, secret_path: &str) -> Result<Vec<u8>> {
    let file = secrets_dir(store).join(format!("{secret_path}.age"));
    if !file.exists() {
        return Err(HimitsuError::SecretNotFound(secret_path.to_string()));
    }
    Ok(std::fs::read(&file)?)
}

/// List all secret paths in the store, optionally filtered by a path prefix.
/// Returns paths relative to `secrets_dir` without the `.age` extension.
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

/// Delete a secret from `.himitsu/secrets/<path>.age`.
pub fn delete_secret(store: &Path, secret_path: &str) -> Result<()> {
    let file = secrets_dir(store).join(format!("{secret_path}.age"));
    if !file.exists() {
        return Err(HimitsuError::SecretNotFound(secret_path.to_string()));
    }
    std::fs::remove_file(&file)?;
    Ok(())
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Recursively collect relative paths to `.age` files, stripping the extension.
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
        } else if ft.is_file() && name.ends_with(".age") {
            let key_name = name.strip_suffix(".age").unwrap();
            let full = if prefix.is_empty() {
                key_name.to_string()
            } else {
                format!("{prefix}/{key_name}")
            };
            paths.push(full);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let data = b"encrypted data";
        write_secret(tmp.path(), "prod/API_KEY", data).unwrap();
        let read_back = read_secret(tmp.path(), "prod/API_KEY").unwrap();
        assert_eq!(read_back, data);
    }

    #[test]
    fn read_nonexistent_secret_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let result = read_secret(tmp.path(), "prod/MISSING");
        assert!(result.is_err());
    }

    #[test]
    fn list_secrets_returns_all_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let base = secrets_dir(tmp.path());
        std::fs::create_dir_all(base.join("prod")).unwrap();
        std::fs::write(base.join("prod/API_KEY.age"), b"enc").unwrap();
        std::fs::write(base.join("prod/DB_PASS.age"), b"enc").unwrap();
        std::fs::create_dir_all(base.join("dev")).unwrap();
        std::fs::write(base.join("dev/API_KEY.age"), b"enc").unwrap();
        let paths = list_secrets(tmp.path(), None).unwrap();
        assert_eq!(paths, vec!["dev/API_KEY", "prod/API_KEY", "prod/DB_PASS"]);
    }

    #[test]
    fn list_secrets_filters_by_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let base = secrets_dir(tmp.path());
        std::fs::create_dir_all(base.join("prod")).unwrap();
        std::fs::write(base.join("prod/API_KEY.age"), b"enc").unwrap();
        std::fs::create_dir_all(base.join("dev")).unwrap();
        std::fs::write(base.join("dev/OTHER.age"), b"enc").unwrap();
        let paths = list_secrets(tmp.path(), Some("prod")).unwrap();
        assert_eq!(paths, vec!["prod/API_KEY"]);
    }

    #[test]
    fn list_secrets_handles_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = secrets_dir(tmp.path());
        let nested = base.join("prod/integrations");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("STRIPE_KEY.age"), b"enc").unwrap();
        std::fs::write(base.join("prod/DB_PASS.age"), b"enc").unwrap();

        // create prod first
        std::fs::create_dir_all(base.join("prod")).unwrap();

        let paths = list_secrets(tmp.path(), Some("prod")).unwrap();
        assert_eq!(paths, vec!["prod/DB_PASS", "prod/integrations/STRIPE_KEY"]);
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
    fn delete_secret_removes_file() {
        let tmp = tempfile::tempdir().unwrap();
        write_secret(tmp.path(), "test/KEY", b"data").unwrap();
        assert!(read_secret(tmp.path(), "test/KEY").is_ok());
        delete_secret(tmp.path(), "test/KEY").unwrap();
        assert!(read_secret(tmp.path(), "test/KEY").is_err());
    }
}
