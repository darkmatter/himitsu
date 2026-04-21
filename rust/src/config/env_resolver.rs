//! Pure-function resolver for preset env labels.
//!
//! Expands a concrete or wildcard env label (see [`super::validate_env_label`])
//! into a deterministic, nested [`EnvNode`] tree. The tree feeds both the TUI
//! preview and `himitsu codegen <env>`; both need identical, sorted output
//! regardless of how entries were authored in YAML.
//!
//! ## Semantics
//!
//! - A concrete env (e.g. `foo/bar`) produces a flat `Branch` whose keys come
//!   from the entries themselves: the last segment of `Single`, the explicit
//!   `key` of `Alias`, and the last segment of every secret matched by `Glob`.
//! - A wildcard env (e.g. `foo/*`) binds `$1` to the first segment *after* the
//!   concrete prefix of every matching secret path in `available_secrets`. For
//!   each discovered `$1` value we produce a sub-branch; every entry in the
//!   env is resolved under that sub-branch with `$1` substituted.
//! - Wildcard resolution never peeks at sibling env labels. It walks the store
//!   (via `available_secrets`) only.
//!
//! The tree is rooted at the label's concrete prefix: `foo/*` → `foo`,
//! `foo/bar` → `foo/bar` (nested as `foo` → `bar`).

use std::collections::BTreeMap;

use super::{is_wildcard_label, label_prefix_segments, validate_env_label, EnvEntry};
use crate::error::{HimitsuError, Result};

/// A node in the resolved env tree.
///
/// `Leaf` carries the final secret path (unmodified after `$1` substitution);
/// callers dereference it against the store to obtain the plaintext value.
/// `Branch` is a sorted map so output is byte-identical across runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvNode {
    /// Terminal reference to a secret in the store.
    Leaf { secret_path: String },
    /// Nested map. Always sorted thanks to `BTreeMap`.
    Branch(BTreeMap<String, EnvNode>),
}

impl EnvNode {
    /// Construct an empty `Branch`.
    pub fn empty_branch() -> Self {
        EnvNode::Branch(BTreeMap::new())
    }
}

/// Resolve a single env label (concrete or wildcard) against the full env map.
///
/// - `envs`: the full [`super::ProjectConfig::envs`] map. Wildcard resolution
///   does **not** consult sibling labels; the map is only used to look up the
///   target's entries.
/// - `target`: the label to resolve. Validated with
///   [`validate_env_label`] before anything else.
/// - `available_secrets`: a list of secret paths that exist in the store. The
///   list need not be pre-sorted — `BTreeMap` sorts the output. Pass an empty
///   slice when the caller has no store visibility; wildcards then resolve to
///   an empty `Branch` rooted at their concrete prefix.
///
/// Returns an [`EnvNode`] tree rooted at the label's concrete prefix:
/// a concrete `foo/bar` returns `Branch{foo: Branch{bar: <entries>}}`, and a
/// wildcard `foo/*` returns `Branch{foo: Branch{<discovered>: <entries>}}`.
pub fn resolve(
    envs: &BTreeMap<String, Vec<EnvEntry>>,
    target: &str,
    available_secrets: &[String],
) -> Result<EnvNode> {
    validate_env_label(target)?;

    let entries = envs.get(target).ok_or_else(|| {
        HimitsuError::InvalidConfig(format!("unknown env label '{target}'"))
    })?;

    let prefix_segments = label_prefix_segments(target);

    if is_wildcard_label(target) {
        resolve_wildcard(entries, &prefix_segments, available_secrets)
    } else {
        resolve_concrete(entries, &prefix_segments, available_secrets)
    }
}

