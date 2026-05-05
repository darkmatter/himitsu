//! Extended env DSL: brace expansion + `{}` placeholder + flat KEY=value resolution.
//!
//! This module sits above the legacy [`super::env_resolver`] and adds the
//! features called for in the envs-revamp brief without breaking the existing
//! `EnvEntry` shape on disk:
//!
//! 1. **Parameterized env names**: `my-env-{dev,prod,stg}` expands to three
//!    concrete envs (`my-env-dev`, `my-env-prod`, `my-env-stg`). The brace
//!    list is parsed once; `{}` placeholders inside entry paths are filled
//!    with the matching expansion value.
//! 2. **Default env-key derivation**: when an entry lacks an explicit `KEY:`
//!    override, the env-var key is derived from the item name as
//!    `upper(replace(name, '-', '_'))`, with `/` separators turning into `__`.
//! 3. **Flat resolution**: produces an ordered list of `(KEY, item_path)`
//!    pairs plus a parallel list of warnings (unmatched globs, ambiguous
//!    placeholders, missing keys, …) — what the right-hand TUI preview pane
//!    shows.
//!
//! Backwards compat: if an env's name has no `{...}` segment and no entry
//! contains `{}`, this module behaves exactly like the legacy resolver.

use std::collections::BTreeMap;

use super::EnvEntry;

/// One resolved KEY=item_path pair for the live preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPair {
    /// The expanded env label that produced this pair (e.g. `my-env-dev`).
    pub env_label: String,
    /// The environment variable key (after override / derivation).
    pub key: String,
    /// The store item path the value comes from.
    pub item_path: String,
}

/// Soft warning attached to a resolution result — never fatal, surfaced in
/// the right-hand preview so authors see why something didn't expand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionWarning {
    pub env_label: String,
    pub message: String,
}

/// Output of [`resolve_env_dsl`].
#[derive(Debug, Default, Clone)]
pub struct ResolutionOutput {
    pub pairs: Vec<ResolvedPair>,
    pub warnings: Vec<ResolutionWarning>,
}

/// Expand a brace-parameterized env label into its concrete instances.
///
/// `my-env-{dev,prod,stg}` → `[("my-env-dev", "dev"), ("my-env-prod", "prod"),
/// ("my-env-stg", "stg")]`. A label with no `{...}` segment returns a single
/// element with an empty placeholder.
///
/// Only the **first** brace group is expanded — multi-group expansion is out
/// of scope for v1; nested or repeated `{...}` are treated as literals.
pub fn expand_brace_label(label: &str) -> Vec<(String, String)> {
    let Some(open) = label.find('{') else {
        return vec![(label.to_string(), String::new())];
    };
    let Some(close_rel) = label[open..].find('}') else {
        return vec![(label.to_string(), String::new())];
    };
    let close = open + close_rel;
    let prefix = &label[..open];
    let suffix = &label[close + 1..];
    let body = &label[open + 1..close];
    if body.is_empty() {
        return vec![(label.to_string(), String::new())];
    }
    body.split(',')
        .map(|raw| raw.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|val| (format!("{prefix}{val}{suffix}"), val))
        .collect()
}

/// Substitute `{}` (the empty-brace placeholder) with `value` in `s`.
pub fn substitute_placeholder(s: &str, value: &str) -> String {
    s.replace("{}", value)
}

/// Derive the default env-var key from an item name when no explicit KEY
/// override is provided. Mirrors the brief: `upper(replace(name, '-', '_'))`,
/// with `/` separators becoming `__`.
pub fn derive_env_key(item_name: &str) -> String {
    item_name
        .replace('/', "__")
        .replace('-', "_")
        .to_ascii_uppercase()
}

