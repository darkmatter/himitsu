//! Config mutation helpers for the `outputs:` block.
//!
//! YAML fidelity matches `envs_mut`: mutations use a lossy serde_yaml
//! round-trip, so comments and custom formatting are not preserved. Map order
//! is deterministic because `OutputsMap` is a `BTreeMap`. Adding the exact same
//! output definition twice is idempotent; adding the same name with different
//! content replaces the previous definition.

use std::path::Path;

use crate::config::envs_mut::{resolve_scope, ResolvedScope, ScopeHint};
use crate::config::outputs::{OutputDef, OutputsMap};
use crate::config::{Config, ProjectConfig};
use crate::error::{HimitsuError, Result};

pub use crate::config::envs_mut::ScopeHint as OutputScopeHint;

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
    match resolved.scope {
        crate::config::env_cache::Scope::Project => {
            let cfg: ProjectConfig = serde_yaml::from_str(&contents)?;
            Ok(cfg.outputs)
        }
        crate::config::env_cache::Scope::Global => {
            let cfg: Config = serde_yaml::from_str(&contents)?;
            Ok(cfg.outputs)
        }
    }
}

fn write_outputs(resolved: &ResolvedScope, new_outputs: &OutputsMap) -> Result<()> {
    if let Some(parent) = resolved.config_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let serialized = match resolved.scope {
        crate::config::env_cache::Scope::Project => {
            let mut cfg: ProjectConfig = if resolved.config_path.exists() {
                let contents = std::fs::read_to_string(&resolved.config_path)?;
                serde_yaml::from_str(&contents)?
            } else {
                ProjectConfig::default()
            };
            cfg.outputs = new_outputs.clone();
            cfg.validate()?;
            serde_yaml::to_string(&cfg)?
        }
        crate::config::env_cache::Scope::Global => {
            let mut cfg: Config = if resolved.config_path.exists() {
                let contents = std::fs::read_to_string(&resolved.config_path)?;
                serde_yaml::from_str(&contents)?
            } else {
                Config::default()
            };
            cfg.outputs = new_outputs.clone();
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
}
