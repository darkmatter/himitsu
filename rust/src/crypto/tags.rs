//! Tag grammar and validator.
//!
//! Tags are free-form labels carried inside `SecretValue.tags`. They are
//! authored from many surfaces (CLI `--tag`, `himitsu tag` subcommand, TUI
//! new-secret form, env DSL `tag:foo`) and every authoring path runs the
//! same validator so the on-disk shape stays consistent.

/// Maximum length of a single tag.
pub const MAX_TAG_LEN: usize = 64;

/// Validate a single tag against the grammar `[A-Za-z0-9_.-]+`, 1-64 chars,
/// case-sensitive, no whitespace.
///
/// Returns the input borrow on success so callers can chain inside a
/// `map`/`collect` without allocating, e.g.
///
/// ```ignore
/// let cleaned: Result<Vec<&str>, _> = raw.iter().map(|t| validate_tag(t)).collect();
/// ```
pub fn validate_tag(s: &str) -> Result<&str, String> {
    if s.is_empty() {
        return Err("tag must not be empty".to_string());
    }
    if s.len() > MAX_TAG_LEN {
        return Err(format!(
            "tag {s:?} is {} chars; max is {MAX_TAG_LEN}",
            s.len()
        ));
    }
    for ch in s.chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-';
        if !ok {
            return Err(format!(
                "tag {s:?} contains invalid character {ch:?} \
                 (allowed: A-Z a-z 0-9 _ . -)"
            ));
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_tags() {
        assert_eq!(validate_tag("pci"), Ok("pci"));
        assert_eq!(validate_tag("Stripe"), Ok("Stripe"));
        assert_eq!(validate_tag("rotate-2026-q1"), Ok("rotate-2026-q1"));
        assert_eq!(validate_tag("team_backend"), Ok("team_backend"));
        assert_eq!(validate_tag("v1.2.3"), Ok("v1.2.3"));
        assert_eq!(validate_tag("a"), Ok("a"));
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_tag("").is_err());
    }

    #[test]
    fn rejects_whitespace() {
        assert!(validate_tag("foo bar").is_err());
        assert!(validate_tag("foo\tbar").is_err());
        assert!(validate_tag(" foo").is_err());
        assert!(validate_tag("foo ").is_err());
    }

    #[test]
    fn rejects_special_characters() {
        assert!(validate_tag("foo:bar").is_err());
        assert!(validate_tag("foo/bar").is_err());
        assert!(validate_tag("foo,bar").is_err());
        assert!(validate_tag("foo!").is_err());
    }

    #[test]
    fn rejects_too_long() {
        let s = "a".repeat(MAX_TAG_LEN + 1);
        assert!(validate_tag(&s).is_err());
        let s = "a".repeat(MAX_TAG_LEN);
        assert_eq!(validate_tag(&s), Ok(s.as_str()));
    }

    #[test]
    fn case_sensitive() {
        // "PCI" and "pci" are distinct tags — both valid, but not equal.
        assert_eq!(validate_tag("PCI"), Ok("PCI"));
        assert_eq!(validate_tag("pci"), Ok("pci"));
    }
}
