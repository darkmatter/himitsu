use std::path::PathBuf;

/// Top-level error type for himitsu.
#[allow(dead_code)]
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

    #[error("secret not found: {key} in environment {env}")]
    SecretNotFound { env: String, key: String },

    #[error("remote error: {0}")]
    Remote(String),

    #[error("git error: {0}")]
    Git(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias used throughout the crate.
#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, HimitsuError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages() {
        let err = HimitsuError::SecretNotFound {
            env: "prod".into(),
            key: "API_KEY".into(),
        };
        assert_eq!(
            err.to_string(),
            "secret not found: API_KEY in environment prod"
        );

        let err = HimitsuError::ConfigNotFound(PathBuf::from("/tmp/missing.yaml"));
        assert_eq!(err.to_string(), "config file not found: /tmp/missing.yaml");
    }

    #[test]
    fn io_error_converts() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err: HimitsuError = io_err.into();
        assert!(matches!(err, HimitsuError::Io(_)));
    }
}
