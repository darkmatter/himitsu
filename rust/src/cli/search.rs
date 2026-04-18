use std::io::{self, IsTerminal};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::Args;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use owo_colors::OwoColorize;

use super::Context;
use crate::crypto::{age, secret_value};
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
    /// Human-readable description pulled from the secret's encrypted
    /// payload (`SecretValue.description`).
    ///
    /// Populated best-effort by [`search_core`]: it loads the ambient age
    /// identity once and attempts to decrypt every listed secret. Any
    /// failure (missing identity, decrypt error, legacy raw payload, empty
    /// description) falls back to `None` so search still completes when
    /// some secrets are unreadable by the current user.
    pub description: Option<String>,
}

/// Run a search across all known stores and return sorted results.
///
/// Pure IO-in/data-out: no printing. Shared by the CLI (`run`) and the TUI
/// search view so both see the same results.
pub fn search_core(ctx: &Context, query: &str) -> Result<Vec<SearchResult>> {
    let mut candidates: Vec<SearchResult> = Vec::new();

    // Try to load the age identity once so we can best-effort extract the
    // description from each secret's encrypted payload. If the identity
    // isn't available (fresh install, CI test fixture, missing key file)
    // we still return search results — just without descriptions.
    let identity = age::read_identity(&ctx.key_path()).ok();

    for (slug, store_path) in collect_stores(ctx)? {
        let paths = store::list_secrets(&store_path, None).unwrap_or_default();
        for path in paths {
            let meta = store::read_secret_meta(&store_path, &path).ok();
            let (created_at, updated_at) = match meta {
                Some(m) => (m.created_at, m.lastmodified),
                None => (None, None),
            };
            let description = identity
                .as_ref()
                .and_then(|id| read_description(&store_path, &path, id));
            candidates.push(SearchResult {
                store: slug.clone(),
                store_path: store_path.clone(),
                path,
                created_at,
                updated_at,
                description,
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

/// Best-effort read of the `description` field from a secret's encrypted
/// payload. Returns `None` on any failure (read error, decrypt error, legacy
/// raw-bytes payload with no structured metadata, empty description).
///
/// Used by [`search_core`] to populate [`SearchResult::description`] without
/// aborting the whole search when some secrets can't be decrypted (e.g. the
/// current identity isn't on the recipient list for them).
fn read_description(
    store_path: &std::path::Path,
    secret_path: &str,
    identity: &::age::x25519::Identity,
) -> Option<String> {
    let ciphertext = store::read_secret(store_path, secret_path).ok()?;
    let plain = age::decrypt(&ciphertext, identity).ok()?;
    let decoded = secret_value::decode(&plain);
    if decoded.description.is_empty() {
        None
    } else {
        Some(decoded.description)
    }
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
    // JSON mode emits raw RFC3339 timestamps (not humanized strings) so
    // machine consumers get absolute times they can re-render in any
    // locale / timezone. Description is included alongside so the TUI
    // and other scripts can show it without re-decrypting.
    let items: Vec<_> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "store":       r.store,
                "path":        r.path,
                "created_at":  r.created_at,
                "updated_at":  r.updated_at,
                "description": r.description,
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
///
/// Column order: `PATH | UPDATED | DESCRIPTION | STORE`. `STORE` sits last
/// because the primary identifier is the path and most users scan by it;
/// the store slug is secondary context. The UPDATED column falls back to
/// `created_at` when `updated_at` is absent and renders an em dash ("—")
/// when neither exists.
fn render_table(results: &[SearchResult], use_color: bool, now: DateTime<Utc>) -> String {
    let rows: Vec<(String, String, String, String)> = results
        .iter()
        .map(|r| {
            let ts = r.updated_at.as_deref().or(r.created_at.as_deref());
            let updated = ts
                .and_then(parse_ts)
                .map(|t| humanize_age(now, t))
                .unwrap_or_else(|| "—".to_string());
            let description = r.description.clone().unwrap_or_default();
            (r.path.clone(), updated, description, r.store.clone())
        })
        .collect();

    let path_w = rows
        .iter()
        .map(|(p, _, _, _)| p.len())
        .max()
        .unwrap_or(0)
        .max("PATH".len());
    let updated_w = rows
        .iter()
        .map(|(_, u, _, _)| u.len())
        .max()
        .unwrap_or(0)
        .max("UPDATED".len());
    let desc_w = rows
        .iter()
        .map(|(_, _, d, _)| d.len())
        .max()
        .unwrap_or(0)
        .max("DESCRIPTION".len());

    let mut buf = String::new();
    buf.push_str(&format!(
        "{:<path_w$}  {:<updated_w$}  {:<desc_w$}  STORE\n",
        "PATH", "UPDATED", "DESCRIPTION",
    ));

    for (path, updated, description, store) in rows {
        let path_cell = format!("{path:<path_w$}");
        let updated_cell = format!("{updated:<updated_w$}");
        let desc_cell = format!("{description:<desc_w$}");
        if use_color {
            buf.push_str(&format!(
                "{}  {}  {}  {}\n",
                path_cell.cyan(),
                updated_cell.dimmed(),
                desc_cell,
                store,
            ));
        } else {
            buf.push_str(&format!(
                "{path_cell}  {updated_cell}  {desc_cell}  {store}\n"
            ));
        }
    }

    buf
}

/// Parse an ISO 8601 / RFC3339 timestamp or a plain `YYYY-MM-DD` date into
/// a UTC `DateTime`. Returns `None` if neither format matches.
pub(crate) fn parse_ts(ts: &str) -> Option<DateTime<Utc>> {
    if let Ok(d) = DateTime::parse_from_rfc3339(ts) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(ts, "%Y-%m-%d") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(
            d.and_hms_opt(0, 0, 0)?,
            Utc,
        ));
    }
    None
}

/// Render the time between `ts` and `now` as a short human-readable age.
///
/// Picks the largest unit that gives an integer ≥ 1: "just now",
/// "n minutes ago", "n hours ago", "n days ago", "n months ago",
/// "n years ago". Future timestamps (ts > now) and anything under a
/// minute both render as "just now" — we don't distinguish drift from
/// freshness. Months use a 30-day approximation and years use 365; this
/// matches every other "time ago" formatter in the wild and is good
/// enough for a list view.
pub(crate) fn humanize_age(now: DateTime<Utc>, ts: DateTime<Utc>) -> String {
    let delta = now.signed_duration_since(ts);
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
        return format!("{hours} hour{} ago", plural(hours));
    }
    let days = delta.num_days();
    if days < 30 {
        return format!("{days} day{} ago", plural(days));
    }
    // Only roll up to years once we've accumulated a full 365 days —
    // otherwise a 364-day-old secret would render as "0 years ago".
    if days < 365 {
        let months = days / 30;
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
            description: None,
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
    fn test_humanize_age_buckets() {
        let now = Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();

        // Under a minute collapses to "just now", even at the boundary.
        assert_eq!(
            humanize_age(now, now - chrono::Duration::seconds(10)),
            "just now"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::seconds(59)),
            "just now"
        );

        // Future timestamps (clock drift / wrong-TZ bugs) also get "just now".
        assert_eq!(
            humanize_age(now, now + chrono::Duration::minutes(3)),
            "just now"
        );

        // Minutes: singular vs plural + largest-unit-wins.
        assert_eq!(
            humanize_age(now, now - chrono::Duration::minutes(1)),
            "1 minute ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::minutes(5)),
            "5 minutes ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::minutes(59)),
            "59 minutes ago"
        );

        // Hours.
        assert_eq!(
            humanize_age(now, now - chrono::Duration::hours(1)),
            "1 hour ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::hours(2)),
            "2 hours ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::hours(23)),
            "23 hours ago"
        );

        // Days.
        assert_eq!(
            humanize_age(now, now - chrono::Duration::days(1)),
            "1 day ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::days(3)),
            "3 days ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::days(29)),
            "29 days ago"
        );

        // Months (30-day approximation).
        assert_eq!(
            humanize_age(now, now - chrono::Duration::days(30)),
            "1 month ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::days(90)),
            "3 months ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::days(364)),
            "12 months ago"
        );

        // Years (365-day approximation).
        assert_eq!(
            humanize_age(now, now - chrono::Duration::days(365)),
            "1 year ago"
        );
        assert_eq!(
            humanize_age(now, now - chrono::Duration::days(365 * 2 + 5)),
            "2 years ago"
        );
    }

    #[test]
    fn test_parse_ts_accepts_rfc3339_and_date_only() {
        let t = parse_ts("2026-04-15T12:00:00Z").unwrap();
        assert_eq!(t.format("%Y-%m-%d").to_string(), "2026-04-15");

        let t = parse_ts("2026-04-15").unwrap();
        assert_eq!(t.format("%Y-%m-%d").to_string(), "2026-04-15");

        assert!(parse_ts("not a date").is_none());
    }

    #[test]
    fn test_gh_style_table_output() {
        let now = Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();
        let two_hours_ago = (now - chrono::Duration::hours(2)).to_rfc3339();
        let three_days_ago = (now - chrono::Duration::days(3)).to_rfc3339();

        let results = vec![
            SearchResult {
                store: "acme/prod".into(),
                store_path: PathBuf::new(),
                path: "DATABASE_URL".into(),
                created_at: None,
                updated_at: Some(two_hours_ago),
                description: Some("prod postgres primary".into()),
            },
            SearchResult {
                store: "acme/staging".into(),
                store_path: PathBuf::new(),
                path: "API_KEY".into(),
                created_at: None,
                updated_at: Some(three_days_ago),
                description: None,
            },
        ];

        let out = render_table(&results, false, now);
        let mut lines = out.lines();

        // Header order: PATH | UPDATED | DESCRIPTION | STORE.
        let header = lines.next().unwrap();
        assert!(header.starts_with("PATH"), "header: {header}");
        assert!(header.contains("UPDATED"), "header: {header}");
        assert!(header.contains("DESCRIPTION"), "header: {header}");
        assert!(header.trim_end().ends_with("STORE"), "header: {header}");

        // PATH column comes before UPDATED which comes before DESCRIPTION
        // which comes before STORE.
        let p = header.find("PATH").unwrap();
        let u = header.find("UPDATED").unwrap();
        let d = header.find("DESCRIPTION").unwrap();
        let s = header.find("STORE").unwrap();
        assert!(p < u && u < d && d < s, "column order wrong: {header}");

        let row1 = lines.next().unwrap();
        assert!(row1.contains("DATABASE_URL"));
        assert!(row1.contains("2 hours ago"));
        assert!(row1.contains("prod postgres primary"));
        assert!(row1.trim_end().ends_with("acme/prod"));

        let row2 = lines.next().unwrap();
        assert!(row2.contains("API_KEY"));
        assert!(row2.contains("3 days ago"));
        assert!(row2.trim_end().ends_with("acme/staging"));

        // No dash separator row.
        assert!(!out.contains("----"));
        // No ANSI when use_color=false.
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn test_render_table_falls_back_to_em_dash_when_no_timestamps() {
        let now = Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();
        let results = vec![SearchResult {
            store: "acme/prod".into(),
            store_path: PathBuf::new(),
            path: "ORPHAN".into(),
            created_at: None,
            updated_at: None,
            description: None,
        }];

        let out = render_table(&results, false, now);
        let row = out.lines().nth(1).unwrap();
        assert!(row.contains("—"), "expected em-dash fallback in: {row}");
    }

    #[test]
    fn test_render_table_prefers_updated_then_created_at() {
        let now = Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap();
        let results = vec![SearchResult {
            store: "acme/prod".into(),
            store_path: PathBuf::new(),
            path: "LEGACY".into(),
            created_at: Some("2026-04-12".into()), // 3 days ago
            updated_at: None,
            description: None,
        }];

        let out = render_table(&results, false, now);
        assert!(out.contains("3 days ago"), "output: {out}");
    }
}
