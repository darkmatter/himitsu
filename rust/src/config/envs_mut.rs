//! Config mutation layer for env presets.
//!
//! Thin API the TUI (and any future CLI subcommand) calls to create, update,
//! and delete env presets. Writes YAML atomically, refreshes the SQLite
//! cache, and routes project vs global based on cwd.
//!
//! YAML fidelity: lossy round-trip via `serde_yaml` is accepted —
//! comments/formatting do not survive a mutation. Atomic writes use
//! temp-file + rename in the same directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::env_cache::{EnvCache, Scope};
use crate::config::{
    config_path, validate_env_label, validate_envs, Config, EnvEntry, ProjectConfig,
};
use crate::error::{HimitsuError, Result};

/// Candidate filenames checked when walking up for a project config.
/// Matches the set documented in [`super::find_project_config`].
const PROJECT_CANDIDATES: &[&str] = &[
    ".himitsu.yaml",
    "himitsu.yaml",
    "himitsu.yml",
    ".config/himitsu.yaml",
    ".config/himitsu.yml",
    ".himitsu/config.yaml",
    ".himitsu/config.yml",
];

/// Which config file an env mutation targets.
#[derive(Debug, Clone, Copy)]
pub enum ScopeHint {
    /// Force project scope — errors if no project config is found walking up.
    Project,
    /// Force global scope — writes to `config_dir()/config.yaml`.
    Global,
    /// Auto: project if a project config exists walking up from cwd, else global.
    Auto,
}

/// Resolved scope after inference — what actually got chosen.
#[derive(Debug, Clone)]
pub struct ResolvedScope {
    /// Which scope class (Project | Global).
    pub scope: Scope,
    /// Absolute path to the config file we will read/write.
    pub config_path: PathBuf,
}

/// Walk up from `start` looking for any of [`PROJECT_CANDIDATES`]. Returns
/// the first match found, or `None`. Stops after 20 levels or at the
/// filesystem root.
fn find_project_config_from(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..=20 {
        for candidate in PROJECT_CANDIDATES {
            let path = dir.join(candidate);
            if path.exists() {
                return Some(path);
            }
        }
        if !dir.pop() {
            return None;
        }
    }
    None
}

/// Resolve a scope hint against `cwd`. Pure: does not touch disk beyond
/// existence checks for project config candidates.
pub fn resolve_scope(hint: ScopeHint, cwd: &Path) -> Result<ResolvedScope> {
    match hint {
        ScopeHint::Project => match find_project_config_from(cwd) {
            Some(p) => Ok(ResolvedScope {
                scope: Scope::Project,
                config_path: p,
            }),
            None => Err(HimitsuError::ProjectConfigRequired(format!(
                "no project config (.himitsu.yaml) found walking up from {}",
                cwd.display()
            ))),
        },
        ScopeHint::Global => Ok(ResolvedScope {
            scope: Scope::Global,
            config_path: config_path(),
        }),
        ScopeHint::Auto => {
            if let Some(p) = find_project_config_from(cwd) {
                Ok(ResolvedScope {
                    scope: Scope::Project,
                    config_path: p,
                })
            } else {
                Ok(ResolvedScope {
                    scope: Scope::Global,
                    config_path: config_path(),
                })
            }
        }
    }
}

/// Load the envs map from the resolved config file, treating a missing file
/// as an empty map. The returned struct is a minimal "envs container"
/// regardless of scope so callers can mutate in one place.
fn load_envs(resolved: &ResolvedScope) -> Result<BTreeMap<String, Vec<EnvEntry>>> {
    if !resolved.config_path.exists() {
        return Ok(BTreeMap::new());
    }
    let contents = std::fs::read_to_string(&resolved.config_path)?;
    match resolved.scope {
        Scope::Project => {
            let cfg: ProjectConfig = serde_yaml::from_str(&contents)?;
            Ok(cfg.envs)
        }
        Scope::Global => {
            let cfg: Config = serde_yaml::from_str(&contents)?;
            Ok(cfg.envs)
        }
    }
}

