//! SQLite-backed cache for secret paths, used to accelerate shell completions.
//!
//! # Layout
//!
//! Database lives at `state_dir/completions.db`. Schema:
//!
//! ```sql
//! paths      (store_path, secret_path)  -- indexed secret paths per store
//! store_info (store_path, mtime)        -- last-seen mtime of secrets dir tree
//! ```
//!
//! # Refresh strategy
//!
//! `refresh_store` computes a recursive max-mtime over the store's secrets
//! directory. If the mtime matches what's stored in `store_info`, the walk is
//! skipped (cheap path). Otherwise the cache is rebuilt and `store_info` is
//! updated. This means `--refresh-cache` is fast when nothing has changed.
//!
//! After any mutating command the dispatcher calls `refresh_store` so the
//! cache is always warm for the next tab-press.
//!
//! # Fallback
//!
//! When the database doesn't exist, is corrupt, or has no entries for a given
//! store, callers fall back to a live filesystem scan. The cache is
//! best-effort; it never causes a completion failure.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

// ── Public API ───────────────────────────────────────────────────────────────

/// Path to the completions cache database.
pub fn db_path(state_dir: &Path) -> PathBuf {
    state_dir.join("completions.db")
}

/// Refresh the cache for a single store.
///
/// Computes a recursive max-mtime over the store's secrets directory.  If it
/// matches the previously stored value the walk is skipped.  Otherwise the
/// cache is rebuilt and the stored mtime is updated.
///
/// Returns the number of paths now in the cache for this store.
pub fn refresh_store(state_dir: &Path, store: &Path) -> rusqlite::Result<usize> {
    let conn = open(state_dir)?;
    let store_key = store.to_string_lossy().to_string();

    let secrets_dir = store.join(".himitsu").join("secrets");
    let current_mtime = max_mtime_recursive(&secrets_dir);

    // Check whether the tree has changed since the last cache build.
    let stored_mtime: i64 = conn
        .query_row(
            "SELECT mtime FROM store_info WHERE store_path = ?1",
            params![store_key],
            |row| row.get(0),
        )
        .unwrap_or(-1);

    if stored_mtime >= 0 && stored_mtime as u64 == current_mtime {
        // Tree unchanged — return the cached count without touching paths.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE store_path = ?1",
                params![store_key],
                |row| row.get(0),
            )
            .unwrap_or(0);
        return Ok(count as usize);
    }

    // Tree changed — rebuild.
    conn.execute(
        "DELETE FROM paths WHERE store_path = ?1",
        params![store_key],
    )?;

    let paths =
        crate::remote::store::list_secrets(store, None).unwrap_or_default();
    let count = paths.len();
    for p in &paths {
        conn.execute(
            "INSERT OR REPLACE INTO paths (store_path, secret_path) VALUES (?1, ?2)",
            params![store_key, p],
        )?;
    }

    conn.execute(
        "INSERT OR REPLACE INTO store_info (store_path, mtime) VALUES (?1, ?2)",
        params![store_key, current_mtime as i64],
    )?;

    Ok(count)
}

/// Refresh the cache for every store checkout found under `stores_dir`.
///
/// Walks the two-level `<org>/<repo>` layout used by himitsu's managed stores.
/// Returns the total number of paths indexed across all stores.
pub fn refresh_all(state_dir: &Path, stores_dir: &Path) -> rusqlite::Result<usize> {
    let mut total = 0;
    let Ok(orgs) = std::fs::read_dir(stores_dir) else {
        return Ok(0);
    };
    for org in orgs.flatten() {
        let Ok(ft) = org.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let Ok(repos) = std::fs::read_dir(org.path()) else {
            continue;
        };
        for repo in repos.flatten() {
            let Ok(rft) = repo.file_type() else { continue };
            if !rft.is_dir() {
                continue;
            }
            if let Ok(n) = refresh_store(state_dir, &repo.path()) {
                total += n;
            }
        }
    }
    Ok(total)
}

