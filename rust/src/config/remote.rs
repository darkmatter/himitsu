use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Remote config at `~/.himitsu/data/<org>/<repo>/himitsu.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteConfig {
    /// Recipient policies.
    #[serde(default)]
    pub policies: Vec<Policy>,

    /// External identity sources.
    #[serde(default)]
    pub identity_sources: Vec<IdentitySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub path_prefix: String,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentitySource {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub key_public: Option<String>,
    #[serde(default)]
    pub key_inbox: Option<String>,
}

impl RemoteConfig {
    /// Load remote config from a YAML file.
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: RemoteConfig = serde_yaml::from_str(&contents)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_loads_policies_and_sources() {
        let yaml = r#"
policies:
  - path_prefix: "vars/common/"
    include: ["group:all"]
  - path_prefix: "vars/prod/"
    include: ["group:admins", "remote:github:coopmoney/keys#team=security"]
    exclude: ["group:contractors"]
identity_sources:
  - id: coopmoney_keys
    kind: github_keys_repo
    repo: coopmoney/keys
    ref: main
  - id: coopmoney_domain
    kind: well_known
    domain: coopmoney.com
    path: /.well-known/himitsu.json
"#;
        let config: RemoteConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.policies.len(), 2);
        assert_eq!(config.policies[0].path_prefix, "vars/common/");
        assert_eq!(config.policies[1].exclude, vec!["group:contractors"]);
        assert_eq!(config.identity_sources.len(), 2);
        assert_eq!(config.identity_sources[0].kind, "github_keys_repo");
    }

    #[test]
    fn parse_empty_remote_config() {
        let yaml = "{}\n";
        let config: RemoteConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.policies.is_empty());
        assert!(config.identity_sources.is_empty());
    }
}
