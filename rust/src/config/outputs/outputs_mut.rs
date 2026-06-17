//! Config mutation helpers for the `codegen:` block.
//!
//! YAML fidelity matches `envs_mut`: mutations use a lossy serde_yaml
//! round-trip, so comments and custom formatting are not preserved. Map order
//! is deterministic because `OutputsMap` is a `BTreeMap`. Adding the exact same
//! output definition twice is idempotent; adding the same name with different
//! content replaces the previous definition.

use std::path::{Path, PathBuf};

use crate::config::outputs::{OutputDef, OutputsMap};
use crate::config::{Config, ProjectConfig, config_path};
use crate::error::{HimitsuError, Result};

/// Candidate filenames checked when walking up for a project config.
const PROJECT_CANDIDATES: &[&str] = &[
    ".himitsu.yaml",
    "himitsu.yaml",
    "himitsu.yml",
    ".config/himitsu.yaml",
    ".config/himitsu.yml",
    ".himitsu/config.yaml",
    ".himitsu/config.yml",
];

/// Whether the config file is project-scoped or global.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Project-scoped config file (discovered by walking up from cwd).
    Project,
    /// Global config at `config_dir()/config.yaml`.
    Global,
}

/// Which config file an output mutation targets.
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

/// Re-export so callers that name the hint type `OutputScopeHint` still compile.
pub use ScopeHint as OutputScopeHint;

/// Mutex used in tests to serialise mutations to `HIMITSU_CONFIG` / config
/// files so test runs do not stomp on each other.
#[cfg(test)]
pub(crate) static HIMITSU_CONFIG_TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// Resolve a scope hint against `cwd`. Pure: only does existence checks.
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

fn validate_output_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(HimitsuError::InvalidConfig(
            "output name must not be empty".into(),
        ));
    }
    Ok(())
}

fn load_outputs(resolved: &ResolvedScope) -> Result<OutputsMap> {
    if !resolved.config_path.exists() {
        return Ok(OutputsMap::new());
    }
    let contents = std::fs::read_to_string(&resolved.config_path)?;
    let source = resolved.config_path.display().to_string();
    match resolved.scope {
        Scope::Project => {
            let cfg: ProjectConfig = serde_yaml::from_str(&contents)?;
            cfg.reject_legacy_outputs(&source)?;
            Ok(cfg.codegen)
        }
        Scope::Global => {
            let cfg: Config = serde_yaml::from_str(&contents)?;
            cfg.reject_legacy_outputs(&source)?;
            Ok(cfg.codegen)
        }
    }
}

fn write_outputs(resolved: &ResolvedScope, new_outputs: &OutputsMap) -> Result<()> {
    if let Some(parent) = resolved.config_path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }

    // Re-serializing the typed struct silently drops `skip_serializing`
    // legacy fields, so a lingering legacy `outputs:` block must be a
    // hard error here — otherwise a mutation would destroy it.
    let source = resolved.config_path.display().to_string();
    let serialized = match resolved.scope {
        Scope::Project => {
            let mut cfg: ProjectConfig = if resolved.config_path.exists() {
                let contents = std::fs::read_to_string(&resolved.config_path)?;
                serde_yaml::from_str(&contents)?
            } else {
                ProjectConfig::default()
            };
            cfg.reject_legacy_outputs(&source)?;
            cfg.codegen = new_outputs.clone();
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
            cfg.reject_legacy_outputs(&source)?;
            cfg.codegen = new_outputs.clone();
            cfg.validate()?;
            serde_yaml::to_string(&cfg)?
        }
    };

    let tmp = resolved.config_path.with_extension("yaml.tmp");
    std::fs::write(&tmp, serialized)?;
    std::fs::rename(&tmp, &resolved.config_path)?;
    Ok(())
}

/// Add or replace one output definition in an in-memory outputs map.
///
/// Calling this with the same name and definition repeatedly is a no-op after
/// the first call. Calling it with the same name and different definition uses
/// upsert semantics and replaces the existing output.
pub fn add_output_entry(outputs: &mut OutputsMap, name: &str, output: OutputDef) -> Result<()> {
    validate_output_name(name)?;
    outputs.insert(name.to_string(), output);
    Ok(())
}

/// Missing names are a no-op, matching `envs_mut::delete`.
pub fn remove_output(outputs: &mut OutputsMap, name: &str) -> Result<()> {
    validate_output_name(name)?;
    outputs.remove(name);
    Ok(())
}

