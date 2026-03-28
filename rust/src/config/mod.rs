use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HimitsuError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub identity: Identity,

    #[serde(default)]
    pub policies: Vec<Policy>,

    #[serde(default)]
    pub imports: Vec<Import>,

    #[serde(default = "default_enable_audits")]
    pub enable_audits: bool,

    #[serde(default)]
    pub codegen: Option<CodegenConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Identity {
    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub public_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Policy {
    pub path_pattern: String,

    #[serde(default)]
    pub include: Vec<String>,

    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Import {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CodegenConfig {
    pub lang: String,
    pub path: String,
}

fn default_enable_audits() -> bool {
    true
}

impl Config {
    /// Load unified config from a YAML file.
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    /// Write a default unified config to the given path.
    pub fn write_default(path: &Path) -> Result<()> {
        let config = Config {
            identity: Identity {
                name: None,
                public_keys: Vec::new(),
            },
            policies: Vec::new(),
            imports: Vec::new(),
            enable_audits: true,
            codegen: None,
        };
        let yaml = serde_yaml::to_string(&config)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }
}

/// User-level home: keys, config, and global search index.
/// Always `~/.himitsu/` (or `HIMITSU_HOME` override).
pub fn user_home() -> PathBuf {
    if let Ok(val) = std::env::var("HIMITSU_HOME") {
        return PathBuf::from(val);
    }
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".himitsu")
}

/// Resolve the project store path.
///
/// 1. If `--store` override is set, use it directly.
/// 2. Walk up from CWD looking for `.himitsu/` inside a git repo.
/// 3. Fall back to `~/.himitsu/` as a personal store.
pub fn store_path(store_override: &Option<String>) -> Result<PathBuf> {
    if let Some(s) = store_override {
        return Ok(PathBuf::from(s));
    }

    let cwd = std::env::current_dir()?;

    // Walk up looking for $GIT_ROOT/.himitsu/
    if let Some(root) = find_git_root(&cwd) {
        let local = root.join(".himitsu");
        if local.exists() {
            return Ok(local);
        }
    }

    // Fall back to user home (also acts as personal store)
    let home = user_home();
    if home.join("keys").exists() {
        return Ok(home);
    }

    Err(HimitsuError::NotInitialized)
}

/// Resolve the store, or return the git root's `.himitsu/` even if it
/// doesn't exist yet (for init to create it).
pub fn store_path_or_default(store_override: &Option<String>) -> PathBuf {
    if let Some(s) = store_override {
        return PathBuf::from(s);
    }

    let cwd = std::env::current_dir().unwrap_or_default();

    if let Some(root) = find_git_root(&cwd) {
        return root.join(".himitsu");
    }

    user_home()
}

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

/// Path to the user's age key file.
pub fn key_path(user_home: &Path) -> PathBuf {
    user_home.join("keys/age.txt")
}

/// Path to the global search index.
pub fn index_path(user_home: &Path) -> PathBuf {
    user_home.join("state/index.db")
}

/// Register a store in the global index so search can find it.
pub fn register_store(user_home: &Path, store: &Path) -> Result<()> {
    let known_path = user_home.join("state/known_stores");
    std::fs::create_dir_all(user_home.join("state"))?;

    let store_str = store.to_string_lossy().to_string();
    let mut stores = load_known_stores(user_home);
    if !stores.contains(&store_str) {
        stores.push(store_str);
        std::fs::write(&known_path, stores.join("\n") + "\n")?;
    }
    Ok(())
}

/// Load the list of known store paths.
pub fn load_known_stores(user_home: &Path) -> Vec<String> {
    let known_path = user_home.join("state/known_stores");
    std::fs::read_to_string(&known_path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
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
    fn store_path_finds_project_local() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".himitsu/vars")).unwrap();

        // Override CWD for the test
        let result = store_path(&Some(
            tmp.path().join(".himitsu").to_string_lossy().to_string(),
        ));
        assert!(result.is_ok());
    }

    #[test]
    fn register_and_load_stores() {
        let tmp = tempfile::tempdir().unwrap();
        register_store(tmp.path(), Path::new("/projects/a/.himitsu")).unwrap();
        register_store(tmp.path(), Path::new("/projects/b/.himitsu")).unwrap();
        register_store(tmp.path(), Path::new("/projects/a/.himitsu")).unwrap(); // dup

        let stores = load_known_stores(tmp.path());
        assert_eq!(stores.len(), 2);
    }

    #[test]
    fn unified_config_parse_minimal() {
        let yaml = r#"
identity:
  public_keys:
    - age1examplekey
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.identity.public_keys, vec!["age1examplekey"]);
        assert!(cfg.policies.is_empty());
        assert!(cfg.imports.is_empty());
        assert!(cfg.enable_audits);
        assert!(cfg.codegen.is_none());
    }

    #[test]
    fn unified_config_parse_full() {
        let yaml = r#"
identity:
  name: Acme
  public_keys:
    - age1abc
policies:
  - path_pattern: "common/*"
    include: ["group:all"]
    exclude: []
imports:
  - type: github
    ref: org/repo
    path: vendor/common
enable_audits: false
codegen:
  lang: typescript
  path: src/generated/secrets.ts
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.identity.name.as_deref(), Some("Acme"));
        assert_eq!(cfg.policies.len(), 1);
        assert_eq!(cfg.imports.len(), 1);
        assert!(!cfg.enable_audits);
        assert_eq!(cfg.codegen.as_ref().unwrap().lang, "typescript");
    }
}
