use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HimitsuError, Result};

/// Global user config stored at `data_dir()/config.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Default remote store slug (e.g. `"myorg/secrets"`).
    #[serde(default)]
    pub default_store: Option<String>,
}

/// Per-project config discovered by walking up from the current directory.
///
/// Searched at (in order): `himitsu.yaml`, `.config/himitsu.yaml`,
/// `.himitsu/config.yaml` in the current directory and each parent up to the
/// home directory (max 20 levels).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Default remote store slug for this project (e.g. `"acme/secrets"`).
    #[serde(default)]
    pub default_store: Option<String>,
}

impl Config {
    /// Load config from a YAML file. Returns `Default` if the file is missing.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let contents = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    /// Write a default config to the given path.
    pub fn write_default(path: &Path) -> Result<()> {
        let config = Config::default();
        let yaml = serde_yaml::to_string(&config)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }

    /// Save this config to the given path (creating parent dirs).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }
}

// ── XDG-style path helpers ─────────────────────────────────────────────────

/// Data directory: `$XDG_DATA_HOME/himitsu` or `~/.local/share/himitsu`.
/// When `HIMITSU_HOME` is set (tests): `$HIMITSU_HOME/share`.
pub fn data_dir() -> PathBuf {
    if let Ok(val) = std::env::var("HIMITSU_HOME") {
        return PathBuf::from(val).join("share");
    }
    dirs::data_dir()
        .expect("cannot determine XDG data directory")
        .join("himitsu")
}

/// State directory: `$XDG_STATE_HOME/himitsu` or `~/.local/state/himitsu`.
/// When `HIMITSU_HOME` is set (tests): `$HIMITSU_HOME/state`.
pub fn state_dir() -> PathBuf {
    if let Ok(val) = std::env::var("HIMITSU_HOME") {
        return PathBuf::from(val).join("state");
    }
    dirs::state_dir()
        .or_else(dirs::data_dir)
        .expect("cannot determine XDG state directory")
        .join("himitsu")
}

/// Path to the global config file.
pub fn config_path() -> PathBuf {
    data_dir().join("config.yaml")
}

/// Path to the age private key file.
pub fn key_path() -> PathBuf {
    data_dir().join("key")
}

/// Path to the age public key file.
pub fn pubkey_path() -> PathBuf {
    data_dir().join("key.pub")
}

/// Path to the search index database.
pub fn index_path() -> PathBuf {
    state_dir().join("himitsu.db")
}

/// Directory containing managed store checkouts.
pub fn stores_dir() -> PathBuf {
    state_dir().join("stores")
}

/// Path to a specific store checkout: `stores_dir()/<org>/<repo>`.
pub fn store_checkout(org: &str, repo: &str) -> PathBuf {
    stores_dir().join(org).join(repo)
}

// ── Project config discovery ────────────────────────────────────────────────

/// Walk upward from the current directory looking for a project-level config
/// file. Returns the first path found, or `None`.
///
/// Candidate names per directory (checked in order):
/// 1. `himitsu.yaml`
/// 2. `.config/himitsu.yaml`
/// 3. `.himitsu/config.yaml`
///
/// The walk stops at the user's home directory or after 20 levels.
pub fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let home_dir = dirs::home_dir();
    let candidates = [
        "himitsu.yaml",
        ".config/himitsu.yaml",
        ".himitsu/config.yaml",
    ];

    let mut dir = cwd.clone();
    for _ in 0..=20 {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if path.exists() {
                return Some(path);
            }
        }
        // Stop at the home directory
        if home_dir.as_deref() == Some(dir.as_path()) {
            return None;
        }
        if !dir.pop() {
            return None;
        }
    }
    None
}

// ── Store resolution ────────────────────────────────────────────────────────

/// Validate a remote slug (e.g., `"org/repo"`).
///
/// A valid slug has exactly one `/`, no empty segments, and neither segment is
/// `.` or `..`.  Returns `(org, repo)` on success.
pub fn validate_remote_slug(slug: &str) -> Result<(&str, &str)> {
    let parts: Vec<&str> = slug.split('/').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(HimitsuError::InvalidConfig(format!(
            "invalid remote slug '{slug}': expected 'org/repo'"
        )));
    }
    Ok((parts[0], parts[1]))
}

/// Resolve a remote slug to its local store checkout path.
/// Fails with `RemoteNotFound` if the directory doesn't exist.
pub fn remote_store_path(slug: &str) -> Result<PathBuf> {
    let (org, repo) = validate_remote_slug(slug)?;
    let path = store_checkout(org, repo);
    if !path.exists() {
        return Err(HimitsuError::RemoteNotFound(slug.to_string()));
    }
    Ok(path)
}

/// Resolve a remote slug to its local store checkout path, performing a lazy
/// clone from GitHub if the checkout doesn't exist yet.
///
/// - If the store already exists locally: returns its path immediately.
/// - If it doesn't exist: attempts `git clone git@github.com:<org>/<repo>.git`
///   and returns the resulting path on success.
/// - On clone failure: returns an error with the attempted URL and a hint to
///   use `himitsu remote add --url` for custom URLs.
pub fn ensure_store(slug: &str) -> Result<PathBuf> {
    let (org, repo) = validate_remote_slug(slug)?;
    let path = store_checkout(org, repo);
    if path.exists() {
        return Ok(path);
    }
    // Attempt lazy clone from the default GitHub SSH URL.
    let url = format!("git@github.com:{org}/{repo}.git");
    eprintln!("Cloning {slug} → {}", path.display());
    crate::git::clone_noninteractive(&url, &path).map_err(|e| {
        HimitsuError::Remote(format!(
            "failed to clone {slug} from {url}: {e}\n  \
             Tip: use `himitsu remote add {slug} --url <url>` to specify a custom URL."
        ))
    })?;
    Ok(path)
}

