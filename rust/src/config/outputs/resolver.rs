//! Resolution engine for `outputs:` definitions.

use std::collections::BTreeMap;

use super::dsl::{OutputDef, OutputsMap, SelectorEntry};
use super::selector::{SecretMatch, Selector};
use super::dsl::{derive_env_key, expand_brace_label};
use crate::error::HimitsuError;
use crate::reference::SecretRef;

/// A secret visible to the resolver when evaluating tag/glob selectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretCandidate {
    pub path: String,
    pub tags: Vec<String>,
}

/// Context supplied by callers that have store visibility.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub available_secrets: Vec<SecretCandidate>,
}

/// A node in an expanded output tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputNode {
    /// Terminal reference to a secret in a store.
    Leaf {
        env_key: String,
        secret_path: String,
        store_slug: Option<String>,
    },
    /// Nested map. Always sorted thanks to `BTreeMap`.
    Branch(BTreeMap<String, OutputNode>),
}

impl OutputNode {
    /// Construct an empty `Branch`.
    pub fn empty_branch() -> Self {
        OutputNode::Branch(BTreeMap::new())
    }
}

/// A resolved output entry: one env-var binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEntry {
    /// The env-var name (from alias map or derived from path).
    pub env_key: String,
    /// The secret path (possibly in a remote store).
    pub secret_path: String,
    /// The store slug if cross-store (e.g. "org/repo"), or None for local.
    pub store_slug: Option<String>,
}

/// A resolved output: one named output block fully expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOutput {
    pub name: String,
    pub entries: Vec<ResolvedEntry>,
}

/// Resolve all outputs in the given map, expanding brace-expansion names
/// and $1 captures, and returning one `ResolvedOutput` per expanded name.
pub fn resolve_outputs(
    outputs: &OutputsMap,
    ctx: &Context,
) -> Result<Vec<ResolvedOutput>, HimitsuError> {
    let mut resolved = Vec::new();
    for (name, def) in outputs {
        for (expanded_name, capture) in expand_brace_label(name) {
            resolved.push(ResolvedOutput {
                name: expanded_name,
                entries: resolve_output_def(def, &capture, ctx)?,
            });
        }
    }
    Ok(resolved)
}

fn resolve_output_def(
    def: &OutputDef,
    capture: &str,
    ctx: &Context,
) -> Result<Vec<ResolvedEntry>, HimitsuError> {
    let mut entries = Vec::new();
    for SelectorEntry(selector) in &def.selectors {
        let selector = selector.replace("$1", capture);
        entries.extend(resolve_selector_entry(&selector, ctx)?);
    }
    for (env_key, secret_ref) in &def.aliases {
        let secret_ref = secret_ref.replace("$1", capture);
        entries.push(entry_from_ref(env_key.clone(), &secret_ref)?);
    }
    Ok(entries)
}

fn resolve_selector_entry(
    selector: &str,
    ctx: &Context,
) -> Result<Vec<ResolvedEntry>, HimitsuError> {
    if is_concrete_ref(selector) {
        return Ok(vec![entry_from_ref(env_key_for_ref(selector), selector)?]);
    }

    let parsed = Selector::parse(selector)?;
    let entries = ctx
        .available_secrets
        .iter()
        .filter(|candidate| {
            parsed.matches(&SecretMatch {
                path: &candidate.path,
                tags: &candidate.tags,
            })
        })
        .map(|candidate| ResolvedEntry {
            env_key: env_key_for_ref(&candidate.path),
            secret_path: candidate.path.clone(),
            store_slug: None,
        })
        .collect();
    Ok(entries)
}

fn is_concrete_ref(selector: &str) -> bool {
    !selector.starts_with("tag:") && !selector.contains('*') && !selector.contains('?')
}

fn env_key_for_ref(secret_ref: &str) -> String {
    derive_env_key(&last_path_component(secret_ref).unwrap_or_else(|_| secret_ref.to_string()))
}

fn entry_from_ref(env_key: String, secret_ref: &str) -> Result<ResolvedEntry, HimitsuError> {
    let (store_slug, secret_path) = split_store_ref(secret_ref)?;
    Ok(ResolvedEntry {
        env_key,
        secret_path,
        store_slug,
    })
}

fn split_store_ref(secret_ref: &str) -> Result<(Option<String>, String), HimitsuError> {
    if let Some((store_ref, _)) = secret_ref.split_once('#') {
        let store = SecretRef::parse_store_ref(store_ref)?;
        let parsed = SecretRef::parse(secret_ref)?;
        let path = parsed.path.ok_or_else(|| {
            HimitsuError::InvalidReference(format!(
                "missing secret path in reference: {secret_ref}"
            ))
        })?;
        return Ok((store.store_slug.or(parsed.store_slug), path));
    }
    Ok((None, secret_ref.to_string()))
}

fn last_path_component(secret_ref: &str) -> Result<String, HimitsuError> {
    let (_, secret_path) = split_store_ref(secret_ref)?;
    Ok(secret_path
        .rsplit('/')
        .next()
        .unwrap_or(secret_path.as_str())
        .to_string())
}