/// Resolve a single env definition (which may use `{}` placeholders) against
/// the available item names. Returns flat KEY=value pairs and any warnings.
///
/// `expansion_value` is the brace-expansion value bound to `{}` (empty
/// string when the env name has no `{...}` segment).
pub fn resolve_concrete_env(
    label: &str,
    expansion_value: &str,
    entries: &[EnvEntry],
    available_items: &[String],
) -> ResolutionOutput {
    let mut out = ResolutionOutput::default();
    for entry in entries {
        match entry {
            EnvEntry::Single(path) => {
                let resolved = substitute_placeholder(path, expansion_value);
                if available_items.iter().any(|p| p == &resolved) {
                    let key = derive_env_key(last_component(&resolved));
                    out.pairs.push(ResolvedPair {
                        env_label: label.to_string(),
                        key,
                        item_path: resolved,
                    });
                } else if expansion_value.is_empty() && !resolved.contains("{}") {
                    out.pairs.push(ResolvedPair {
                        env_label: label.to_string(),
                        key: derive_env_key(last_component(&resolved)),
                        item_path: resolved.clone(),
                    });
                    out.warnings.push(ResolutionWarning {
                        env_label: label.to_string(),
                        message: format!("item '{resolved}' not found in store"),
                    });
                } else {
                    out.warnings.push(ResolutionWarning {
                        env_label: label.to_string(),
                        message: format!("item '{resolved}' not found in store"),
                    });
                }
            }
            EnvEntry::Alias { key, path } => {
                let resolved = substitute_placeholder(path, expansion_value);
                let final_key = substitute_placeholder(key, expansion_value);
                if available_items.iter().any(|p| p == &resolved) {
                    out.pairs.push(ResolvedPair {
                        env_label: label.to_string(),
                        key: final_key,
                        item_path: resolved,
                    });
                } else {
                    out.pairs.push(ResolvedPair {
                        env_label: label.to_string(),
                        key: final_key,
                        item_path: resolved.clone(),
                    });
                    out.warnings.push(ResolutionWarning {
                        env_label: label.to_string(),
                        message: format!("aliased item '{resolved}' not found in store"),
                    });
                }
            }
            EnvEntry::Glob(prefix) => {
                let resolved_prefix = substitute_placeholder(prefix, expansion_value);
                let needle = format!("{resolved_prefix}/");
                let mut matched = false;
                for item in available_items {
                    if item.starts_with(&needle) {
                        matched = true;
                        let tail = &item[needle.len()..];
                        out.pairs.push(ResolvedPair {
                            env_label: label.to_string(),
                            key: derive_env_key(tail),
                            item_path: item.clone(),
                        });
                    }
                }
                if !matched {
                    out.warnings.push(ResolutionWarning {
                        env_label: label.to_string(),
                        message: format!("glob '{resolved_prefix}/*' matched no items"),
                    });
                }
            }
            // Tag selectors require decrypting candidate secrets — the
            // DSL preview is a read-only path with no age identity, so we
            // surface a warning instead of silently dropping the entry.
            // The codegen pipeline (`resolve_with_tags`) does the real work.
            EnvEntry::Tag(t) => {
                out.warnings.push(ResolutionWarning {
                    env_label: label.to_string(),
                    message: format!(
                        "tag selector 'tag:{t}' is not previewable here — \
                         see `himitsu codegen <env>` for the resolved tree"
                    ),
                });
            }
            EnvEntry::AliasTag { key, tag } => {
                out.warnings.push(ResolutionWarning {
                    env_label: label.to_string(),
                    message: format!(
                        "alias '{key}: tag:{tag}' is not previewable here — \
                         see `himitsu codegen <env>` for the resolved tree"
                    ),
                });
            }
        }
    }
    out
}

/// Resolve every env in `envs`, expanding brace labels and substituting `{}`
/// placeholders. Returns combined pairs + warnings across all expansions.
pub fn resolve_all(
    envs: &BTreeMap<String, Vec<EnvEntry>>,
    available_items: &[String],
) -> ResolutionOutput {
    let mut combined = ResolutionOutput::default();
    for (label, entries) in envs {
        for (concrete_label, value) in expand_brace_label(label) {
            let mut piece = resolve_concrete_env(&concrete_label, &value, entries, available_items);
            combined.pairs.append(&mut piece.pairs);
            combined.warnings.append(&mut piece.warnings);
        }
    }
    combined
}

