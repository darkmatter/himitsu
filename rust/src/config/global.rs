use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Global user config at `~/.himitsu/config.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobalConfig {
    /// Default remote (org/repo) when no project binding or -r flag.
    #[serde(default)]
    pub default_remote: Option<String>,

    /// Nostr relay configuration.
    #[serde(default)]
    pub nostr: Option<NostrConfig>,

    /// Sharing defaults.
    #[serde(default)]
    pub sharing: Option<SharingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NostrConfig {
    #[serde(default)]
    pub relays: Vec<String>,
    #[serde(default)]
    pub event_kind: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharingConfig {
    #[serde(default)]
    pub default_transport: Option<String>,
}

impl GlobalConfig {
    /// Load global config from a YAML file.
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: GlobalConfig = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    /// Write a default global config to the given path.
    pub fn write_default(path: &Path) -> Result<()> {
        let config = GlobalConfig::default();
        let yaml = serde_yaml::to_string(&config)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_config() {
        let yaml = r#"
default_remote: myorg/secrets
nostr:
  relays:
    - wss://relay.damus.io
  event_kind: 30420
sharing:
  default_transport: github_pr
"#;
        let config: GlobalConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.default_remote.unwrap(), "myorg/secrets");
        let nostr = config.nostr.unwrap();
        assert_eq!(nostr.relays.len(), 1);
        assert_eq!(nostr.event_kind.unwrap(), 30420);
    }

    #[test]
    fn parse_minimal_config() {
        let yaml = "default_remote: me/passwords\n";
        let config: GlobalConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.default_remote.unwrap(), "me/passwords");
        assert!(config.nostr.is_none());
    }

    #[test]
    fn parse_empty_config() {
        let yaml = "{}\n";
        let config: GlobalConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.default_remote.is_none());
    }

    #[test]
    fn reject_malformed_yaml() {
        let yaml = "{{invalid yaml";
        let result: std::result::Result<GlobalConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }
}
