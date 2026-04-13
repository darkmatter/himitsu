use std::io::{self, IsTerminal};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::Args;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use owo_colors::OwoColorize;

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
    pub updated_at: Option<String>,
}

/// Run a search across all known stores and return sorted results.
///
/// Pure IO-in/data-out: no printing. Shared by the CLI (`run`) and the TUI
/// search view so both see the same results.
pub fn search_core(ctx: &Context, query: &str) -> Result<Vec<SearchResult>> {
    let mut candidates: Vec<SearchResult> = Vec::new();

    for (slug, store_path) in collect_stores(ctx)? {
        let paths = store::list_secrets(&store_path, None).unwrap_or_default();
        for path in paths {
            let meta = store::read_secret_meta(&store_path, &path).ok();
            let (created_at, updated_at) = match meta {
                Some(m) => (m.created_at, m.lastmodified),
                None => (None, None),
            };
            candidates.push(SearchResult {
                store: slug.clone(),
                store_path: store_path.clone(),
                path,
                created_at,
                updated_at,
            });
        }
    }

    let results = if query.trim().is_empty() {
        let mut r = candidates;
        r.sort_by(|a, b| (&a.store, &a.path).cmp(&(&b.store, &b.path)));
        r
    } else {
        fuzzy_filter(&candidates, query)
    };

    Ok(results)
}

/// Score every candidate against `query`, returning matches sorted by
/// descending score with ties broken alphabetically by (store, path).
fn fuzzy_filter(candidates: &[SearchResult], query: &str) -> Vec<SearchResult> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);

    let mut scored: Vec<(u32, SearchResult)> = Vec::new();
    for c in candidates {
        let path_score = pattern.score(
            nucleo_matcher::Utf32Str::Ascii(c.path.as_bytes()),
            &mut matcher,
        );
        // Only include the store slug in scoring when it looks like an
        // `org/repo` slug — not a filesystem fallback like `/tmp/.../store`.
        // This keeps `himitsu search acme/stripe` working while preventing
        // random temp-dir chars from matching path-only queries.
        let slug_score = if is_slug_like(&c.store) {
            let slug_haystack = format!("{}/{}", c.store, c.path);
            pattern.score(
                nucleo_matcher::Utf32Str::Ascii(slug_haystack.as_bytes()),
                &mut matcher,
            )
        } else {
            None
        };
        let best = match (path_score, slug_score) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        if let Some(score) = best {
            scored.push((score, c.clone()));
        }
    }

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| (&a.1.store, &a.1.path).cmp(&(&b.1.store, &b.1.path)))
    });
    scored.into_iter().map(|(_, r)| r).collect()
}

pub fn run(args: SearchArgs, ctx: &Context) -> Result<()> {
    let results = search_core(ctx, &args.query)?;

    if args.json {
        print_json(&results);
    } else {
        let use_color = io::stdout().is_terminal();
        print_table(&results, &args.query, use_color, Utc::now());
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
                "store":      r.store,
                "path":       r.path,
                "created_at": r.created_at,
                "updated_at": r.updated_at,
            })
        })
        .collect();
    println!("{}", serde_json::to_string(&items).unwrap_or_default());
}

fn print_table(results: &[SearchResult], query: &str, use_color: bool, now: DateTime<Utc>) {
    if results.is_empty() {
        eprintln!("No results for {query:?}.");
        eprintln!("Tip: run `himitsu remote add <org/repo>` to register stores.");
        return;
    }

    let out = render_table(results, use_color, now);
    print!("{out}");
}

/// Render a gh-style table to a String. Pulled out so tests can snapshot it.
fn render_table(results: &[SearchResult], use_color: bool, now: DateTime<Utc>) -> String {
    let rows: Vec<(String, String, String)> = results
        .iter()
        .map(|r| {
            let ts = r.updated_at.as_deref().or(r.created_at.as_deref());
            let updated = ts.map(|t| relative_time(t, now)).unwrap_or_else(|| "-".to_string());
            (r.path.clone(), r.store.clone(), updated)
        })
        .collect();

    let path_w = rows
        .iter()
        .map(|(p, _, _)| p.len())
        .max()
        .unwrap_or(0)
        .max("PATH".len());
    let store_w = rows
        .iter()
        .map(|(_, s, _)| s.len())
        .max()
        .unwrap_or(0)
        .max("STORE".len());

    let mut buf = String::new();
    buf.push_str(&format!(
        "{:<path_w$}  {:<store_w$}  UPDATED\n",
        "PATH", "STORE",
    ));

    for (path, store, updated) in rows {
        let path_cell = format!("{path:<path_w$}");
        let store_cell = format!("{store:<store_w$}");
        if use_color {
            buf.push_str(&format!(
                "{}  {}  {}\n",
                path_cell.cyan(),
                store_cell,
                updated.dimmed(),
            ));
        } else {
            buf.push_str(&format!("{path_cell}  {store_cell}  {updated}\n"));
        }
    }

    buf
}

