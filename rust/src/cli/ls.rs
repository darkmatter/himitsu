use std::collections::BTreeSet;
use std::path::PathBuf;

use clap::Args;

use super::Context;
use crate::crypto::{age, secret_value, tags as tag_grammar};
use crate::error::{HimitsuError, Result};
use crate::reference::SecretRef;
use crate::remote::store;

const DEFAULT_LIMIT: usize = 24;

/// List secrets — behaves like a directory browser.
///
/// Without arguments, shows the top-level items of every known store at
/// depth 1.  Items that have children are shown with a trailing `/`;
/// leaf secrets are shown without one.  Use `-r` or `-d N` to go deeper,
/// and `--offset` / `--limit` to page through large stores.
#[derive(Debug, Args)]
pub struct LsArgs {
    /// Path prefix to list, or a qualified store reference.
    ///
    /// Bare prefix (`prod`): lists items one level deep under `prod/`
    /// in the current store.  Qualified store (`github:org/repo`) or
    /// qualified prefix (`github:org/repo/prod`) also accepted.
    pub path: Option<String>,

    /// Show all descendants recursively (overrides --depth).
    #[arg(short = 'R', long)]
    pub recursive: bool,

    /// Maximum depth to display. Default 1 (top-level only).
    #[arg(short = 'd', long, default_value_t = 1)]
    pub depth: usize,

    /// Maximum number of items to show.
    #[arg(long, default_value_t = DEFAULT_LIMIT)]
    pub limit: usize,

    /// Number of items to skip before displaying.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Filter to secrets carrying the given tag. Repeat to AND multiple tags
    /// (`--tag pci --tag rotate-2026-q1` lists only secrets that carry both).
    /// Tags follow the grammar `[A-Za-z0-9_.-]+`, 1-64 chars, case-sensitive.
    ///
    /// When set, listing decrypts each candidate with the ambient identity to
    /// inspect its tags; entries that fail to decrypt are dropped (we can't
    /// verify their tags). Without `--tag`, listing never touches the
    /// identity, so plain `himitsu ls` keeps working in CI/test environments
    /// without a key.
    #[arg(long = "tag", value_name = "TAG")]
    pub tag: Vec<String>,
}

pub fn run(args: LsArgs, ctx: &Context) -> Result<()> {
    let max_depth = if args.recursive {
        usize::MAX
    } else {
        args.depth
    };

    // Validate every requested tag once up front so we fail fast on bad input
    // before doing any store discovery or decrypting.
    for t in &args.tag {
        tag_grammar::validate_tag(t).map_err(|reason| {
            HimitsuError::InvalidReference(format!("invalid tag {t:?}: {reason}"))
        })?;
    }

    // Only load ambient identities when we actually need them for tag
    // filtering. Plain `ls` (no `--tag`) must keep working without a key —
    // CI fixtures and fresh installs don't have one yet.
    let identities = if args.tag.is_empty() {
        None
    } else {
        ctx.load_identities().ok()
    };

    // ── Resolve qualified references ──────────────────────────────────────
    if let Some(ref path_str) = args.path {
        let r = SecretRef::parse(path_str)?;
        if r.is_qualified() {
            let store_path = r.resolve_store()?;
            let prefix = r.path.as_deref();
            let slug = path_str.clone();
            return show_items(
                vec![(slug, store_path)],
                prefix,
                max_depth,
                args.limit,
                args.offset,
                &args.tag,
                identities.as_deref(),
            );
        }
    }

    // ── Collect stores ────────────────────────────────────────────────────
    let stores = collect_stores(ctx)?;

    if stores.is_empty() {
        eprintln!("No stores configured. Use `himitsu remote add <org/repo>` to add one.");
        return Ok(());
    }

    let prefix = args.path.as_deref().map(|p| p.trim_end_matches('/'));

    show_items(
        stores,
        prefix,
        max_depth,
        args.limit,
        args.offset,
        &args.tag,
        identities.as_deref(),
    )
}

// ── Core listing logic ─────────────────────────────────────────────────────

