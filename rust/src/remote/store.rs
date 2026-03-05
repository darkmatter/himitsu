use std::path::Path;

use crate::error::{HimitsuError, Result};

/// Write an encrypted secret to `vars/<env>/<key>.age`.
pub fn write_secret(remote_path: &Path, env: &str, key: &str, ciphertext: &[u8]) -> Result<()> {
    let dir = remote_path.join("vars").join(env);
    std::fs::create_dir_all(&dir)?;
    let file_path = dir.join(format!("{key}.age"));
    std::fs::write(&file_path, ciphertext)?;
    Ok(())
}

/// Read an encrypted secret from `vars/<env>/<key>.age`.
pub fn read_secret(remote_path: &Path, env: &str, key: &str) -> Result<Vec<u8>> {
    let file_path = remote_path
        .join("vars")
        .join(env)
        .join(format!("{key}.age"));
    if !file_path.exists() {
        return Err(HimitsuError::SecretNotFound {
            env: env.into(),
            key: key.into(),
        });
    }
    Ok(std::fs::read(&file_path)?)
}

/// List all environments (directories under `vars/`).
pub fn list_envs(remote_path: &Path) -> Result<Vec<String>> {
    let vars_dir = remote_path.join("vars");
    let mut envs = vec![];
    if !vars_dir.exists() {
        return Ok(envs);
    }
    for entry in std::fs::read_dir(&vars_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            envs.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    envs.sort();
    Ok(envs)
}

/// List all secret key names in an environment.
/// Returns key names without the `.age` extension.
pub fn list_secrets(remote_path: &Path, env: &str) -> Result<Vec<String>> {
    let env_dir = remote_path.join("vars").join(env);
    let mut keys = vec![];
    if !env_dir.exists() {
        return Ok(keys);
    }
    collect_secrets_recursive(&env_dir, "", &mut keys)?;
    keys.sort();
    Ok(keys)
}

/// Recursively collect secret names from a directory.
/// Supports nested subdirectories (e.g. `integrations/STRIPE_KEY`).
fn collect_secrets_recursive(dir: &Path, prefix: &str, keys: &mut Vec<String>) -> Result<()> {
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
            collect_secrets_recursive(&entry.path(), &new_prefix, keys)?;
        } else if ft.is_file() && name.ends_with(".age") {
            let key_name = name.strip_suffix(".age").unwrap();
            let full_name = if prefix.is_empty() {
                key_name.to_string()
            } else {
                format!("{prefix}/{key_name}")
            };
            keys.push(full_name);
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
        write_secret(tmp.path(), "prod", "API_KEY", data).unwrap();
        let read_back = read_secret(tmp.path(), "prod", "API_KEY").unwrap();
        assert_eq!(read_back, data);
    }

    #[test]
    fn read_nonexistent_secret_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let result = read_secret(tmp.path(), "prod", "MISSING");
        assert!(result.is_err());
    }

    #[test]
    fn list_envs_returns_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("vars/prod")).unwrap();
        std::fs::create_dir_all(tmp.path().join("vars/dev")).unwrap();
        std::fs::create_dir_all(tmp.path().join("vars/common")).unwrap();
        let envs = list_envs(tmp.path()).unwrap();
        assert_eq!(envs, vec!["common", "dev", "prod"]);
    }

    #[test]
    fn list_secrets_returns_key_names() {
        let tmp = tempfile::tempdir().unwrap();
        let prod = tmp.path().join("vars/prod");
        std::fs::create_dir_all(&prod).unwrap();
        std::fs::write(prod.join("API_KEY.age"), b"enc").unwrap();
        std::fs::write(prod.join("DB_PASS.age"), b"enc").unwrap();
        let keys = list_secrets(tmp.path(), "prod").unwrap();
        assert_eq!(keys, vec!["API_KEY", "DB_PASS"]);
    }

    #[test]
    fn list_secrets_handles_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("vars/prod/integrations");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("STRIPE_KEY.age"), b"enc").unwrap();
        std::fs::write(tmp.path().join("vars/prod/DB_PASS.age"), b"enc").unwrap();
        let keys = list_secrets(tmp.path(), "prod").unwrap();
        assert_eq!(keys, vec!["DB_PASS", "integrations/STRIPE_KEY"]);
    }
}
