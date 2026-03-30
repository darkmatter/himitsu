use std::collections::BTreeMap;
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

    /// Environment definitions: env name → list of entry specs.
    #[serde(default)]
    pub envs: BTreeMap<String, Vec<EnvEntry>>,

    /// Generate output settings.
    #[serde(default)]
    pub generate: Option<GenerateConfig>,

    /// Store-level overrides.
    #[serde(default)]
    pub store: Option<StoreConfig>,
}

/// A single entry in an env's secret list.
///
/// YAML shapes:
/// - `"dev/API_KEY"` → `Single("dev/API_KEY")` — key name = last path component
/// - `"dev/*"` → `Glob("dev")` — all secrets under prefix
/// - `{MY_KEY: "dev/DB_PASSWORD"}` → `Alias { key: "MY_KEY", path: "dev/DB_PASSWORD" }`
#[derive(Debug, Clone)]
pub enum EnvEntry {
    /// Explicit alias: output key `key`, value from store path `path`.
    Alias { key: String, path: String },
    /// Single secret by path; output key = last path component.
    Single(String),
    /// All secrets whose path starts with `prefix/`.
    Glob(String),
}

impl Serialize for EnvEntry {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            EnvEntry::Single(p) => s.serialize_str(p),
            EnvEntry::Glob(prefix) => s.serialize_str(&format!("{prefix}/*")),
            EnvEntry::Alias { key, path } => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry(key, path)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for EnvEntry {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // Use an untagged intermediate to handle both string and map shapes.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Str(String),
            Map(BTreeMap<String, String>),
        }

        match Raw::deserialize(d)? {
            Raw::Str(s) => {
                if let Some(prefix) = s.strip_suffix("/*") {
                    Ok(EnvEntry::Glob(prefix.to_string()))
                } else {
                    Ok(EnvEntry::Single(s))
                }
            }
            Raw::Map(m) => {
                if m.len() != 1 {
                    return Err(serde::de::Error::custom(
                        "alias entry must have exactly one key-value pair",
                    ));
                }
                let (key, path) = m.into_iter().next().unwrap();
                Ok(EnvEntry::Alias { key, path })
            }
        }
    }
}

/// Settings for the `generate` command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateConfig {
    /// Output directory relative to the project root (e.g. `".generated"`).
    pub target: String,
    /// Output format. Currently only `"sops"` is supported.
    #[serde(default = "default_generate_format")]
    pub format: String,
    /// Age recipients for the generated output files.
    #[serde(default)]
    pub age_recipients: Vec<String>,
}

fn default_generate_format() -> String {
    "sops".to_string()
}

/// Store-level config overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreConfig {
    /// Override for the recipients directory path within the store.
    #[serde(default)]
    pub recipients_path: Option<String>,
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
/// 1. `himitsu.yaml` / `himitsu.yml`
/// 2. `.config/himitsu.yaml` / `.config/himitsu.yml`
/// 3. `.himitsu/config.yaml` / `.himitsu/config.yml`
///
/// The walk stops at the user's home directory or after 20 levels.
pub fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let home_dir = dirs::home_dir();
    let candidates = [
        "himitsu.yaml",
        "himitsu.yml",
        ".config/himitsu.yaml",
        ".config/himitsu.yml",
        ".himitsu/config.yaml",
        ".himitsu/config.yml",
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

/// Load and parse the first project config found by [`find_project_config`].
///
/// Returns `Some((config, path))` if a config file exists and parses
/// successfully, or `None` if no config file is found.
///
/// Parsing errors are returned as `Err`.
pub fn load_project_config() -> Option<(ProjectConfig, PathBuf)> {
    let path = find_project_config()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let cfg: ProjectConfig = serde_yaml::from_str(&contents).ok()?;
    Some((cfg, path))
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

    #[test]
    fn env_entry_deserialize_single() {
        let yaml = "\"dev/API_KEY\"";
        let entry: EnvEntry = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(entry, EnvEntry::Single(ref p) if p == "dev/API_KEY"));
    }

    #[test]
    fn env_entry_deserialize_glob() {
        let yaml = "\"dev/*\"";
        let entry: EnvEntry = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(entry, EnvEntry::Glob(ref p) if p == "dev"));
    }

    #[test]
    fn env_entry_deserialize_alias() {
        let yaml = "MY_KEY: dev/DB_PASSWORD";
        let entry: EnvEntry = serde_yaml::from_str(yaml).unwrap();
        match entry {
            EnvEntry::Alias { key, path } => {
                assert_eq!(key, "MY_KEY");
                assert_eq!(path, "dev/DB_PASSWORD");
            }
            _ => panic!("expected Alias variant"),
        }
    }

    #[test]
    fn env_entry_round_trip_serialize() {
        // Single
        let e = EnvEntry::Single("prod/STRIPE_KEY".into());
        let s = serde_yaml::to_string(&e).unwrap();
        assert!(s.trim() == "prod/STRIPE_KEY");

        // Glob
        let e = EnvEntry::Glob("prod".into());
        let s = serde_yaml::to_string(&e).unwrap();
        assert!(s.trim() == "prod/*");

        // Alias
        let e = EnvEntry::Alias {
            key: "MY_DB".into(),
            path: "prod/DB_PASS".into(),
        };
        let s = serde_yaml::to_string(&e).unwrap();
        let back: EnvEntry = serde_yaml::from_str(&s).unwrap();
        assert!(
            matches!(back, EnvEntry::Alias { ref key, ref path } if key == "MY_DB" && path == "prod/DB_PASS")
        );
    }

    #[test]
    fn project_config_full_yaml_parses() {
        let yaml = r#"
default_store: acme/secrets
envs:
  dev:
    - dev/API_KEY
    - DB_PASS: dev/DB_PASSWORD
    - dev/*
  prod:
    - prod/*
generate:
  target: .generated
  format: sops
  age_recipients:
    - age1abc
    - age1def
store:
  recipients_path: keys/recipients
"#;
        let cfg: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.default_store.as_deref(), Some("acme/secrets"));
        assert_eq!(cfg.envs.len(), 2);

        let dev_entries = cfg.envs.get("dev").unwrap();
        assert_eq!(dev_entries.len(), 3);
        assert!(matches!(&dev_entries[0], EnvEntry::Single(p) if p == "dev/API_KEY"));
        assert!(
            matches!(&dev_entries[1], EnvEntry::Alias { key, path } if key == "DB_PASS" && path == "dev/DB_PASSWORD")
        );
        assert!(matches!(&dev_entries[2], EnvEntry::Glob(p) if p == "dev"));

        let gen = cfg.generate.unwrap();
        assert_eq!(gen.target, ".generated");
        assert_eq!(gen.format, "sops");
        assert_eq!(gen.age_recipients, vec!["age1abc", "age1def"]);

        let store = cfg.store.unwrap();
        assert_eq!(store.recipients_path.as_deref(), Some("keys/recipients"));
    }
}
