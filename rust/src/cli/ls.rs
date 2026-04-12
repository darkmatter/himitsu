use std::collections::BTreeSet;
use std::path::PathBuf;

use clap::Args;

use super::Context;
use crate::error::Result;
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
}

pub fn run(args: LsArgs, ctx: &Context) -> Result<()> {
    let max_depth = if args.recursive {
        usize::MAX
    } else {
        args.depth
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

    show_items(stores, prefix, max_depth, args.limit, args.offset)
}

// ── Core listing logic ─────────────────────────────────────────────────────

fn show_items(
    stores: Vec<(String, PathBuf)>,
    prefix: Option<&str>,
    max_depth: usize,
    limit: usize,
    offset: usize,
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