/// Serialize the new envs map back into the file at `resolved.config_path`,
/// preserving any other fields already present. Performs an atomic
/// temp-file + rename write and creates parent directories for the global
/// config if they do not exist yet.
fn write_envs(resolved: &ResolvedScope, new_envs: &BTreeMap<String, Vec<EnvEntry>>) -> Result<()> {
    // Make sure parent exists (common for a fresh global config).
    if let Some(parent) = resolved.config_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let serialized = match resolved.scope {
        Scope::Project => {
            // Load existing on-disk state (if any), update envs, validate,
            // then serialize. Using ProjectConfig preserves sibling fields
            // like `default_store`, `generate`, `store`.
            let mut cfg: ProjectConfig = if resolved.config_path.exists() {
                let contents = std::fs::read_to_string(&resolved.config_path)?;
                serde_yaml::from_str(&contents)?
            } else {
                ProjectConfig::default()
            };
            cfg.envs = new_envs.clone();
            cfg.validate()?;
            serde_yaml::to_string(&cfg)?
        }
        Scope::Global => {
            let mut cfg: Config = if resolved.config_path.exists() {
                let contents = std::fs::read_to_string(&resolved.config_path)?;
                serde_yaml::from_str(&contents)?
            } else {
                Config::default()
            };
            cfg.envs = new_envs.clone();
            cfg.validate()?;
            serde_yaml::to_string(&cfg)?
        }
    };

    // Atomic write via temp + rename in the same directory. Using the
    // target file's parent keeps the rename on a single filesystem.
    let tmp = resolved.config_path.with_extension("yaml.tmp");
    std::fs::write(&tmp, serialized)?;
    std::fs::rename(&tmp, &resolved.config_path)?;
    Ok(())
}

/// Upsert (create or replace) an env label's entries.
///
/// Validates the label and entries, writes the YAML atomically, and
/// refreshes the SQLite cache row for (scope, config_path).
pub fn upsert(
    label: &str,
    entries: Vec<EnvEntry>,
    hint: ScopeHint,
    cwd: &Path,
) -> Result<ResolvedScope> {
    // Validate first so a bad input never touches disk.
    validate_env_label(label)?;
    {
        // Also run the full envs-map validation on a single-entry map so
        // capture-ref rules fire for this label.
        let mut check = BTreeMap::new();
        check.insert(label.to_string(), entries.clone());
        validate_envs(&check)?;
    }

    let resolved = resolve_scope(hint, cwd)?;
    let mut envs = load_envs(&resolved)?;
    envs.insert(label.to_string(), entries);

    // Full-map validation (covers interaction with other labels in the file).
    validate_envs(&envs)?;

    write_envs(&resolved, &envs)?;

    let cache = EnvCache::open()?;
    cache.refresh(&resolved.config_path, resolved.scope, &envs)?;

    Ok(resolved)
}

/// Delete an env label. No-op if the label is absent. Returns the
/// resolved scope that was targeted.
pub fn delete(label: &str, hint: ScopeHint, cwd: &Path) -> Result<ResolvedScope> {
    let resolved = resolve_scope(hint, cwd)?;
    let mut envs = load_envs(&resolved)?;

    if envs.remove(label).is_none() {
        // Nothing changed — skip the write entirely, but still ensure the
        // cache is consistent with the current file contents.
        let cache = EnvCache::open()?;
        cache.refresh(&resolved.config_path, resolved.scope, &envs)?;
        return Ok(resolved);
    }

    validate_envs(&envs)?;
    write_envs(&resolved, &envs)?;

    let cache = EnvCache::open()?;
    cache.refresh(&resolved.config_path, resolved.scope, &envs)?;

    Ok(resolved)
}

/// Read a config file's envs map from disk (global or project). Useful for
/// the TUI to show current state before mutating. Honors [`ScopeHint`] the
/// same way [`upsert`] / [`delete`] do.
pub fn read(
    hint: ScopeHint,
    cwd: &Path,
) -> Result<(ResolvedScope, BTreeMap<String, Vec<EnvEntry>>)> {
    let resolved = resolve_scope(hint, cwd)?;
    let envs = load_envs(&resolved)?;
    Ok((resolved, envs))
}

