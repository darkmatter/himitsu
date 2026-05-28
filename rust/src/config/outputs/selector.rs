//! Tag-selector grammar for `himitsu exec`, `himitsu generate`, and the `outputs:` block.
//!
//! # Grammar (EBNF)
//!
//! ```text
//! ref      = group ("," group)*
//! group    = token ("+" token)*
//! token    = glob | tag | path
//! glob     = path-with-wildcards  (contains '*' or '?')
//! tag      = "tag:" IDENT
//! path     = literal-path          (no wildcards, no "tag:" prefix)
//! IDENT    = [A-Za-z0-9_.-]+       (1–64 chars, case-sensitive)
//! ```
//!
//! # Semantics
//!
//! - `+` within a group = AND (all tokens must match the secret)
//! - `,` between groups = OR  (union across groups)
//!
//! # Examples
//!
//! ```text
//! tag:pci                    — all secrets tagged "pci"
//! tag:pci+tag:prod           — secrets tagged BOTH pci AND prod
//! prod/*+tag:pci             — secrets under prod/* AND tagged pci
//! tag:pci,tag:dev            — secrets tagged pci OR tagged dev
//! prod/api-key               — single concrete path
//! ```

use crate::error::HimitsuError;

/// A single selector token: glob pattern, tag filter, or concrete path.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// A glob pattern (contains `*` or `?`). Matched against secret paths.
    Glob(String),
    /// A `tag:NAME` selector. Matched against a secret's tag list.
    Tag(String),
    /// A concrete secret path (no wildcards, no `tag:` prefix).
    Path(String),
}

/// A group of AND-combined tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Group(pub Vec<Token>);

/// A parsed selector: one or more OR-combined groups.
#[derive(Debug, Clone, PartialEq)]
pub struct Selector(pub Vec<Group>);

/// Input to `Selector::matches`.
pub struct SecretMatch<'a> {
    pub path: &'a str,
    pub tags: &'a [String],
}

impl Selector {
    /// Parse a selector string according to the grammar above.
    ///
    /// Returns `Err(HimitsuError::InvalidSelector)` for any parse error.
    pub fn parse(_s: &str) -> Result<Self, HimitsuError> {
        // TODO(T8): implement full parser
        Err(HimitsuError::InvalidSelector(
            "not yet implemented".to_string(),
        ))
    }

    /// Returns true if this selector matches the given secret.
    pub fn matches(&self, _secret: &SecretMatch<'_>) -> bool {
        // TODO(T8): implement matching
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stub_returns_err() {
        // Stub: parser returns error for everything until T8 implements it.
        assert!(Selector::parse("tag:foo").is_err());
    }
}