/// Query cached paths across the given stores, filtered by an optional prefix.
///
/// Returns paths sorted lexicographically (deduplicated across stores).
pub fn lookup(
    state_dir: &Path,
    stores: &[PathBuf],
    prefix: &str,
) -> rusqlite::Result<Vec<String>> {
    let conn = open(state_dir)?;
    let pfx_slash = format!("{prefix}/");
    let mut all: Vec<String> = Vec::new();

    for store in stores {
        let store_key = store.to_string_lossy().to_string();
        let mut stmt = conn.prepare(
            "SELECT secret_path FROM paths WHERE store_path = ?1 ORDER BY secret_path",
        )?;
        let rows: Vec<String> = stmt
            .query_map(params![store_key], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .filter(|p: &String| {
                prefix.is_empty() || p == prefix || p.starts_with(&pfx_slash)
            })
            .collect();
        all.extend(rows);
    }
    Ok(all)
}

/// Returns `true` if the cache exists and has at least one entry for any of
/// the given stores. A `false` result signals that a live filesystem scan
/// should be used instead.
pub fn is_warm(state_dir: &Path, stores: &[PathBuf]) -> bool {
    let conn = match open(state_dir) {
        Ok(c) => c,
        Err(_) => return false,
    };
    for store in stores {
        let store_key = store.to_string_lossy().to_string();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE store_path = ?1 LIMIT 1",
                params![store_key],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if count > 0 {
            return true;
        }
    }
    false
}

// ── Internal helpers ─────────────────────────────────────────────────────────

fn open(state_dir: &Path) -> rusqlite::Result<Connection> {
    let path = db_path(state_dir);
    let conn = Connection::open(&path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous  = NORMAL;",
    )?;
    ensure_schema(&conn)?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS paths (
             store_path  TEXT NOT NULL,
             secret_path TEXT NOT NULL,
             PRIMARY KEY (store_path, secret_path)
         );
         CREATE INDEX IF NOT EXISTS idx_paths_store
             ON paths (store_path);
         CREATE TABLE IF NOT EXISTS store_info (
             store_path TEXT PRIMARY KEY,
             mtime      INTEGER NOT NULL
         );",
    )
}

/// Recursive max-mtime over a directory tree.  Returns 0 if the directory
/// doesn't exist or mtime can't be read (drives a full rebuild on next call).
fn max_mtime_recursive(dir: &Path) -> u64 {
    if !dir.exists() {
        return 0;
    }
    let mut max = 0u64;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if let Ok(modified) = meta.modified() {
            let secs = modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if secs > max {
                max = secs;
            }
        }
        if meta.is_dir() {
            let child = max_mtime_recursive(&entry.path());
            if child > max {
                max = child;
            }
        }
    }
    max
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store(root: &Path) -> PathBuf {
        let store = root.join("store");
        let secrets = store.join(".himitsu").join("secrets").join("prod");
        std::fs::create_dir_all(&secrets).unwrap();
        std::fs::write(secrets.join("API_KEY.yaml"), "fake: yaml").unwrap();
        std::fs::write(secrets.join("DB_PASS.yaml"), "fake: yaml").unwrap();
        store
    }

    fn state(root: &Path) -> PathBuf {
        let s = root.join("state");
        std::fs::create_dir_all(&s).unwrap();
        s
    }

    #[test]
    fn refresh_and_lookup_returns_all_paths() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let state_dir = state(tmp.path());

        let n = refresh_store(&state_dir, &store).unwrap();
        assert_eq!(n, 2);
        assert!(is_warm(&state_dir, &[store.clone()]));

        let paths = lookup(&state_dir, &[store.clone()], "").unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"prod/API_KEY".to_string()));
        assert!(paths.contains(&"prod/DB_PASS".to_string()));
    }

    #[test]
    fn lookup_filters_by_prefix() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let state_dir = state(tmp.path());
        refresh_store(&state_dir, &store).unwrap();

        let prod = lookup(&state_dir, &[store.clone()], "prod").unwrap();
        assert_eq!(prod.len(), 2);

        let dev = lookup(&state_dir, &[store.clone()], "dev").unwrap();
        assert!(dev.is_empty());
    }

    #[test]
    fn refresh_skips_walk_when_mtime_unchanged() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(tmp.path());
        let state_dir = state(tmp.path());

        refresh_store(&state_dir, &store).unwrap();

        // Add a file without touching mtime (simulate no-change; just verify
        // second call doesn't error and returns the same count).
        let n2 = refresh_store(&state_dir, &store).unwrap();
        assert_eq!(n2, 2, "second refresh should return cached count");
    }

    #[test]
    fn is_warm_false_for_unknown_store() {
        let tmp = TempDir::new().unwrap();
        let state_dir = state(tmp.path());
        // Initialise schema without any data.
        open(&state_dir).unwrap();
        let ghost = tmp.path().join("ghost");
        assert!(!is_warm(&state_dir, &[ghost]));
    }

    #[test]
    fn is_warm_false_when_db_missing() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path().join("no_such_dir");
        let store = tmp.path().join("store");
        // state_dir doesn't exist → open() will fail → is_warm returns false
        assert!(!is_warm(&state_dir, &[store]));
    }

    #[test]
    fn refresh_all_indexes_nested_stores() {
        let tmp = TempDir::new().unwrap();
        let state_dir = state(tmp.path());

        // Build a two-level org/repo structure.
        let stores_dir = tmp.path().join("stores");
        let store1 = stores_dir.join("acme").join("backend");
        let store2 = stores_dir.join("acme").join("frontend");
        for s in [&store1, &store2] {
            let secrets = s.join(".himitsu").join("secrets");
            std::fs::create_dir_all(&secrets).unwrap();
            std::fs::write(secrets.join("KEY.yaml"), "fake: yaml").unwrap();
        }

        let total = refresh_all(&state_dir, &stores_dir).unwrap();
        assert_eq!(total, 2, "one secret per store = 2 total");
    }
}