/// Process-global mutex guarding mutations to `HIMITSU_CONFIG` in tests.
///
/// Several test modules (this one plus `tui::views::envs`) need to swap the
/// global config file path under their feet. Those tests MUST acquire this
/// lock before mutating the env var so they never clobber each other under
/// `cargo test`'s default parallelism.
#[cfg(test)]
pub(crate) static HIMITSU_CONFIG_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    use super::HIMITSU_CONFIG_TEST_GUARD as ENV_GUARD;

    struct HimitsuHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        _tmp: tempfile::TempDir,
        pub path: PathBuf,
    }

    impl HimitsuHome {
        fn new() -> Self {
            let guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            std::env::set_var("HIMITSU_CONFIG", tmp.path().join("config.yaml"));
            let path = tmp.path().to_path_buf();
            Self {
                _guard: guard,
                _tmp: tmp,
                path,
            }
        }
    }

    impl Drop for HimitsuHome {
        fn drop(&mut self) {
            std::env::remove_var("HIMITSU_CONFIG");
        }
    }

    fn single(p: &str) -> EnvEntry {
        EnvEntry::Single(p.into())
    }

    // 1. resolve_scope(Auto, cwd) finds a nearby .himitsu.yaml vs falls back
    //    to global.
    #[test]
    fn resolve_scope_auto_prefers_project_when_present() {
        let home = HimitsuHome::new();
        let proj_dir = home.path.join("proj");
        let sub = proj_dir.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        let proj_cfg = proj_dir.join(".himitsu.yaml");
        std::fs::write(&proj_cfg, "").unwrap();

        let resolved = resolve_scope(ScopeHint::Auto, &sub).unwrap();
        assert_eq!(resolved.scope, Scope::Project);
        assert_eq!(resolved.config_path, proj_cfg);
    }

    #[test]
    fn resolve_scope_auto_falls_back_to_global() {
        let home = HimitsuHome::new();
        let cwd = home.path.join("isolated");
        std::fs::create_dir_all(&cwd).unwrap();

        let resolved = resolve_scope(ScopeHint::Auto, &cwd).unwrap();
        assert_eq!(resolved.scope, Scope::Global);
        assert_eq!(resolved.config_path, config_path());
    }

    // 2. resolve_scope(Project, cwd) errors when no project config exists.
    #[test]
    fn resolve_scope_project_errors_without_config() {
        let home = HimitsuHome::new();
        let cwd = home.path.join("lonely");
        std::fs::create_dir_all(&cwd).unwrap();

        let err = resolve_scope(ScopeHint::Project, &cwd).unwrap_err();
        assert!(matches!(err, HimitsuError::ProjectConfigRequired(_)));
    }

    // 3. upsert creates a new label in project scope — file on disk has the
    //    entry.
    #[test]
    fn upsert_creates_label_in_project_scope() {
        let home = HimitsuHome::new();
        let proj_dir = home.path.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let cfg = proj_dir.join(".himitsu.yaml");
        std::fs::write(&cfg, "default_store: acme/secrets\n").unwrap();

        let resolved = upsert(
            "dev",
            vec![single("dev/API_KEY")],
            ScopeHint::Auto,
            &proj_dir,
        )
        .unwrap();
        assert_eq!(resolved.scope, Scope::Project);

        let on_disk: ProjectConfig =
            serde_yaml::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(on_disk.envs.contains_key("dev"));
        assert_eq!(on_disk.envs["dev"].len(), 1);
        assert_eq!(on_disk.default_store.as_deref(), Some("acme/secrets"));
    }

    // 4. upsert replaces an existing label.
    #[test]
    fn upsert_replaces_existing_label() {
        let home = HimitsuHome::new();
        let proj_dir = home.path.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let cfg = proj_dir.join(".himitsu.yaml");
        std::fs::write(&cfg, "").unwrap();

        upsert("dev", vec![single("dev/OLD")], ScopeHint::Auto, &proj_dir).unwrap();
        upsert("dev", vec![single("dev/NEW")], ScopeHint::Auto, &proj_dir).unwrap();

        let (_res, envs) = read(ScopeHint::Auto, &proj_dir).unwrap();
        let dev = envs.get("dev").unwrap();
        assert_eq!(dev.len(), 1);
        assert!(matches!(&dev[0], EnvEntry::Single(p) if p == "dev/NEW"));
    }

    // 5. upsert with an invalid label returns InvalidConfig and does NOT
    //    touch the file.
    #[test]
    fn upsert_with_invalid_label_does_not_write() {
        let home = HimitsuHome::new();
        let proj_dir = home.path.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let cfg = proj_dir.join(".himitsu.yaml");
        let original = "default_store: acme/secrets\n";
        std::fs::write(&cfg, original).unwrap();

        let err = upsert("foo/*/bar", vec![single("x/y")], ScopeHint::Auto, &proj_dir).unwrap_err();
        assert!(matches!(err, HimitsuError::InvalidConfig(_)));

        // File untouched.
        let on_disk = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(on_disk, original);
        // No stray temp file.
        assert!(!cfg.with_extension("yaml.tmp").exists());
    }

    // 6. upsert in global scope writes to config_path() and the cache has
    //    the row.
    #[test]
    fn upsert_in_global_scope_writes_to_config_path_and_caches() {
        let home = HimitsuHome::new();
        let cwd = home.path.join("anywhere");
        std::fs::create_dir_all(&cwd).unwrap();

        let resolved = upsert(
            "shared",
            vec![single("shared/API")],
            ScopeHint::Global,
            &cwd,
        )
        .unwrap();
        assert_eq!(resolved.scope, Scope::Global);
        assert_eq!(resolved.config_path, config_path());
        assert!(config_path().exists());

        let cache = EnvCache::open().unwrap();
        let got = cache
            .get("shared", Scope::Global, &config_path())
            .unwrap()
            .expect("row should exist");
        assert_eq!(got.label, "shared");
        assert_eq!(got.entries.len(), 1);
    }

    // 7. delete removes a label; delete of an unknown label is a no-op.
    #[test]
    fn delete_removes_label_and_is_noop_for_unknown() {
        let home = HimitsuHome::new();
        let proj_dir = home.path.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join(".himitsu.yaml"), "").unwrap();

        upsert("dev", vec![single("dev/API")], ScopeHint::Auto, &proj_dir).unwrap();

        // Delete known.
        delete("dev", ScopeHint::Auto, &proj_dir).unwrap();
        let (_res, envs) = read(ScopeHint::Auto, &proj_dir).unwrap();
        assert!(!envs.contains_key("dev"));

        // Delete unknown: no error, still no rows.
        delete("nope", ScopeHint::Auto, &proj_dir).unwrap();
        let (_res, envs) = read(ScopeHint::Auto, &proj_dir).unwrap();
        assert!(envs.is_empty());
    }

    // 8. Round-trip: upsert → read returns what was written, order preserved
    //    for entries.
    #[test]
    fn upsert_then_read_round_trip_preserves_order() {
        let home = HimitsuHome::new();
        let proj_dir = home.path.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join(".himitsu.yaml"), "").unwrap();

        let entries = vec![
            single("dev/A"),
            EnvEntry::Alias {
                key: "DB_PASS".into(),
                path: "dev/DB_PASSWORD".into(),
            },
            EnvEntry::Glob("dev".into()),
        ];
        upsert("dev", entries.clone(), ScopeHint::Auto, &proj_dir).unwrap();

        let (_res, envs) = read(ScopeHint::Auto, &proj_dir).unwrap();
        let got = envs.get("dev").unwrap();
        assert_eq!(got.len(), 3);
        assert!(matches!(&got[0], EnvEntry::Single(p) if p == "dev/A"));
        assert!(
            matches!(&got[1], EnvEntry::Alias { key, path } if key == "DB_PASS" && path == "dev/DB_PASSWORD")
        );
        assert!(matches!(&got[2], EnvEntry::Glob(p) if p == "dev"));
    }

    // 9. Atomic write: after a successful write the temp file does not
    //    remain and the target file has the new content.
    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let home = HimitsuHome::new();
        let proj_dir = home.path.join("proj");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let cfg = proj_dir.join(".himitsu.yaml");
        std::fs::write(&cfg, "").unwrap();

        upsert("dev", vec![single("dev/TOKEN")], ScopeHint::Auto, &proj_dir).unwrap();

        assert!(!cfg.with_extension("yaml.tmp").exists());
        let contents = std::fs::read_to_string(&cfg).unwrap();
        assert!(contents.contains("dev/TOKEN"));
    }
}
