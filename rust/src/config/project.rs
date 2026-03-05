use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Project binding config at `<repo>/.himitsu.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectConfig {
    /// Remote reference (org/repo).
    pub remote: String,

    /// Codegen configuration.
    #[serde(default)]
    pub codegen: Option<CodegenConfig>,

    /// Enable automatic sync after mutations.
    #[serde(default)]
    pub autosync: bool,

    /// When to trigger autosync: "set", "commit", or "push".
    #[serde(default)]
    pub autosync_on: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodegenConfig {
    pub lang: String,
    pub path: String,
}

impl ProjectConfig {
    /// Load project config from a YAML file.
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: ProjectConfig = serde_yaml::from_str(&contents)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_remote_field() {
        let yaml = r#"
remote: myorg/secrets
codegen:
  lang: typescript
  path: src/generated/config.ts
"#;
        let config: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.remote, "myorg/secrets");
        let cg = config.codegen.unwrap();
        assert_eq!(cg.lang, "typescript");
        assert_eq!(cg.path, "src/generated/config.ts");
    }

    #[test]
    fn parse_minimal() {
        let yaml = "remote: me/passwords\n";
        let config: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.remote, "me/passwords");
        assert!(!config.autosync);
    }

    #[test]
    fn parse_with_autosync() {
        let yaml = r#"
remote: myorg/secrets
autosync: true
autosync_on: push
"#;
        let config: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.autosync);
        assert_eq!(config.autosync_on.unwrap(), "push");
    }
}
