use std::path::PathBuf;

/// Top-level error type for himitsu.
#[derive(Debug, thiserror::Error)]
pub enum HimitsuError {
    #[error("config file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("secret not found: {0}")]
    SecretNotFound(String),

    #[error("remote not found: {0}")]
    RemoteNotFound(String),

    #[error("remote error: {0}")]
    Remote(String),

    #[error("store not found: {0}")]
    StoreNotFound(String),

    #[error(
        "ambiguous store — multiple stores found: {0:?}\n  \
         Use --remote <org/repo> or run `himitsu remote default <org/repo>` to set a default."
    )]
    AmbiguousStore(Vec<String>),

    #[error("git error: {0}")]
    Git(String),

    #[error("group error: {0}")]
    Group(String),

    #[error("recipient error: {0}")]
    Recipient(String),

    #[error("not initialized: run `himitsu init` first")]
    NotInitialized,

    #[error("not supported: {0}")]
    NotSupported(String),

    #[error("project config required: {0}")]
    ProjectConfigRequired(String),

    #[error("generate error: {0}")]
    GenerateError(String),

    #[error("invalid reference: {0}")]
    InvalidReference(String),

    #[error("keychain error: {0}")]
    Keychain(String),

    #[error("index error: {0}")]
    Index(String),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, HimitsuError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages() {
        let err = HimitsuError::SecretNotFound("prod/API_KEY".into());
        assert_eq!(err.to_string(), "secret not found: prod/API_KEY");

        let err = HimitsuError::ConfigNotFound(PathBuf::from("/tmp/missing.yaml"));
        assert_eq!(err.to_string(), "config file not found: /tmp/missing.yaml");
    }

    #[test]
    fn io_error_converts() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: HimitsuError = io_err.into();
        assert!(matches!(err, HimitsuError::Io(_)));
    }

    #[test]
    fn yaml_error_converts() {
        let yaml_err: std::result::Result<String, _> = serde_yaml::from_str("{{invalid");
        let err: HimitsuError = yaml_err.unwrap_err().into();
        assert!(matches!(err, HimitsuError::Yaml(_)));
    }
}