fn last_component(path: &str) -> &str {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn expand_brace_label_no_brace_returns_single() {
        let v = expand_brace_label("dev");
        assert_eq!(v, vec![("dev".to_string(), "".to_string())]);
    }

    #[test]
    fn expand_brace_label_three_values() {
        let v = expand_brace_label("my-env-{dev,prod,stg}");
        assert_eq!(
            v,
            vec![
                ("my-env-dev".to_string(), "dev".to_string()),
                ("my-env-prod".to_string(), "prod".to_string()),
                ("my-env-stg".to_string(), "stg".to_string()),
            ]
        );
    }

    #[test]
    fn expand_brace_label_trims_whitespace() {
        let v = expand_brace_label("e-{a, b , c}");
        assert_eq!(
            v.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
            vec!["e-a", "e-b", "e-c"]
        );
    }

    #[test]
    fn derive_env_key_handles_dashes_and_slashes() {
        assert_eq!(derive_env_key("api-key"), "API_KEY");
        assert_eq!(derive_env_key("group/item-name"), "GROUP__ITEM_NAME");
        assert_eq!(derive_env_key("DB_PASS"), "DB_PASS");
    }

    #[test]
    fn substitute_placeholder_replaces_braces() {
        assert_eq!(substitute_placeholder("ref-{}", "dev"), "ref-dev");
        assert_eq!(
            substitute_placeholder("noplaceholder", "x"),
            "noplaceholder"
        );
        assert_eq!(substitute_placeholder("a/{}/b", "mid"), "a/mid/b");
    }

    #[test]
    fn resolve_concrete_env_single_match() {
        let entries = vec![EnvEntry::Single("app/api-key".into())];
        let items = s(&["app/api-key", "other"]);
        let out = resolve_concrete_env("e", "", &entries, &items);
        assert_eq!(out.pairs.len(), 1);
        assert_eq!(out.pairs[0].key, "API_KEY");
        assert_eq!(out.pairs[0].item_path, "app/api-key");
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn resolve_concrete_env_alias_overrides_key() {
        let entries = vec![EnvEntry::Alias {
            key: "MYDB".into(),
            path: "db/secret".into(),
        }];
        let items = s(&["db/secret"]);
        let out = resolve_concrete_env("e", "", &entries, &items);
        assert_eq!(out.pairs[0].key, "MYDB");
    }

    #[test]
    fn resolve_concrete_env_glob_no_match_warns() {
        let entries = vec![EnvEntry::Glob("nope".into())];
        let items = s(&["other/x"]);
        let out = resolve_concrete_env("e", "", &entries, &items);
        assert!(out.pairs.is_empty());
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn resolve_concrete_env_glob_collects_pairs() {
        let entries = vec![EnvEntry::Glob("api".into())];
        let items = s(&["api/key-one", "api/key-two", "other"]);
        let out = resolve_concrete_env("e", "", &entries, &items);
        let keys: Vec<_> = out.pairs.iter().map(|p| p.key.clone()).collect();
        assert!(keys.contains(&"KEY_ONE".to_string()));
        assert!(keys.contains(&"KEY_TWO".to_string()));
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn resolve_all_with_brace_expansion() {
        let mut envs = BTreeMap::new();
        envs.insert(
            "env-{dev,prod}".to_string(),
            vec![EnvEntry::Alias {
                key: "DB".into(),
                path: "{}/db".into(),
            }],
        );
        let items = s(&["dev/db", "prod/db"]);
        let out = resolve_all(&envs, &items);
        assert_eq!(out.pairs.len(), 2);
        let labels: Vec<_> = out.pairs.iter().map(|p| p.env_label.clone()).collect();
        assert!(labels.contains(&"env-dev".to_string()));
        assert!(labels.contains(&"env-prod".to_string()));
    }
}
