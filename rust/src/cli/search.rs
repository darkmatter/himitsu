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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub store: String,
    /// Filesystem path of the store that holds this result. Consumed by the
    /// TUI viewer (US-006) to load the secret; unused by the CLI path.
    #[allow(dead_code)]
    pub store_path: PathBuf,
    pub path: String,
    pub created_at: Option<String>,
    /// Plaintext `lastmodified` timestamp from the on-disk envelope
    /// (`YYYY-MM-DDTHH:MM:SSZ`). Populated without decryption.
    pub updated_at: Option<String>,
}

/// Run a search across all known stores and return sorted results.
///
/// Pure IO-in/data-out: no printing. Shared by the CLI (`run`) and the TUI
/// search view so both see the same results.
pub fn search_core(ctx: &Context, query: &str) -> Result<Vec<SearchResult>> {
    let needle = query.to_lowercase();
    let mut results = Vec::new();

    for (slug, store_path) in collect_stores(ctx)? {
        let paths = store::list_secrets(&store_path, None).unwrap_or_default();
        for path in paths {
            if needle.is_empty() || path.to_lowercase().contains(&needle) {
                let meta = store::read_secret_meta(&store_path, &path).ok();
                let created_at = meta.as_ref().and_then(|m| m.created_at.clone());
                let updated_at = meta.and_then(|m| m.lastmodified);
                results.push(SearchResult {
                    store: slug.clone(),
                    store_path: store_path.clone(),
                    path,
                    created_at,
                    updated_at,
                });
            }
        }
    }

    results.sort_by(|a, b| (&a.store, &a.path).cmp(&(&b.store, &b.path)));
    Ok(results)
}

pub fn run(args: SearchArgs, ctx: &Context) -> Result<()> {
    let results = search_core(ctx, &args.query)?;

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

// ── Time helpers ───────────────────────────────────────────────────────────

/// Format an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) or a bare date
/// (`YYYY-MM-DD`) as a human-readable relative time ("3 days ago").
///
/// Returns `"-"` when `ts` is `None`, and the original string when parsing
/// fails so callers still surface something useful.
pub(crate) fn relative_time(ts: Option<&str>) -> String {
    let Some(raw) = ts else {
        return "-".to_string();
    };
    let Some(then) = parse_utc_epoch(raw) else {
        return raw.to_string();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let delta = now - then;
    if delta < 0 {
        // Clock skew — treat anything in the "future" as just-now.
        return "just now".to_string();
    }
    if delta < 60 {
        return "just now".to_string();
    }
    let (n, unit) = if delta < 3600 {
        (delta / 60, "minute")
    } else if delta < 86_400 {
        (delta / 3600, "hour")
    } else if delta < 86_400 * 30 {
        (delta / 86_400, "day")
    } else if delta < 86_400 * 365 {
        (delta / (86_400 * 30), "month")
    } else {
        (delta / (86_400 * 365), "year")
    };
    let plural = if n == 1 { "" } else { "s" };
    format!("{n} {unit}{plural} ago")
}

/// Parse `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ` into epoch seconds (UTC).
fn parse_utc_epoch(raw: &str) -> Option<i64> {
    // Split "date[Thh:mm:ss[Z]]".
    let (date, time) = match raw.split_once('T') {
        Some((d, t)) => (d, Some(t.trim_end_matches('Z'))),
        None => (raw, None),
    };
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    // days_from_civil (Hinnant) — inverse of civil_date above.
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut secs = days * 86_400;
    if let Some(t) = time {
        let mut tp = t.split(':');
        let h: i64 = tp.next()?.parse().ok()?;
        let mi: i64 = tp.next()?.parse().ok()?;
        let s: i64 = tp.next().unwrap_or("0").parse().ok()?;
        secs += h * 3600 + mi * 60 + s;
    }
    Some(secs)
}

// ── Output formatters ──────────────────────────────────────────────────────

fn print_json(results: &[SearchResult]) {
    let items: Vec<_> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "store":      r.store,
                "path":       r.path,
                "created_at": r.created_at,
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
