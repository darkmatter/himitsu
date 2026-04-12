use std::path::PathBuf;

use clap::Args;

use super::Context;
use crate::error::Result;
use crate::remote::store;

/// Search secrets across all known stores.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Search query to match against secret paths.
    pub query: String,

    /// No-op — kept for backward compatibility. Search now always reads
    /// directly from store files so no separate refresh step is required.
    #[arg(long)]
    pub refresh: bool,

    /// Output as JSON array (for machine/TUI consumption).
    #[arg(long, hide = true)]
    pub json: bool,
}

/// A single search result.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub store: String,
    pub path: String,
}

pub fn run(args: SearchArgs, ctx: &Context) -> Result<()> {
    let query = args.query.to_lowercase();
    let mut results = Vec::new();

    for (slug, store_path) in collect_stores(ctx)? {
        let paths = store::list_secrets(&store_path, None).unwrap_or_default();
        for path in paths {
            if path.to_lowercase().contains(&query) {
                results.push(SearchResult {
                    store: slug.clone(),
                    path,
                });
            }
        }
    }

    results.sort_by(|a, b| (&a.store, &a.path).cmp(&(&b.store, &b.path)));

    if args.json {
        print_json(&results);
    } else {
        print_table(&results, &args.query);
    }

    Ok(())
}

// ── Store discovery ────────────────────────────────────────────────────────

fn collect_stores(ctx: &Context) -> Result<Vec<(String, PathBuf)>> {
    let mut stores = Vec::new();

    // If the caller specified an explicit store (--store / --remote), include
    // it first so its secrets always appear in results.
    if !ctx.store.as_os_str().is_empty() && ctx.store.exists() {
        let label = store_label(&ctx.store, ctx);
        stores.push((label, ctx.store.clone()));
    }

    // Also scan stores_dir for all registered remote checkouts.
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
                // Skip if already included via the explicit --store flag.
                if store_path != ctx.store {
                    stores.push((format!("{org}/{repo}"), store_path));
                }
            }
        }
    }

    Ok(stores)
}

/// Derive a human-readable label for a store path.
///
/// If the path is under `stores_dir`, returns the `org/repo` slug.
/// Otherwise falls back to the full path string.
fn store_label(store: &std::path::Path, ctx: &Context) -> String {
    if let Ok(rel) = store.strip_prefix(ctx.stores_dir()) {
        let s = rel.to_string_lossy().replace('\\', "/");
        if !s.is_empty() {
            return s;
        }
    }
    store.to_string_lossy().to_string()
}

// ── Output formatters ──────────────────────────────────────────────────────

fn print_json(results: &[SearchResult]) {
    let items: Vec<_> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "store": r.store,
                "path":  r.path,
            })
        })
        .collect();
    println!("{}", serde_json::to_string(&items).unwrap_or_default());
}

fn print_table(results: &[SearchResult], query: &str) {
    if results.is_empty() {
        eprintln!("No results for {query:?}.");
        eprintln!("Tip: run `himitsu remote add <org/repo>` to register stores.");
        return;
    }

    let path_w = results
        .iter()
        .map(|r| r.path.len())
        .max()
        .unwrap_or(0)
        .max("PATH".len());

    let store_w = results
        .iter()
        .map(|r| r.store.len())
        .max()
        .unwrap_or(0)
        .max("STORE".len());

    println!("{:<path_w$}  STORE", "PATH");
    println!("{:-<path_w$}  {:-<store_w$}", "", "");

    for r in results {
        println!("{:<path_w$}  {}", r.path, r.store);
    }
}