fn show_items(
    stores: Vec<(String, PathBuf)>,
    prefix: Option<&str>,
    max_depth: usize,
    limit: usize,
    offset: usize,
    want_tags: &[String],
    identities: Option<&[::age::x25519::Identity]>,
) -> Result<()> {
    // prefix_components is the number of '/' segments in the prefix itself;
    // the effective display depth is relative to the prefix.
    let prefix_depth = prefix
        .map(|p| p.split('/').filter(|s| !s.is_empty()).count())
        .unwrap_or(0);
    let effective_depth = prefix_depth.saturating_add(max_depth);

    // Collect unique (store_slug, display_path) rows.
    let mut rows: BTreeSet<(String, String)> = BTreeSet::new();

    for (slug, store_path) in &stores {
        let paths = store::list_secrets(store_path, prefix).unwrap_or_default();
        for path in paths {
            // Filter on the leaf path before depth-truncation: a directory
            // row appears whenever any secret beneath it matches, so a
            // dropped leaf can still surface as its parent when a sibling
            // matches.
            if !want_tags.is_empty() {
                let Some(ids) = identities else { continue };
                if !secret_has_all_tags(store_path, &path, ids, want_tags) {
                    continue;
                }
            }
            let display = truncate_to_depth(&path, effective_depth);
            rows.insert((slug.clone(), display));
        }
    }

    let total = rows.len();

    if total == 0 {
        match prefix {
            Some(p) => eprintln!("No secrets under '{p}'."),
            None => eprintln!("No secrets found."),
        }
        return Ok(());
    }

    let page: Vec<(String, String)> = rows.into_iter().skip(offset).take(limit).collect();

    let path_w = page
        .iter()
        .map(|(_, p)| p.len())
        .max()
        .unwrap_or(0)
        .max("PATH".len());

    let single_store = {
        let mut seen = std::collections::HashSet::new();
        for (s, _) in &page {
            seen.insert(s.as_str());
        }
        seen.len() == 1
    };

    // ── Header ────────────────────────────────────────────────────────────
    if single_store {
        println!("{:<path_w$}", "PATH");
        println!("{:-<path_w$}", "");
        for (_, path) in &page {
            println!("{path}");
        }
    } else {
        let store_w = page
            .iter()
            .map(|(s, _)| s.len())
            .max()
            .unwrap_or(0)
            .max("STORE".len());

        println!("{:<path_w$}  STORE", "PATH");
        println!("{:-<path_w$}  {:-<store_w$}", "", "");
        for (store, path) in &page {
            println!("{:<path_w$}  {store}", path);
        }
    }

    // ── Pagination footer ─────────────────────────────────────────────────
    let shown = page.len();
    let end = offset + shown;
    if total > limit || offset > 0 {
        eprintln!(
            "\n{shown} of {total}  (showing {start}–{end}; use --offset {end} for next page)",
            start = offset + 1,
        );
    } else {
        eprintln!("\n{total} item{}", if total == 1 { "" } else { "s" });
    }

    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Truncate `path` to at most `max_depth` components.
///
/// If the path has more components than `max_depth`, the return value ends
/// with `/` to indicate a directory.  Otherwise the full path is returned.
fn truncate_to_depth(path: &str, max_depth: usize) -> String {
    if max_depth == usize::MAX {
        return path.to_string();
    }
    // Split into at most max_depth + 1 parts to detect overflow.
    let parts: Vec<&str> = path.splitn(max_depth + 1, '/').collect();
    if parts.len() > max_depth {
        // Has children beyond max_depth → directory.
        parts[..max_depth].join("/") + "/"
    } else {
        path.to_string()
    }
}

/// Collect all known stores as (slug, path) pairs.
fn collect_stores(ctx: &Context) -> Result<Vec<(String, PathBuf)>> {
    let mut stores = Vec::new();

    if !ctx.store.as_os_str().is_empty() && ctx.store.exists() {
        let label = store_label(&ctx.store, ctx);
        stores.push((label, ctx.store.clone()));
    }

    let stores_dir = ctx.stores_dir();
    if stores_dir.exists() {
        for org_entry in std::fs::read_dir(&stores_dir)? {
            let org_entry = org_entry?;
            if !org_entry.file_type()?.is_dir() {
                continue;
            }
            let org = org_entry.file_name().to_string_lossy().to_string();
            for repo_entry in std::fs::read_dir(org_entry.path())? {
                let repo_entry = repo_entry?;
                if !repo_entry.file_type()?.is_dir() {
                    continue;
                }
                let repo = repo_entry.file_name().to_string_lossy().to_string();
                let store_path = repo_entry.path();
                if store_path != ctx.store {
                    stores.push((format!("{org}/{repo}"), store_path));
                }
            }
        }
    }

    Ok(stores)
}

fn store_label(store: &std::path::Path, ctx: &Context) -> String {
    if let Ok(rel) = store.strip_prefix(ctx.stores_dir()) {
        let s = rel.to_string_lossy().replace('\\', "/");
        if !s.is_empty() {
            return s;
        }
    }
    store.to_string_lossy().to_string()
}

/// Best-effort tag check: read + decrypt the secret at `secret_path` and
/// return `true` only when the decoded payload carries every tag in `want`.
///
/// Any failure (read error, decrypt error, legacy raw-bytes payload that
/// has no tags) returns `false` so the entry is dropped — we can't verify
/// what we can't decrypt.
fn secret_has_all_tags(
    store_path: &std::path::Path,
    secret_path: &str,
    identities: &[::age::x25519::Identity],
    want: &[String],
) -> bool {
    let Ok(ciphertext) = store::read_secret(store_path, secret_path) else {
        return false;
    };
    let Ok(plain) = age::decrypt_with_identities(&ciphertext, identities) else {
        return false;
    };
    let decoded = secret_value::decode(&plain);
    matches_all_tags(&decoded.tags, want)
}

/// AND-semantic tag match: returns `true` when `have` contains every entry
/// in `want`. An empty `want` always matches; an empty `have` only matches
/// an empty `want`.
fn matches_all_tags(have: &[String], want: &[String]) -> bool {
    want.iter().all(|w| have.iter().any(|h| h == w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|t| (*t).to_string()).collect()
    }

    #[test]
    fn matches_all_tags_empty_want_always_matches() {
        // An empty filter is the "no `--tag`" case — every entry should pass.
        assert!(matches_all_tags(&s(&[]), &s(&[])));
        assert!(matches_all_tags(&s(&["pci"]), &s(&[])));
        assert!(matches_all_tags(&s(&["pci", "stripe"]), &s(&[])));
    }

    #[test]
    fn matches_all_tags_subset_passes() {
        // `have` is a superset of `want` → match.
        assert!(matches_all_tags(
            &s(&["pci", "stripe", "rotate"]),
            &s(&["pci"])
        ));
        assert!(matches_all_tags(
            &s(&["pci", "stripe", "rotate"]),
            &s(&["pci", "stripe"])
        ));
    }

    #[test]
    fn matches_all_tags_exact_match_passes() {
        assert!(matches_all_tags(&s(&["pci"]), &s(&["pci"])));
        assert!(matches_all_tags(
            &s(&["pci", "stripe"]),
            &s(&["pci", "stripe"])
        ));
        // Order in `have` shouldn't matter.
        assert!(matches_all_tags(
            &s(&["stripe", "pci"]),
            &s(&["pci", "stripe"])
        ));
    }

    #[test]
    fn matches_all_tags_missing_tag_fails() {
        // One required tag absent → reject the whole entry (AND semantics).
        assert!(!matches_all_tags(&s(&["pci"]), &s(&["pci", "stripe"])));
        assert!(!matches_all_tags(&s(&["stripe"]), &s(&["pci"])));
        assert!(!matches_all_tags(&s(&[]), &s(&["pci"])));
    }

    #[test]
    fn matches_all_tags_is_case_sensitive() {
        // The grammar (`crate::crypto::tags::validate_tag`) is case-sensitive,
        // so the filter must be too. "PCI" and "pci" are distinct tags.
        assert!(!matches_all_tags(&s(&["PCI"]), &s(&["pci"])));
        assert!(!matches_all_tags(&s(&["pci"]), &s(&["PCI"])));
    }
}
