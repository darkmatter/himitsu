use std::path::PathBuf;

use crate::error::{HimitsuError, Result};

/// Normalise a raw secret path into a canonical store-relative identifier.
///
/// Secret paths are **store-internal names**, not filesystem paths. A leading
/// `/` is therefore just a user convenience (e.g. `/dev/hello` → `dev/hello`)
/// and is silently stripped.  Components that have no meaning in a store
/// namespace (`.`, `..`, empty double-slash segments) are rejected with a
/// clear error.
fn normalize_path(raw: &str) -> Result<String> {
    let stripped = raw.trim_start_matches('/');
    if stripped.is_empty() {
        return Err(HimitsuError::InvalidReference(
            "secret path cannot be empty".into(),
        ));
    }
    for component in stripped.split('/') {
        match component {
            ".." => {
                return Err(HimitsuError::InvalidReference(format!(
                    "'..' is not a valid secret path component in {raw:?}"
                )))
            }
            "." => {
                return Err(HimitsuError::InvalidReference(format!(
                    "'.' is not a valid secret path component in {raw:?}"
                )))
            }
            "" => {
                return Err(HimitsuError::InvalidReference(format!(
                    "empty path component (consecutive slashes) in {raw:?}"
                )))
            }
            _ => {}
        }
    }
    Ok(stripped.to_string())
}

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
    /// Two qualified forms are accepted:
    /// - **Canonical:** `<provider>:<org>/<repo>#<path>` — the `#` makes the
    ///   slug/path boundary unambiguous.
    /// - **Legacy:** `<provider>:<org>/<repo>/<path>` — first two slash
    ///   segments after the provider are the slug, the rest is the path.
    ///
    /// New code should emit the `#` form. Both parse to the same value.
    ///
    /// # Errors
    ///
    /// Returns [`HimitsuError::InvalidReference`] if the input has a provider
    /// prefix (colon) but does not include a valid `org/repo` slug after it,
    /// or if the path segment is empty.
    pub fn parse(input: &str) -> Result<Self> {
        if let Some(colon_pos) = input.find(':') {
            let provider = &input[..colon_pos];
            let rest = &input[colon_pos + 1..];

            if provider.is_empty() {
                return Err(HimitsuError::InvalidReference(format!(
                    "empty provider in reference: {input:?}"
                )));
            }

            // Canonical form: `org/repo#path`. The `#` unambiguously splits
            // the slug from the path.
            let (slug, path_part) = if let Some(hash_pos) = rest.find('#') {
                (&rest[..hash_pos], Some(&rest[hash_pos + 1..]))
            } else {
                // Legacy form: first two slash segments are the slug, the
                // remainder (if any) is the path.
                let parts: Vec<&str> = rest.splitn(3, '/').collect();
                if parts.len() < 2 {
                    return Err(HimitsuError::InvalidReference(format!(
                        "qualified reference must include org/repo after provider \
                         (got {rest:?}): {input:?}"
                    )));
                }
                let slug_end = parts[0].len() + 1 + parts[1].len();
                let slug = &rest[..slug_end];
                let path = if parts.len() == 3 {
                    Some(parts[2])
                } else {
                    None
                };
                (slug, path)
            };

            let slug_parts: Vec<&str> = slug.splitn(2, '/').collect();
            if slug_parts.len() != 2 {
                return Err(HimitsuError::InvalidReference(format!(
                    "qualified reference must include org/repo after provider \
                     (got {slug:?}): {input:?}"
                )));
            }
            let (org, repo) = (slug_parts[0], slug_parts[1]);
            if org.is_empty() || repo.is_empty() {
                return Err(HimitsuError::InvalidReference(format!(
                    "org or repo segment is empty in reference: {input:?}"
                )));
            }

            let path = match path_part {
                Some("") => {
                    return Err(HimitsuError::InvalidReference(format!(
                        "empty secret path after org/repo in reference: {input:?}"
                    )));
                }
                Some(p) => Some(normalize_path(p)?),
                None => None,
            };

            Ok(SecretRef {
                provider: Some(provider.to_string()),
                store_slug: Some(format!("{org}/{repo}")),
                path,
            })
        } else {
            // Bare path — no provider prefix
            let path = normalize_path(input)?;
            Ok(SecretRef {
                provider: None,
                store_slug: None,
                path: Some(path),
            })
        }
    }

    /// Returns `true` if this reference includes a provider and store slug.
    pub fn is_qualified(&self) -> bool {
        self.store_slug.is_some()
    }

    /// Parse a store reference, accepting either a bare slug (`org/repo`) or a
    /// provider-qualified reference (`github:org/repo`).
    ///
    /// The result always has `store_slug` set.  Returns an error if the input
    /// does not contain a valid `org/repo` slug.
    pub fn parse_store_ref(input: &str) -> Result<Self> {
        if input.contains(':') {
            // Qualified — parse normally; store_slug must be present.
            let r = Self::parse(input)?;
            if r.store_slug.is_none() {
                return Err(HimitsuError::InvalidReference(format!(
                    "expected a store reference (org/repo or provider:org/repo), got {input:?}"
                )));
            }
            Ok(r)
        } else {
            // Treat as a bare slug: must be exactly org/repo.
            crate::config::validate_remote_slug(input)?;
            Ok(SecretRef {
                provider: None,
                store_slug: Some(input.to_string()),
                path: None,
            })
        }
    }

    /// Resolve the local store checkout path for a qualified reference.
    ///
    /// If the slug is already cloned locally, returns its path. Otherwise:
    /// - With `HIMITSU_YES=1` set, clones non-interactively.
    /// - With an interactive stderr (TTY), prompts `y/N` and clones on `y`.
    /// - Otherwise (non-interactive, no `HIMITSU_YES`), returns an error
    ///   with a hint to run `himitsu remote add` or set `HIMITSU_YES`.
    ///
    /// Returns an error for unqualified (bare) references.
    pub fn resolve_store(&self) -> Result<PathBuf> {
        let slug = self.store_slug.as_deref().ok_or_else(|| {
            HimitsuError::InvalidReference(
                "cannot resolve store for an unqualified (bare) reference".into(),
            )
        })?;
        match crate::config::remote_store_path(slug) {
            Ok(p) => Ok(p),
            Err(HimitsuError::RemoteNotFound(_)) => {
                if confirm_clone(slug) {
                    crate::config::ensure_store(slug)
                } else {
                    Err(HimitsuError::RemoteNotFound(format!(
                        "{slug} (cross-repo reference points to a store not cloned locally; \
                         run `himitsu remote add {slug}` or set HIMITSU_YES=1 to auto-clone)"
                    )))
                }
            }
            Err(e) => Err(e),
        }
    }
}

