//! Tag-selector grammar for `himitsu exec`, `himitsu generate`, and the `codegen:` block.
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
use crate::{cli::export::glob_match, crypto::tags::validate_tag};

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
    pub fn parse(s: &str) -> Result<Self, HimitsuError> {
        if s.is_empty() {
            return Err(HimitsuError::InvalidSelector("empty selector".to_string()));
        }

        let groups = s
            .split(',')
            .map(parse_group)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self(groups))
    }

    /// Returns true if this selector matches the given secret.
    pub fn matches(&self, secret: &SecretMatch<'_>) -> bool {
        self.0.iter().any(|group| group.matches(secret))
    }
}

impl Group {
    fn matches(&self, secret: &SecretMatch<'_>) -> bool {
        self.0.iter().all(|token| token.matches(secret))
    }
}

impl Token {
    fn matches(&self, secret: &SecretMatch<'_>) -> bool {
        match self {
            Self::Glob(pattern) => glob_match(pattern, secret.path),
            Self::Tag(name) => secret.tags.iter().any(|tag| tag == name),
            Self::Path(literal) => secret.path == literal,
        }
    }
}

fn parse_group(s: &str) -> Result<Group, HimitsuError> {
    if s.is_empty() {
        return Err(HimitsuError::InvalidSelector(
            "empty selector group".to_string(),
        ));
    }

    let tokens = s
        .split('+')
        .map(parse_token)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Group(tokens))
}

fn parse_token(s: &str) -> Result<Token, HimitsuError> {
    if s.is_empty() {
        return Err(HimitsuError::InvalidSelector(
            "empty selector token".to_string(),
        ));
    }

    if let Some(name) = s.strip_prefix("tag:") {
        if name.is_empty() {
            return Err(HimitsuError::InvalidSelector(
                "tag name is empty".to_string(),
            ));
        }

        validate_tag(name).map_err(HimitsuError::InvalidSelector)?;
        return Ok(Token::Tag(name.to_string()));
    }

    if s.contains('*') || s.contains('?') {
        return Ok(Token::Glob(s.to_string()));
    }

    Ok(Token::Path(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_tag() {
        assert_eq!(
            Selector::parse("tag:pci").unwrap(),
            Selector(vec![Group(vec![Token::Tag("pci".to_string())])])
        );
    }

    #[test]
    fn parse_and_combined_tags() {
        assert_eq!(
            Selector::parse("tag:pci+tag:prod").unwrap(),
            Selector(vec![Group(vec![
                Token::Tag("pci".to_string()),
                Token::Tag("prod".to_string()),
            ])])
        );
    }

    #[test]
    fn parse_glob_and_tag() {
        assert_eq!(
            Selector::parse("prod/*+tag:pci").unwrap(),
            Selector(vec![Group(vec![
                Token::Glob("prod/*".to_string()),
                Token::Tag("pci".to_string()),
            ])])
        );
    }

    #[test]
    fn parse_or_combined_tags() {
        assert_eq!(
            Selector::parse("tag:pci,tag:dev").unwrap(),
            Selector(vec![
                Group(vec![Token::Tag("pci".to_string())]),
                Group(vec![Token::Tag("dev".to_string())]),
            ])
        );
    }

    #[test]
    fn parse_literal_path() {
        assert_eq!(
            Selector::parse("prod/api-key").unwrap(),
            Selector(vec![Group(vec![Token::Path("prod/api-key".to_string())])])
        );
    }

    #[test]
    fn parse_question_mark_glob() {
        assert_eq!(
            Selector::parse("prod/key?").unwrap(),
            Selector(vec![Group(vec![Token::Glob("prod/key?".to_string())])])
        );
    }

    #[test]
    fn parse_rejects_empty_selector() {
        assert_invalid_selector(Selector::parse(""));
    }

    #[test]
    fn parse_rejects_empty_tag_name() {
        assert_invalid_selector(Selector::parse("tag:"));
    }

    #[test]
    fn parse_rejects_invalid_tag_characters() {
        assert_invalid_selector(Selector::parse("tag:foo bar"));
    }

    #[test]
    fn parse_rejects_too_long_tag_name() {
        assert_invalid_selector(Selector::parse(&format!("tag:{}", "a".repeat(65))));
    }

    #[test]
    fn parse_rejects_empty_group() {
        assert_invalid_selector(Selector::parse("tag:pci,"));
    }

    #[test]
    fn parse_rejects_empty_token() {
        assert_invalid_selector(Selector::parse("tag:pci+"));
    }

    #[test]
    fn tag_token_matches_secret_tag() {
        let selector = Selector(vec![Group(vec![Token::Tag("pci".to_string())])]);
        let tags = vec!["pci".to_string()];

        assert!(selector.matches(&SecretMatch {
            path: "x",
            tags: &tags,
        }));
    }

    #[test]
    fn and_group_requires_all_tokens() {
        let selector = Selector(vec![Group(vec![
            Token::Tag("pci".to_string()),
            Token::Tag("prod".to_string()),
        ])]);
        let tags = vec!["pci".to_string()];

        assert!(!selector.matches(&SecretMatch {
            path: "x",
            tags: &tags,
        }));
    }

    #[test]
    fn or_groups_match_when_any_group_matches() {
        let selector = Selector(vec![
            Group(vec![Token::Tag("pci".to_string())]),
            Group(vec![Token::Tag("dev".to_string())]),
        ]);
        let tags = vec!["dev".to_string()];

        assert!(selector.matches(&SecretMatch {
            path: "x",
            tags: &tags,
        }));
    }

    #[test]
    fn path_token_matches_exact_path() {
        let selector = Selector(vec![Group(vec![Token::Path("prod/key".to_string())])]);

        assert!(selector.matches(&SecretMatch {
            path: "prod/key",
            tags: &[],
        }));
    }

    #[test]
    fn path_token_rejects_different_path() {
        let selector = Selector(vec![Group(vec![Token::Path("prod/key".to_string())])]);

        assert!(!selector.matches(&SecretMatch {
            path: "dev/key",
            tags: &[],
        }));
    }

    #[test]
    fn glob_token_matches_secret_path() {
        let selector = Selector(vec![Group(vec![Token::Glob("prod/*".to_string())])]);

        assert!(selector.matches(&SecretMatch {
            path: "prod/key",
            tags: &[],
        }));
    }

    #[test]
    fn question_mark_glob_matches_single_character() {
        let selector = Selector(vec![Group(vec![Token::Glob("prod/key?".to_string())])]);

        assert!(selector.matches(&SecretMatch {
            path: "prod/key1",
            tags: &[],
        }));
    }

    fn assert_invalid_selector(result: Result<Selector, HimitsuError>) {
        assert!(matches!(result, Err(HimitsuError::InvalidSelector(_))));
    }
}