/// Format an ISO 8601 / RFC3339 timestamp (or a plain `YYYY-MM-DD` date) as a
/// gh-style relative time like "2 hours ago", "3 days ago", "just now".
pub(crate) fn relative_time(ts: &str, now: DateTime<Utc>) -> String {
    let parsed = DateTime::parse_from_rfc3339(ts)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(ts, "%Y-%m-%d").map(|d| {
                DateTime::<Utc>::from_naive_utc_and_offset(
                    d.and_hms_opt(0, 0, 0).unwrap_or_default(),
                    Utc,
                )
            })
        });
    let Ok(then) = parsed else {
        return ts.to_string();
    };

    let delta = now.signed_duration_since(then);
    let secs = delta.num_seconds();
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = delta.num_minutes();
    if mins < 60 {
        return format!("{mins} minute{} ago", plural(mins));
    }
    let hours = delta.num_hours();
    if hours < 24 {
        return format!("about {hours} hour{} ago", plural(hours));
    }
    let days = delta.num_days();
    if days < 30 {
        return format!("{days} day{} ago", plural(days));
    }
    let months = days / 30;
    if months < 12 {
        return format!("{months} month{} ago", plural(months));
    }
    let years = days / 365;
    format!("{years} year{} ago", plural(years))
}

/// A slug is "slug-like" when it reads as `org/repo` rather than a fallback
/// absolute filesystem path. We reject paths that start with `/` or contain
/// a Windows drive letter prefix.
fn is_slug_like(slug: &str) -> bool {
    if slug.is_empty() || slug.starts_with('/') {
        return false;
    }
    let bytes = slug.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return false;
    }
    true
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn mk(store: &str, path: &str, updated_at: Option<&str>) -> SearchResult {
        SearchResult {
            store: store.to_string(),
            store_path: PathBuf::new(),
            path: path.to_string(),
            created_at: None,
            updated_at: updated_at.map(String::from),
        }
    }

    #[test]
    fn test_fuzzy_matches_subsequence() {
        let candidates = vec![
            mk("acme/prod", "DATABASE_URL", None),
            mk("acme/prod", "API_KEY", None),
            mk("acme/staging", "DATABASE_URL", None),
            mk("acme/prod", "STRIPE_KEY", None),
        ];
        let hits = fuzzy_filter(&candidates, "dbu");
        assert!(!hits.is_empty(), "expected fuzzy match for 'dbu'");
        assert!(hits.iter().all(|r| r.path == "DATABASE_URL"));

        let hits = fuzzy_filter(&candidates, "stripe");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "STRIPE_KEY");

        // Slug match: query includes store segment.
        let hits = fuzzy_filter(&candidates, "staging/dburl");
        assert!(hits.iter().any(|r| r.store == "acme/staging"));

        // Non-matching query yields empty.
        let hits = fuzzy_filter(&candidates, "zzzzz_nope");
        assert!(hits.is_empty());
    }

    #[test]
    fn test_relative_time_formats() {
        let now = Utc.with_ymd_and_hms(2026, 4, 12, 12, 0, 0).unwrap();

        let t = now - chrono::Duration::seconds(10);
        assert_eq!(relative_time(&t.to_rfc3339(), now), "just now");

        let t = now - chrono::Duration::minutes(5);
        assert_eq!(relative_time(&t.to_rfc3339(), now), "5 minutes ago");

        let t = now - chrono::Duration::minutes(1);
        assert_eq!(relative_time(&t.to_rfc3339(), now), "1 minute ago");

        let t = now - chrono::Duration::hours(2);
        assert_eq!(relative_time(&t.to_rfc3339(), now), "about 2 hours ago");

        let t = now - chrono::Duration::days(3);
        assert_eq!(relative_time(&t.to_rfc3339(), now), "3 days ago");

        // Plain date form (himitsu created_at uses YYYY-MM-DD).
        assert_eq!(relative_time("2026-04-09", now), "3 days ago");
    }

    #[test]
    fn test_gh_style_table_output() {
        let now = Utc.with_ymd_and_hms(2026, 4, 12, 12, 0, 0).unwrap();
        let two_hours_ago = (now - chrono::Duration::hours(2)).to_rfc3339();
        let three_days_ago = (now - chrono::Duration::days(3)).to_rfc3339();

        let results = vec![
            SearchResult {
                store: "acme/prod".into(),
                store_path: PathBuf::new(),
                path: "DATABASE_URL".into(),
                created_at: None,
                updated_at: Some(two_hours_ago),
            },
            SearchResult {
                store: "acme/staging".into(),
                store_path: PathBuf::new(),
                path: "API_KEY".into(),
                created_at: None,
                updated_at: Some(three_days_ago),
            },
        ];

        let out = render_table(&results, false, now);
        let mut lines = out.lines();

        let header = lines.next().unwrap();
        assert!(header.starts_with("PATH"));
        assert!(header.contains("STORE"));
        assert!(header.ends_with("UPDATED"));

        let row1 = lines.next().unwrap();
        assert!(row1.contains("DATABASE_URL"));
        assert!(row1.contains("acme/prod"));
        assert!(row1.contains("about 2 hours ago"));

        let row2 = lines.next().unwrap();
        assert!(row2.contains("API_KEY"));
        assert!(row2.contains("acme/staging"));
        assert!(row2.contains("3 days ago"));

        // No dash separator row.
        assert!(!out.contains("----"));
        // No ANSI when use_color=false.
        assert!(!out.contains("\x1b["));

        // Columns are left-aligned with a 2-space gap.
        assert!(row1.contains("DATABASE_URL  acme/prod"));
    }
}
