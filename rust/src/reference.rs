use std::path::PathBuf;

use crate::error::{HimitsuError, Result};

/// A parsed reference to a store or secret.
///
/// # Supported formats
///
/// | Format | Example | Result |
/// |--------|---------|--------|
/// | Qualified store ref | `github:acme/secrets` | provider + store_slug, no path |
/// | Qualified secret ref | `github:acme/secrets/prod/API_KEY` | provider + store_slug + path |
/// | Bare path | `prod/API_KEY` | path only |
/// | Bare key | `API_KEY` | path only |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    /// Provider prefix, e.g. `"github"`, `"gitlab"`, `"ssh"`.
    /// Present only when the reference includes a colon-prefixed provider.
    pub provider: Option<String>,
    /// Store slug `"org/repo"` — only set when a provider prefix is present.
    pub store_slug: Option<String>,
    /// Secret path within the store, e.g. `"prod/API_KEY"`.
    /// May be `None` for store-only references (`provider:org/repo`).
    pub path: Option<String>,
}

impl SecretRef {
    /// Parse a secret reference from a string.
    ///
    /// # Errors
    ///
    /// Returns [`HimitsuError::InvalidReference`] if the input has a provider
    /// prefix (colon) but does not include a valid `org/repo` slug after it,
    /// or if the path segment after `org/repo` is empty.
    pub fn parse(input: &str) -> Result<Self> {
        if let Some(colon_pos) = input.find(':') {
            let provider = &input[..colon_pos];
            let rest = &input[colon_pos + 1..];

            if provider.is_empty() {
                return Err(HimitsuError::InvalidReference(format!(
                    "empty provider in reference: {input:?}"
                )));
            }

            // rest must be at minimum "org/repo" (two non-empty segments)
            let parts: Vec<&str> = rest.splitn(3, '/').collect();
            if parts.len() < 2 {
                return Err(HimitsuError::InvalidReference(format!(
                    "qualified reference must include org/repo after provider \
                     (got {rest:?}): {input:?}"
                )));
            }

            let (org, repo) = (parts[0], parts[1]);
            if org.is_empty() || repo.is_empty() {
                return Err(HimitsuError::InvalidReference(format!(
                    "org or repo segment is empty in reference: {input:?}"
                )));
            }

            let path = if parts.len() == 3 {
                let p = parts[2];
                if p.is_empty() {
                    return Err(HimitsuError::InvalidReference(format!(
                        "empty secret path after org/repo in reference: {input:?}"
                    )));
                }
                Some(p.to_string())
            } else {
                None
            };

            Ok(SecretRef {
                provider: Some(provider.to_string()),
                store_slug: Some(format!("{org}/{repo}")),
                path,
            })
        } else {
            // Bare path — no provider prefix
            Ok(SecretRef {
                provider: None,
                store_slug: None,
                path: Some(input.to_string()),
            })
        }
    }

    /// Returns `true` if this reference includes a provider and store slug.
    pub fn is_qualified(&self) -> bool {
        self.store_slug.is_some()
    }

    /// Resolve the local store checkout path for a qualified reference.
    ///
    /// Calls [`crate::config::remote_store_path`] to look up the local
    /// checkout of the named `org/repo` store. Returns an error when:
    /// - this is an unqualified (bare) reference, or
    /// - the slug is not found in the local store directory.
    pub fn resolve_store(&self) -> Result<PathBuf> {
        let slug = self.store_slug.as_deref().ok_or_else(|| {
            HimitsuError::InvalidReference(
                "cannot resolve store for an unqualified (bare) reference".into(),
            )
        })?;
        crate::config::remote_store_path(slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Parsing tests ────────────────────────────────────────────────────────

    #[test]
    fn parse_qualified_store_only() {
        let r = SecretRef::parse("github:acme/secrets").unwrap();
        assert_eq!(r.provider, Some("github".into()));
        assert_eq!(r.store_slug, Some("acme/secrets".into()));
        assert_eq!(r.path, None);
        assert!(r.is_qualified());
    }

    #[test]
    fn parse_qualified_with_simple_path() {
        let r = SecretRef::parse("github:acme/secrets/prod/DB_PASS").unwrap();
        assert_eq!(r.provider, Some("github".into()));
        assert_eq!(r.store_slug, Some("acme/secrets".into()));
        assert_eq!(r.path, Some("prod/DB_PASS".into()));
        assert!(r.is_qualified());
    }

    #[test]
    fn parse_qualified_gitlab() {
        let r = SecretRef::parse("gitlab:team/vault/staging/API_KEY").unwrap();
        assert_eq!(r.provider, Some("gitlab".into()));
        assert_eq!(r.store_slug, Some("team/vault".into()));
        assert_eq!(r.path, Some("staging/API_KEY".into()));
        assert!(r.is_qualified());
    }

    #[test]
    fn parse_qualified_deeply_nested_path() {
        let r = SecretRef::parse("ssh:org/repo/a/b/c/KEY").unwrap();
        assert_eq!(r.provider, Some("ssh".into()));
        assert_eq!(r.store_slug, Some("org/repo".into()));
        assert_eq!(r.path, Some("a/b/c/KEY".into()));
    }

    #[test]
    fn parse_bare_path() {
        let r = SecretRef::parse("prod/DB_PASS").unwrap();
        assert_eq!(r.provider, None);
        assert_eq!(r.store_slug, None);
        assert_eq!(r.path, Some("prod/DB_PASS".into()));
        assert!(!r.is_qualified());
    }

    #[test]
    fn parse_bare_key() {
        let r = SecretRef::parse("DB_PASS").unwrap();
        assert_eq!(r.provider, None);
        assert_eq!(r.store_slug, None);
        assert_eq!(r.path, Some("DB_PASS".into()));
        assert!(!r.is_qualified());
    }

    // ── Error cases ──────────────────────────────────────────────────────────

    #[test]
    fn parse_qualified_missing_repo_errors() {
        let result = SecretRef::parse("github:invalid");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("org/repo"), "message was: {msg}");
    }

    #[test]
    fn parse_empty_provider_errors() {
        let result = SecretRef::parse(":org/repo");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("empty provider"), "message was: {msg}");
    }

    #[test]
    fn parse_empty_path_after_store_errors() {
        let result = SecretRef::parse("github:org/repo/");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("empty secret path"), "message was: {msg}");
    }

    #[test]
    fn parse_empty_org_errors() {
        let result = SecretRef::parse("github:/repo");
        assert!(result.is_err());
    }

    // ── resolve_store ────────────────────────────────────────────────────────

    #[test]
    fn resolve_store_on_bare_ref_errors() {
        let r = SecretRef::parse("prod/KEY").unwrap();
        assert!(!r.is_qualified());
        assert!(r.resolve_store().is_err());
    }
}