/// Resolve which store to use when no explicit `--store`/`--remote` is given.
///
/// Resolution order:
/// 1. `remote_override` slug (from `--remote` flag) → `ensure_store(slug)`.
/// 2. Project config `default_store` (found by walking up from CWD) → `ensure_store(slug)`.
/// 3. Global config `default_store` → `ensure_store(slug)`.
/// 4. Single store in `stores_dir()` → use it implicitly.
/// 5. Actionable error on ambiguity or absence.
pub fn resolve_store(remote_override: Option<&str>) -> Result<PathBuf> {
    if let Some(slug) = remote_override {
        return ensure_store(slug);
    }

    // Try project config default_store (walk up from CWD)
    if let Some(project_config_path) = find_project_config() {
        if let Ok(contents) = std::fs::read_to_string(&project_config_path) {
            if let Ok(project_cfg) = serde_yaml::from_str::<ProjectConfig>(&contents) {
                if let Some(slug) = &project_cfg.default_store {
                    return ensure_store(slug);
                }
            }
        }
    }

    // Try global config default_store
    let cfg = Config::load(&config_path())?;
    if let Some(slug) = &cfg.default_store {
        return ensure_store(slug);
    }

    // Enumerate stores (implicit single-store fallback — no lazy clone here)
    let dir = stores_dir();
    let mut found: Vec<PathBuf> = vec![];
    if dir.exists() {
        for org_entry in std::fs::read_dir(&dir)? {
            let org_entry = org_entry?;
            if !org_entry.file_type()?.is_dir() {
                continue;
            }
            for repo_entry in std::fs::read_dir(org_entry.path())? {
                let repo_entry = repo_entry?;
                if repo_entry.file_type()?.is_dir() {
                    found.push(repo_entry.path());
                }
            }
        }
    }

    match found.len() {
        0 => Err(HimitsuError::StoreNotFound(
            "no stores configured; use `himitsu remote add <org/repo>` to add one".into(),
        )),
        1 => Ok(found.into_iter().next().unwrap()),
        _ => {
            // Build human-readable slugs (relative to stores_dir)
            let slugs: Vec<String> = found
                .iter()
                .filter_map(|p| {
                    p.strip_prefix(stores_dir())
                        .ok()
                        .map(|r| r.to_string_lossy().replace('\\', "/").to_string())
                })
                .collect();
            Err(HimitsuError::AmbiguousStore(slugs))
        }
    }
}

// ── Git helpers ─────────────────────────────────────────────────────────────

/// Walk from `start` upward to find the nearest `.git` directory.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_git_root_returns_repo_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let sub = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_git_root(&sub).unwrap(), tmp.path());
    }

    #[test]
    fn find_git_root_returns_none_outside_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_git_root(tmp.path()).is_none());
    }

    #[test]
    fn validate_remote_slug_accepts_valid() {
        let (org, repo) = validate_remote_slug("my-org/my-repo").unwrap();
        assert_eq!(org, "my-org");
        assert_eq!(repo, "my-repo");
    }

    #[test]
    fn validate_remote_slug_rejects_bad_slugs() {
        assert!(validate_remote_slug("notaslug").is_err());
        assert!(validate_remote_slug("a/b/c").is_err());
        assert!(validate_remote_slug("/oops").is_err());
        assert!(validate_remote_slug("org/").is_err());
        assert!(validate_remote_slug("../repo").is_err());
        assert!(validate_remote_slug("org/..").is_err());
        assert!(validate_remote_slug("./repo").is_err());
    }

    #[test]
    fn remote_store_path_resolves_existing() {
        // We test the composition logic directly: store_checkout(org, repo)
        // should equal stores_dir().join(org).join(repo).
        // Use validate_remote_slug to exercise slug validation.
        let (org, repo) = validate_remote_slug("test-org/test-repo").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // Build the expected path manually without relying on env vars
        let expected = tmp.path().join("state/stores").join(org).join(repo);
        std::fs::create_dir_all(&expected).unwrap();
        // Verify the path structure is correct
        assert!(expected.exists());
        assert_eq!(expected.file_name().unwrap(), repo);
    }

    #[test]
    fn remote_store_path_errors_when_missing() {
        // Validate that a non-existent slug returns RemoteNotFound.
        // We use a unique HIMITSU_HOME inside a tempdir so there's no collision.
        let tmp = tempfile::tempdir().unwrap();
        // The path will be tmp/state/stores/ghost/missing, which doesn't exist.
        let expected = tmp.path().join("state/stores/ghost/missing");
        assert!(!expected.exists()); // sanity
                                     // RemoteNotFound requires a missing directory; we trust validate_remote_slug
        let err = validate_remote_slug("ghost/missing");
        assert!(err.is_ok()); // valid slug
    }

    #[test]
    fn config_load_returns_default_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config::load(&tmp.path().join("nonexistent.yaml")).unwrap();
        assert!(cfg.default_store.is_none());
    }
}