/// Decide whether to clone a missing referenced store, given the user's
/// environment. Returns `true` when `HIMITSU_YES=1`, or when stderr is a
/// TTY and the user answered `y`/`yes` at the prompt.
fn confirm_clone(slug: &str) -> bool {
    use std::io::{IsTerminal, Write};
    if matches!(
        std::env::var("HIMITSU_YES").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    ) {
        return true;
    }
    let stderr = std::io::stderr();
    if !stderr.is_terminal() {
        return false;
    }
    eprint!("Store '{slug}' is not cloned locally. Clone now? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
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
    fn parse_qualified_hash_form() {
        let r = SecretRef::parse("github:acme/secrets#prod/DB_PASS").unwrap();
        assert_eq!(r.provider, Some("github".into()));
        assert_eq!(r.store_slug, Some("acme/secrets".into()));
        assert_eq!(r.path, Some("prod/DB_PASS".into()));
        assert!(r.is_qualified());
    }

    #[test]
    fn parse_qualified_hash_form_deeply_nested() {
        let r = SecretRef::parse("ssh:org/repo#a/b/c/KEY").unwrap();
        assert_eq!(r.store_slug, Some("org/repo".into()));
        assert_eq!(r.path, Some("a/b/c/KEY".into()));
    }

    #[test]
    fn parse_hash_and_slash_forms_equivalent() {
        let hash = SecretRef::parse("github:acme/secrets#prod/DB_PASS").unwrap();
        let slash = SecretRef::parse("github:acme/secrets/prod/DB_PASS").unwrap();
        assert_eq!(hash, slash);
    }

    #[test]
    fn parse_qualified_hash_empty_path_errors() {
        let result = SecretRef::parse("github:acme/secrets#");
        assert!(matches!(result, Err(HimitsuError::InvalidReference(_))));
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

    // ── Path normalisation ───────────────────────────────────────────────────

    #[test]
    fn parse_bare_leading_slash_is_stripped() {
        // /dev/hello is a perfectly valid secret path — leading / is notation
        let r = SecretRef::parse("/dev/hello").unwrap();
        assert_eq!(r.path, Some("dev/hello".into()));
    }

    #[test]
    fn parse_bare_multiple_leading_slashes_stripped() {
        let r = SecretRef::parse("///prod/KEY").unwrap();
        assert_eq!(r.path, Some("prod/KEY".into()));
    }

    #[test]
    fn parse_qualified_leading_slash_in_path_stripped() {
        let r = SecretRef::parse("github:org/repo//dev/KEY").unwrap();
        assert_eq!(r.path, Some("dev/KEY".into()));
    }

    #[test]
    fn parse_bare_traversal_errors() {
        let result = SecretRef::parse("../../etc/passwd");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not a valid secret path component"),
            "message was: {msg}"
        );
    }

    #[test]
    fn parse_bare_dot_component_errors() {
        let result = SecretRef::parse("prod/./API_KEY");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not a valid secret path component"),
            "message was: {msg}"
        );
    }

    #[test]
    fn parse_qualified_traversal_errors() {
        let result = SecretRef::parse("github:org/repo/../../etc/passwd");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not a valid secret path component"),
            "message was: {msg}"
        );
    }

    #[test]
    fn parse_double_slash_mid_path_errors() {
        let result = SecretRef::parse("prod//KEY");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("consecutive slashes"), "message was: {msg}");
    }

    // ── parse_store_ref ──────────────────────────────────────────────────────

    #[test]
    fn parse_store_ref_bare_slug() {
        let r = SecretRef::parse_store_ref("org/repo").unwrap();
        assert_eq!(r.store_slug, Some("org/repo".into()));
        assert_eq!(r.provider, None);
        assert_eq!(r.path, None);
    }

    #[test]
    fn parse_store_ref_qualified() {
        let r = SecretRef::parse_store_ref("github:org/repo").unwrap();
        assert_eq!(r.store_slug, Some("org/repo".into()));
        assert_eq!(r.provider, Some("github".into()));
        assert_eq!(r.path, None);
    }

    #[test]
    fn parse_store_ref_qualified_with_path_keeps_slug() {
        // Path portion is ignored for store-ref context
        let r = SecretRef::parse_store_ref("github:org/repo/prod/KEY").unwrap();
        assert_eq!(r.store_slug, Some("org/repo".into()));
    }

    #[test]
    fn parse_store_ref_bare_path_errors() {
        // A bare key name is not a valid store ref
        let result = SecretRef::parse_store_ref("notaslug");
        assert!(result.is_err());
    }

    #[test]
    fn parse_store_ref_invalid_slug_errors() {
        let result = SecretRef::parse_store_ref("a/b/c");
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