pub fn upsert_output_entry(
    name: &str,
    output: OutputDef,
    hint: ScopeHint,
    cwd: &Path,
) -> Result<ResolvedScope> {
    let resolved = resolve_scope(hint, cwd)?;
    let mut outputs = load_outputs(&resolved)?;
    add_output_entry(&mut outputs, name, output)?;
    write_outputs(&resolved, &outputs)?;
    Ok(resolved)
}

/// Missing names are a no-op.
pub fn delete_output(name: &str, hint: ScopeHint, cwd: &Path) -> Result<ResolvedScope> {
    let resolved = resolve_scope(hint, cwd)?;
    let mut outputs = load_outputs(&resolved)?;
    remove_output(&mut outputs, name)?;
    write_outputs(&resolved, &outputs)?;
    Ok(resolved)
}

pub fn read_outputs(hint: ScopeHint, cwd: &Path) -> Result<(ResolvedScope, OutputsMap)> {
    let resolved = resolve_scope(hint, cwd)?;
    let outputs = load_outputs(&resolved)?;
    Ok((resolved, outputs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::outputs::{AliasMap, OutputDef, OutputsMap, SelectorEntry};

    fn output(selector: &str) -> OutputDef {
        OutputDef {
            selectors: vec![SelectorEntry(selector.to_string())],
            aliases: AliasMap::new(),
        }
    }

    #[test]
    fn add_output_entry_to_empty_map() {
        let mut outputs = OutputsMap::new();

        add_output_entry(&mut outputs, "pci-prod", output("tag:pci+tag:prod")).unwrap();

        assert_eq!(outputs["pci-prod"], output("tag:pci+tag:prod"));
    }

    #[test]
    fn add_output_entry_to_map_with_existing_entries() {
        let mut outputs = OutputsMap::new();
        add_output_entry(&mut outputs, "dev", output("tag:dev")).unwrap();

        add_output_entry(&mut outputs, "prod", output("tag:prod")).unwrap();

        assert_eq!(outputs["dev"], output("tag:dev"));
        assert_eq!(outputs["prod"], output("tag:prod"));
    }

    #[test]
    fn remove_existing_output_entry() {
        let mut outputs = OutputsMap::new();
        add_output_entry(&mut outputs, "prod", output("tag:prod")).unwrap();

        remove_output(&mut outputs, "prod").unwrap();

        assert!(!outputs.contains_key("prod"));
    }

    #[test]
    fn remove_missing_output_entry_is_noop() {
        let mut outputs = OutputsMap::new();

        remove_output(&mut outputs, "missing").unwrap();

        assert!(outputs.is_empty());
    }

    #[test]
    fn add_output_entry_round_trips_through_yaml() {
        let mut outputs = OutputsMap::new();
        add_output_entry(&mut outputs, "prod", output("tag:prod")).unwrap();

        let yaml = serde_yaml::to_string(&outputs).unwrap();
        let reparsed: OutputsMap = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(reparsed, outputs);
    }

    #[test]
    fn add_same_output_entry_twice_is_idempotent() {
        let mut once = OutputsMap::new();
        add_output_entry(&mut once, "prod", output("tag:prod")).unwrap();

        let mut twice = OutputsMap::new();
        add_output_entry(&mut twice, "prod", output("tag:prod")).unwrap();
        add_output_entry(&mut twice, "prod", output("tag:prod")).unwrap();

        assert_eq!(twice, once);
    }

    /// hm-66a: load/write must hard-error on a legacy `outputs:` block.
    /// Re-serializing the typed struct drops `skip_serializing` legacy
    /// fields, so silently proceeding would destroy the user's block.
    #[test]
    fn load_and_write_reject_legacy_outputs_block() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("himitsu.yaml");
        std::fs::write(
            &path,
            "outputs:\n  pci-prod:\n    selectors:\n      - tag:pci\n",
        )
        .unwrap();
        let resolved = ResolvedScope {
            scope: Scope::Project,
            config_path: path,
        };

        let load_err = load_outputs(&resolved).unwrap_err();
        assert!(
            load_err.to_string().contains("renamed to 'codegen:'"),
            "load must reject with rename guidance: {load_err}"
        );

        let write_err = write_outputs(&resolved, &OutputsMap::new()).unwrap_err();
        assert!(
            write_err.to_string().contains("renamed to 'codegen:'"),
            "write must reject instead of destroying the legacy block: {write_err}"
        );
        // The original file is untouched.
        let contents = std::fs::read_to_string(&resolved.config_path).unwrap();
        assert!(contents.contains("outputs:"));
    }
}