/// Build the flat leaf map for a concrete env's entries. Does not wrap the
/// map in prefix segments — the caller handles that.
fn build_concrete_entries(
    entries: &[EnvEntry],
    available_secrets: &[String],
) -> Result<BTreeMap<String, EnvNode>> {
    let mut out: BTreeMap<String, EnvNode> = BTreeMap::new();
    for entry in entries {
        match entry {
            EnvEntry::Single(path) => {
                let key = last_segment(path).ok_or_else(|| {
                    HimitsuError::InvalidConfig(format!(
                        "env entry path '{path}' has no final segment"
                    ))
                })?;
                out.insert(
                    key.to_string(),
                    EnvNode::Leaf {
                        secret_path: path.clone(),
                    },
                );
            }
            EnvEntry::Alias { key, path } => {
                out.insert(
                    key.clone(),
                    EnvNode::Leaf {
                        secret_path: path.clone(),
                    },
                );
            }
            EnvEntry::Glob(prefix) => {
                let needle = format!("{prefix}/");
                for secret in available_secrets {
                    if secret.starts_with(&needle) {
                        let key = last_segment(secret).ok_or_else(|| {
                            HimitsuError::InvalidConfig(format!(
                                "secret path '{secret}' has no final segment"
                            ))
                        })?;
                        out.insert(
                            key.to_string(),
                            EnvNode::Leaf {
                                secret_path: secret.clone(),
                            },
                        );
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Resolve a concrete env. Returns a tree rooted at the label's segments:
/// `foo/bar` → `{foo: {bar: <entries>}}`.
fn resolve_concrete(
    entries: &[EnvEntry],
    prefix_segments: &[&str],
    available_secrets: &[String],
) -> Result<EnvNode> {
    let leaf_map = build_concrete_entries(entries, available_secrets)?;
    Ok(wrap_in_segments(prefix_segments, EnvNode::Branch(leaf_map)))
}

/// Resolve a wildcard env. The `$1` capture is populated by matching each
/// entry's path (treated as a segment-wise pattern where `$1` is a
/// single-segment wildcard) against `available_secrets`. Discovered values
/// are unioned across all entries and become the sub-branch keys.
fn resolve_wildcard(
    entries: &[EnvEntry],
    prefix_segments: &[&str],
    available_secrets: &[String],
) -> Result<EnvNode> {
    // 1. Discover all candidate `$1` values by matching each entry's path
    //    against the secret store. Entries without `$1` contribute nothing
    //    to discovery — they're constant and apply uniformly to every
    //    discovered capture.
    let mut captures: BTreeMap<String, ()> = BTreeMap::new();
    for entry in entries {
        let path = match entry {
            EnvEntry::Single(p) | EnvEntry::Glob(p) => p,
            EnvEntry::Alias { path, .. } => path,
        };
        if !path.contains("$1") {
            continue;
        }
        for value in match_dollar_one(path, available_secrets, entry_is_glob(entry)) {
            captures.insert(value, ());
        }
    }

    // 2. For each discovered `$1` value, substitute and build a concrete
    //    sub-branch. After substitution, entries resolve with the same
    //    logic as a concrete env (Single → explicit leaf, Glob → prefix
    //    enumeration, Alias → keyed leaf).
    let mut children: BTreeMap<String, EnvNode> = BTreeMap::new();
    for capture in captures.keys() {
        let substituted = substitute_entries(entries, capture);
        let leaf_map = build_concrete_entries(&substituted, available_secrets)?;
        children.insert(capture.clone(), EnvNode::Branch(leaf_map));
    }

    Ok(wrap_in_segments(
        prefix_segments,
        EnvNode::Branch(children),
    ))
}

fn entry_is_glob(e: &EnvEntry) -> bool {
    matches!(e, EnvEntry::Glob(_))
}

/// Match `pattern` against `secrets`, returning every distinct value that
/// `$1` could bind to. The pattern is split segment-wise on `/`; the single
/// segment equal to `$1` (exactly) acts as a one-segment wildcard, and all
/// other segments must match literally.
///
/// `is_glob_prefix`: when true, the pattern is a `Glob` prefix — secrets
/// only need to *start with* `<pattern>/`, not match exactly. When false
/// (`Single` / `Alias`), the whole path must match.
///
/// A leading `/` on the pattern is normalised away so `/$1/foo` and
/// `$1/foo` behave identically.
fn match_dollar_one(
    pattern: &str,
    secrets: &[String],
    is_glob_prefix: bool,
) -> Vec<String> {
    let pat = pattern.strip_prefix('/').unwrap_or(pattern);
    let pat_segs: Vec<&str> = pat.split('/').collect();

    // Locate the `$1` segment. Captures elsewhere in the path are out of
    // scope (validation upstream rejects `$N` for N != 1, and we only
    // support a single `$1` here).
    let capture_idx = match pat_segs.iter().position(|s| *s == "$1") {
        Some(i) => i,
        None => return Vec::new(),
    };

    let mut out: Vec<String> = Vec::new();
    'outer: for secret in secrets {
        let s = secret.strip_prefix('/').unwrap_or(secret);
        let s_segs: Vec<&str> = s.split('/').collect();

        if is_glob_prefix {
            // Prefix match: secret must have at least pat_segs.len() + 1
            // segments (one extra for the concrete tail under the glob).
            if s_segs.len() <= pat_segs.len() {
                continue;
            }
        } else {
            // Exact match: lengths must line up.
            if s_segs.len() != pat_segs.len() {
                continue;
            }
        }

        for (i, p) in pat_segs.iter().enumerate() {
            if i == capture_idx {
                continue;
            }
            if s_segs[i] != *p {
                continue 'outer;
            }
        }

        let captured = s_segs[capture_idx].to_string();
        if !captured.is_empty() && !out.contains(&captured) {
            out.push(captured);
        }
    }
    out
}

/// Replace every `$1` occurrence in entry paths with `value`. Entry kinds are
/// preserved; keys on `Alias` entries are left alone (captures there would
/// collide across different `$1` bindings).
fn substitute_entries(entries: &[EnvEntry], value: &str) -> Vec<EnvEntry> {
    entries
        .iter()
        .map(|e| match e {
            EnvEntry::Single(p) => EnvEntry::Single(substitute_dollar_one(p, value)),
            EnvEntry::Glob(p) => EnvEntry::Glob(substitute_dollar_one(p, value)),
            EnvEntry::Alias { key, path } => EnvEntry::Alias {
                key: key.clone(),
                path: substitute_dollar_one(path, value),
            },
        })
        .collect()
}

/// Replace `$1` in `s` with `value`. Uses the same scanner as
/// [`parse_captures`] so behaviour stays consistent: only bare `$1` is
/// substituted, higher indices are left intact (they fail validation upstream).
fn substitute_dollar_one(s: &str, value: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let digits = std::str::from_utf8(&bytes[start..j]).unwrap();
            if digits == "1" {
                out.push_str(value);
            } else {
                // Leave foreign captures literal; `validate_envs` rejects
                // them before we get here in practice.
                out.push('$');
                out.push_str(digits);
            }
            i = j;
        } else {
            // UTF-8 safe: push the char, not the byte.
            let ch_start = i;
            // Advance one UTF-8 code point.
            let ch = s[ch_start..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Wrap a node inside successive `Branch`es so the tree is rooted at the
/// label's concrete prefix. `segments = ["foo", "bar"]` produces
/// `Branch{foo: Branch{bar: node}}`.
fn wrap_in_segments(segments: &[&str], node: EnvNode) -> EnvNode {
    let mut cur = node;
    for seg in segments.iter().rev() {
        let mut map = BTreeMap::new();
        map.insert((*seg).to_string(), cur);
        cur = EnvNode::Branch(map);
    }
    cur
}

/// Return the last `/`-separated segment of `path`, ignoring leading slashes.
/// `"/dev/postgres-url"` → `Some("postgres-url")`, `""` → `None`.
fn last_segment(path: &str) -> Option<&str> {
    path.rsplit('/').find(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envs(pairs: Vec<(&str, Vec<EnvEntry>)>) -> BTreeMap<String, Vec<EnvEntry>> {
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn strs(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    // Helper: descend into a Branch and return the inner map or panic.
    fn branch(n: &EnvNode) -> &BTreeMap<String, EnvNode> {
        match n {
            EnvNode::Branch(m) => m,
            EnvNode::Leaf { secret_path } => {
                panic!("expected Branch, got Leaf({secret_path})")
            }
        }
    }

    fn leaf_path(n: &EnvNode) -> &str {
        match n {
            EnvNode::Leaf { secret_path } => secret_path,
            _ => panic!("expected Leaf"),
        }
    }

    #[test]
    fn concrete_label_with_single_alias_glob() {
        let e = envs(vec![(
            "dev",
            vec![
                EnvEntry::Single("dev/API_KEY".into()),
                EnvEntry::Alias {
                    key: "DB".into(),
                    path: "dev/DB_PASS".into(),
                },
                EnvEntry::Glob("extras".into()),
            ],
        )]);
        let secrets = strs(&["extras/TOKEN_A", "extras/TOKEN_B", "unrelated/x"]);
        let tree = resolve(&e, "dev", &secrets).unwrap();

        let root = branch(&tree);
        let dev = branch(root.get("dev").unwrap());

        assert_eq!(leaf_path(dev.get("API_KEY").unwrap()), "dev/API_KEY");
        assert_eq!(leaf_path(dev.get("DB").unwrap()), "dev/DB_PASS");
        assert_eq!(leaf_path(dev.get("TOKEN_A").unwrap()), "extras/TOKEN_A");
        assert_eq!(leaf_path(dev.get("TOKEN_B").unwrap()), "extras/TOKEN_B");
        assert!(dev.get("x").is_none());
    }

    #[test]
    fn wildcard_with_zero_matches_is_empty_branch() {
        let e = envs(vec![(
            "foo/*",
            vec![EnvEntry::Alias {
                key: "POSTGRES".into(),
                path: "/$1/postgres-url".into(),
            }],
        )]);
        let tree = resolve(&e, "foo/*", &[]).unwrap();
        let root = branch(&tree);
        let foo = branch(root.get("foo").unwrap());
        assert!(foo.is_empty(), "expected empty branch, got {foo:?}");
    }

    #[test]
    fn wildcard_single_match() {
        let e = envs(vec![(
            "foo/*",
            vec![EnvEntry::Alias {
                key: "POSTGRES".into(),
                path: "$1/postgres-url".into(),
            }],
        )]);
        let secrets = strs(&["dev/postgres-url"]);
        let tree = resolve(&e, "foo/*", &secrets).unwrap();
        let foo = branch(branch(&tree).get("foo").unwrap());
        let dev = branch(foo.get("dev").unwrap());
        assert_eq!(leaf_path(dev.get("POSTGRES").unwrap()), "dev/postgres-url");
    }

    /// Canonical example from the task brief.
    #[test]
    fn wildcard_canonical_dollar_one_example() {
        let e = envs(vec![(
            "foo/*",
            vec![EnvEntry::Alias {
                key: "POSTGRES".into(),
                path: "/$1/postgres-url".into(),
            }],
        )]);
        let secrets = strs(&["dev/postgres-url", "prod/postgres-url"]);
        let tree = resolve(&e, "foo/*", &secrets).unwrap();

        // Root shape: { foo: { dev: { POSTGRES: "/dev/postgres-url" },
        //                      prod: { POSTGRES: "/prod/postgres-url" } } }
        let root = branch(&tree);
        assert_eq!(root.len(), 1);
        let foo = branch(root.get("foo").unwrap());
        assert_eq!(foo.len(), 2);

        let dev = branch(foo.get("dev").unwrap());
        assert_eq!(leaf_path(dev.get("POSTGRES").unwrap()), "/dev/postgres-url");

        let prod = branch(foo.get("prod").unwrap());
        assert_eq!(
            leaf_path(prod.get("POSTGRES").unwrap()),
            "/prod/postgres-url"
        );
    }

    #[test]
    fn wildcard_capture_in_single_entry_path() {
        let e = envs(vec![(
            "svc/*",
            vec![EnvEntry::Single("$1/API_KEY".into())],
        )]);
        let secrets = strs(&["alpha/API_KEY", "beta/API_KEY"]);
        let tree = resolve(&e, "svc/*", &secrets).unwrap();
        let svc = branch(branch(&tree).get("svc").unwrap());

        let alpha = branch(svc.get("alpha").unwrap());
        assert_eq!(leaf_path(alpha.get("API_KEY").unwrap()), "alpha/API_KEY");

        let beta = branch(svc.get("beta").unwrap());
        assert_eq!(leaf_path(beta.get("API_KEY").unwrap()), "beta/API_KEY");
    }

    #[test]
    fn unknown_target_errors() {
        let e = envs(vec![]);
        let err = resolve(&e, "nope", &[]).unwrap_err();
        assert!(err.to_string().contains("unknown env label"));
    }

    #[test]
    fn invalid_label_errors() {
        let e = envs(vec![]);
        assert!(resolve(&e, "foo/*/bar", &[]).is_err());
        assert!(resolve(&e, "", &[]).is_err());
    }

    #[test]
    fn deterministic_key_ordering() {
        // Author entries in reverse and shuffled secret order; BTreeMap
        // should still produce identical sorted output on every run.
        let e = envs(vec![(
            "foo/*",
            vec![
                EnvEntry::Alias {
                    key: "Z_KEY".into(),
                    path: "$1/z".into(),
                },
                EnvEntry::Alias {
                    key: "A_KEY".into(),
                    path: "$1/a".into(),
                },
            ],
        )]);
        let secrets_a = strs(&["zeta/z", "zeta/a", "alpha/z", "alpha/a"]);
        let secrets_b = strs(&["alpha/a", "zeta/a", "alpha/z", "zeta/z"]);

        let ta = resolve(&e, "foo/*", &secrets_a).unwrap();
        let tb = resolve(&e, "foo/*", &secrets_b).unwrap();
        assert_eq!(ta, tb);

        // And inspect: first key under foo must be "alpha" (sorted).
        let foo = branch(branch(&ta).get("foo").unwrap());
        let first = foo.keys().next().unwrap();
        assert_eq!(first, "alpha");

        let alpha = branch(foo.get("alpha").unwrap());
        let first_inner = alpha.keys().next().unwrap();
        assert_eq!(first_inner, "A_KEY");
    }

    #[test]
    fn concrete_single_with_absolute_path_uses_last_segment() {
        let e = envs(vec![("env1", vec![EnvEntry::Single("/dev/postgres-url".into())])]);
        let tree = resolve(&e, "env1", &[]).unwrap();
        let env1 = branch(branch(&tree).get("env1").unwrap());
        assert_eq!(
            leaf_path(env1.get("postgres-url").unwrap()),
            "/dev/postgres-url"
        );
    }

    #[test]
    fn wildcard_pattern_requires_segment_count_match() {
        // `$1/x` is a 2-segment pattern. Only secrets with exactly two
        // segments whose second segment is `x` should match. Longer paths
        // like `foo/alpha/x` and unrelated paths like `foo/baz` must not
        // contribute captures.
        let e = envs(vec![(
            "foo/*",
            vec![EnvEntry::Alias {
                key: "V".into(),
                path: "$1/x".into(),
            }],
        )]);
        let secrets = strs(&[
            "alpha/x",       // matches, $1 = "alpha"
            "beta/x",        // matches, $1 = "beta"
            "gamma/y",       // second segment wrong
            "foo/bar/x",     // too deep
        ]);
        let tree = resolve(&e, "foo/*", &secrets).unwrap();
        let foo = branch(branch(&tree).get("foo").unwrap());
        assert_eq!(foo.len(), 2);
        assert!(foo.contains_key("alpha"));
        assert!(foo.contains_key("beta"));
    }

    #[test]
    fn nested_concrete_label_nests_properly() {
        let e = envs(vec![(
            "foo/bar",
            vec![EnvEntry::Single("x/API_KEY".into())],
        )]);
        let tree = resolve(&e, "foo/bar", &[]).unwrap();
        let root = branch(&tree);
        let foo = branch(root.get("foo").unwrap());
        let bar = branch(foo.get("bar").unwrap());
        assert_eq!(leaf_path(bar.get("API_KEY").unwrap()), "x/API_KEY");
    }

    #[test]
    fn substitute_dollar_one_preserves_unicode() {
        assert_eq!(substitute_dollar_one("héllo/$1", "wörld"), "héllo/wörld");
        assert_eq!(substitute_dollar_one("no-capture", "x"), "no-capture");
    }
}