#[cfg(test)]
fn wrap_in_segments(segments: &[&str], node: OutputNode) -> OutputNode {
    let mut cur = node;
    for seg in segments.iter().rev() {
        let mut map = BTreeMap::new();
        map.insert((*seg).to_string(), cur);
        cur = OutputNode::Branch(map);
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(selectors: &[&str]) -> OutputDef {
        OutputDef {
            selectors: selectors
                .iter()
                .map(|s| SelectorEntry((*s).to_string()))
                .collect(),
            aliases: BTreeMap::new(),
        }
    }

    fn aliased(pairs: &[(&str, &str)]) -> OutputDef {
        OutputDef {
            selectors: Vec::new(),
            aliases: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    fn outputs(pairs: Vec<(&str, OutputDef)>) -> OutputsMap {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn ctx(candidates: &[(&str, &[&str])]) -> Context {
        Context {
            available_secrets: candidates
                .iter()
                .map(|(path, tags)| SecretCandidate {
                    path: (*path).to_string(),
                    tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn plain_selector_list_resolves_to_entries() {
        let resolved =
            resolve_outputs(&outputs(vec![("web", def(&["prod/api-key"]))]), &ctx(&[])).unwrap();

        assert_eq!(resolved[0].entries[0].secret_path, "prod/api-key");
    }

    #[test]
    fn brace_expansion_produces_multiple_outputs() {
        let resolved = resolve_outputs(
            &outputs(vec![("web-{dev,staging}", def(&["common/api-key"]))]),
            &ctx(&[]),
        )
        .unwrap();

        let names: Vec<&str> = resolved.iter().map(|output| output.name.as_str()).collect();
        assert_eq!(names, vec!["web-dev", "web-staging"]);
    }

    #[test]
    fn dollar_one_capture_substitutes_brace_segment() {
        let resolved = resolve_outputs(
            &outputs(vec![("web-{dev,staging}", def(&["$1/database-url"]))]),
            &ctx(&[]),
        )
        .unwrap();

        assert_eq!(resolved[0].entries[0].secret_path, "dev/database-url");
        assert_eq!(resolved[1].entries[0].secret_path, "staging/database-url");
    }

    #[test]
    fn cross_store_ref_extracts_store_slug_and_secret_path() {
        let resolved = resolve_outputs(
            &outputs(vec![("prod", def(&["github:org/secrets#prod/api-key"]))]),
            &ctx(&[]),
        )
        .unwrap();

        assert_eq!(
            resolved[0].entries[0].store_slug,
            Some("org/secrets".into())
        );
        assert_eq!(resolved[0].entries[0].secret_path, "prod/api-key");
    }

    #[test]
    fn alias_map_uses_alias_key_as_env_key() {
        let resolved = resolve_outputs(
            &outputs(vec![("stripe", aliased(&[("STRIPE", "tag:stripe")]))]),
            &ctx(&[]),
        )
        .unwrap();

        assert_eq!(resolved[0].entries[0].env_key, "STRIPE");
        assert_eq!(resolved[0].entries[0].secret_path, "tag:stripe");
    }

    #[test]
    fn empty_selectors_yield_empty_entries() {
        let resolved = resolve_outputs(&outputs(vec![("empty", def(&[]))]), &ctx(&[])).unwrap();

        assert!(resolved[0].entries.is_empty());
    }

    #[test]
    fn multiple_selectors_yield_multiple_entries() {
        let resolved = resolve_outputs(
            &outputs(vec![("web", def(&["prod/a", "prod/b"]))]),
            &ctx(&[]),
        )
        .unwrap();

        assert_eq!(resolved[0].entries.len(), 2);
    }

    #[test]
    fn no_brace_expansion_yields_single_output_with_same_name() {
        let resolved =
            resolve_outputs(&outputs(vec![("web", def(&["prod/a"]))]), &ctx(&[])).unwrap();

        assert_eq!(resolved[0].name, "web");
    }

    #[test]
    fn path_tail_derives_uppercase_env_key() {
        let resolved =
            resolve_outputs(&outputs(vec![("web", def(&["prod/api-key"]))]), &ctx(&[])).unwrap();

        assert_eq!(resolved[0].entries[0].env_key, "API_KEY");
    }

    #[test]
    fn glob_selector_expands_available_secret_matches() {
        let resolved = resolve_outputs(
            &outputs(vec![("prod", def(&["prod/*"]))]),
            &ctx(&[("prod/a", &[]), ("prod/b", &[]), ("dev/c", &[])]),
        )
        .unwrap();

        assert_eq!(resolved[0].entries.len(), 2);
    }

    #[test]
    fn and_combined_selectors_require_all_tokens() {
        let resolved = resolve_outputs(
            &outputs(vec![("pci-prod", def(&["tag:pci+tag:prod"]))]),
            &ctx(&[("prod/a", &["pci", "prod"]), ("prod/b", &["pci"])]),
        )
        .unwrap();

        assert_eq!(resolved[0].entries.len(), 1);
        assert_eq!(resolved[0].entries[0].secret_path, "prod/a");
    }

    #[test]
    fn wrap_in_segments_builds_nested_output_tree() {
        let node = wrap_in_segments(
            &["web", "dev"],
            OutputNode::Leaf {
                env_key: "API_KEY".into(),
                secret_path: "dev/api-key".into(),
                store_slug: None,
            },
        );

        assert!(matches!(node, OutputNode::Branch(_)));
    }
}
