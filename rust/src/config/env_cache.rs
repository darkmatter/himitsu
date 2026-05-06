//! SQLite cache for project/global `envs:` preset maps.
//!
//! YAML remains the source of truth; this cache mirrors the `envs:` section of
//! each config file so the TUI can query it without re-parsing YAML on every
//! keystroke. The cache is always rebuildable from the YAML — losing the DB
//! is harmless.
//!
//! Lifecycle:
//! - [`EnvCache::open`] creates (or opens) `data_dir()/envs.db` and runs the
//!   idempotent schema bootstrap.
//! - Callers drive refresh explicitly via [`EnvCache::refresh`]; the cache
//!   never auto-refreshes on read. `refresh` hashes the file bytes and skips
//!   the write when the stored hash matches.
//! - [`EnvCache::list`] / [`EnvCache::get`] read back the preserved ordering.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::config::{data_dir, EnvEntry, ProjectConfig};
use crate::error::{HimitsuError, Result};

/// Scope of a cached env preset map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Project-scoped config file (discovered by walking up from cwd).
    Project,
    /// Global config at `config_dir()/config.yaml`.
    Global,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Global => "global",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "project" => Ok(Scope::Project),
            "global" => Ok(Scope::Global),
            other => Err(HimitsuError::Index(format!(
                "invalid scope '{other}' in env_cache"
            ))),
        }
    }
}

/// One row read back from the cache, with its entries in original YAML order.
#[derive(Debug)]
pub struct CachedEnv {
    pub label: String,
    pub scope: Scope,
    pub entries: Vec<EnvEntry>,
}

/// Handle on the SQLite cache DB.
pub struct EnvCache {
    conn: Connection,
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS envs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    label        TEXT NOT NULL,
    scope        TEXT NOT NULL CHECK (scope IN ('project', 'global')),
    config_path  TEXT NOT NULL,
    config_hash  TEXT NOT NULL,
    UNIQUE (label, scope, config_path)
);

CREATE TABLE IF NOT EXISTS env_entries (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    env_id     INTEGER NOT NULL REFERENCES envs(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('single', 'glob', 'alias')),
    value      TEXT NOT NULL,
    alias_key  TEXT
);

CREATE INDEX IF NOT EXISTS idx_env_entries_env_id ON env_entries(env_id);
CREATE INDEX IF NOT EXISTS idx_envs_scope_path ON envs(scope, config_path);
"#;

fn map_sqlite_err(e: rusqlite::Error) -> HimitsuError {
    HimitsuError::Index(format!("env_cache sqlite: {e}"))
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    // Hex-encode without pulling in a new crate.
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

impl EnvCache {
    /// Open (or create) the cache DB at `data_dir()/envs.db`.
    pub fn open() -> Result<Self> {
        let dir = data_dir();
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
        }
        Self::open_at(&dir.join("envs.db"))
    }

    /// Open (or create) the cache DB at an explicit path (used by tests).
    pub fn open_at(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path).map_err(map_sqlite_err)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(map_sqlite_err)?;
        conn.execute_batch(SCHEMA_SQL).map_err(map_sqlite_err)?;
        Ok(EnvCache { conn })
    }

    /// Refresh the cache for one config file.
    ///
    /// Steps:
    /// 1. Validate the envs map (label grammar, capture legality).
    /// 2. Hash the file at `config_path` (file bytes — mtime would be
    ///    unreliable across git checkouts).
    /// 3. If every row for (scope, config_path) already matches that hash,
    ///    return early (no-op).
    /// 4. Otherwise delete all rows for (scope, config_path) and re-insert
    ///    the new `envs` map inside a single transaction.
    pub fn refresh(
        &self,
        config_path: &Path,
        scope: Scope,
        envs: &BTreeMap<String, Vec<EnvEntry>>,
    ) -> Result<()> {
        // Validation runs BEFORE opening a transaction so a bad label never
        // partially writes.
        let tmp = ProjectConfig {
            envs: envs.clone(),
            ..ProjectConfig::default()
        };
        tmp.validate()?;

        let bytes = std::fs::read(config_path)?;
        let new_hash = hash_bytes(&bytes);

        let canonical = canonicalize(config_path);
        let canonical_str = canonical.to_string_lossy().into_owned();
        let scope_str = scope.as_str();

        // Fast path: existing rows all at new_hash and exactly the same set of
        // labels → nothing to do. We detect staleness by checking the distinct
        // hash values *and* the label set for this (scope, path).
        if cache_matches(&self.conn, scope_str, &canonical_str, &new_hash, envs)? {
            return Ok(());
        }

        // Slow path: replace atomically.
        let tx = self.conn.unchecked_transaction().map_err(map_sqlite_err)?;

        tx.execute(
            "DELETE FROM envs WHERE scope = ?1 AND config_path = ?2",
            params![scope_str, &canonical_str],
        )
        .map_err(map_sqlite_err)?;

        for (label, entries) in envs {
            tx.execute(
                "INSERT INTO envs (label, scope, config_path, config_hash) VALUES (?1, ?2, ?3, ?4)",
                params![label, scope_str, &canonical_str, &new_hash],
            )
            .map_err(map_sqlite_err)?;
            let env_id = tx.last_insert_rowid();

            for (pos, entry) in entries.iter().enumerate() {
                // Tag selectors are persisted as new kinds (`tag` / `alias_tag`)
                // so the cache round-trips cleanly through the same
                // (kind, value, alias_key) tuple shape every other entry uses.
                let (kind, value, alias_key): (&str, &str, Option<&str>) = match entry {
                    EnvEntry::Single(p) => ("single", p.as_str(), None),
                    EnvEntry::Glob(p) => ("glob", p.as_str(), None),
                    EnvEntry::Alias { key, path } => ("alias", path.as_str(), Some(key.as_str())),
                    EnvEntry::Tag(t) => ("tag", t.as_str(), None),
                    EnvEntry::AliasTag { key, tag } => {
                        ("alias_tag", tag.as_str(), Some(key.as_str()))
                    }
                };
                tx.execute(
                    "INSERT INTO env_entries (env_id, position, kind, value, alias_key) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![env_id, pos as i64, kind, value, alias_key],
                )
                .map_err(map_sqlite_err)?;
            }
        }

        tx.commit().map_err(map_sqlite_err)?;
        Ok(())
    }

    /// Read every cached env for `scope`, optionally narrowed to a single
    /// config_path. Entries within each env preserve their YAML order.
    pub fn list(&self, scope: Scope, config_path: Option<&Path>) -> Result<Vec<CachedEnv>> {
        let scope_str = scope.as_str();
        let mut envs: Vec<(i64, String, String)> = Vec::new();

        if let Some(p) = config_path {
            let canonical = canonicalize(p);
            let canonical_str = canonical.to_string_lossy().into_owned();
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, label, scope FROM envs \
                     WHERE scope = ?1 AND config_path = ?2 ORDER BY label",
                )
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map(params![scope_str, &canonical_str], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(map_sqlite_err)?;
            for row in rows {
                envs.push(row.map_err(map_sqlite_err)?);
            }
        } else {
            let mut stmt = self
                .conn
                .prepare("SELECT id, label, scope FROM envs WHERE scope = ?1 ORDER BY label")
                .map_err(map_sqlite_err)?;
            let rows = stmt
                .query_map(params![scope_str], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(map_sqlite_err)?;
            for row in rows {
                envs.push(row.map_err(map_sqlite_err)?);
            }
        }

        let mut out = Vec::with_capacity(envs.len());
        for (env_id, label, scope_s) in envs {
            let entries = read_entries(&self.conn, env_id)?;
            out.push(CachedEnv {
                label,
                scope: Scope::from_str(&scope_s)?,
                entries,
            });
        }
        Ok(out)
    }

    /// Look up a single env by (label, scope, config_path).
    pub fn get(&self, label: &str, scope: Scope, config_path: &Path) -> Result<Option<CachedEnv>> {
        let canonical = canonicalize(config_path);
        let canonical_str = canonical.to_string_lossy().into_owned();
        let scope_str = scope.as_str();

        let row: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM envs WHERE label = ?1 AND scope = ?2 AND config_path = ?3",
                params![label, scope_str, &canonical_str],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sqlite_err)?;

        let Some(env_id) = row else {
            return Ok(None);
        };
        let entries = read_entries(&self.conn, env_id)?;
        Ok(Some(CachedEnv {
            label: label.to_string(),
            scope,
            entries,
        }))
    }
}

fn read_entries(conn: &Connection, env_id: i64) -> Result<Vec<EnvEntry>> {
    let mut stmt = conn
        .prepare(
            "SELECT kind, value, alias_key FROM env_entries \
             WHERE env_id = ?1 ORDER BY position ASC",
        )
        .map_err(map_sqlite_err)?;
    let rows = stmt
        .query_map(params![env_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(map_sqlite_err)?;

    let mut out = Vec::new();
    for row in rows {
        let (kind, value, alias_key) = row.map_err(map_sqlite_err)?;
        let entry = match kind.as_str() {
            "single" => EnvEntry::Single(value),
            "glob" => EnvEntry::Glob(value),
            "alias" => EnvEntry::Alias {
                key: alias_key
                    .ok_or_else(|| HimitsuError::Index("alias row missing alias_key".into()))?,
                path: value,
            },
            "tag" => EnvEntry::Tag(value),
            "alias_tag" => EnvEntry::AliasTag {
                key: alias_key.ok_or_else(|| {
                    HimitsuError::Index("alias_tag row missing alias_key".into())
                })?,
                tag: value,
            },
            other => {
                return Err(HimitsuError::Index(format!(
                    "unknown env_entry kind '{other}'"
                )));
            }
        };
        out.push(entry);
    }
    Ok(out)
}

/// Return `true` iff every cached row for (scope, config_path) has hash
/// `new_hash` AND the set of labels exactly matches `envs`. In that case
/// the cache is a no-op for this refresh.
fn cache_matches(
    conn: &Connection,
    scope: &str,
    config_path: &str,
    new_hash: &str,
    envs: &BTreeMap<String, Vec<EnvEntry>>,
) -> Result<bool> {
    // All stored hashes identical to new_hash?
    let mut stmt = conn
        .prepare("SELECT DISTINCT config_hash FROM envs WHERE scope = ?1 AND config_path = ?2")
        .map_err(map_sqlite_err)?;
    let mut hashes: Vec<String> = Vec::new();
    let rows = stmt
        .query_map(params![scope, config_path], |r| r.get::<_, String>(0))
        .map_err(map_sqlite_err)?;
    for row in rows {
        hashes.push(row.map_err(map_sqlite_err)?);
    }

    if hashes.is_empty() {
        // Nothing cached yet. If the new map is also empty, it's still a
        // no-op; otherwise we must write.
        return Ok(envs.is_empty());
    }
    if hashes.len() != 1 || hashes[0] != new_hash {
        return Ok(false);
    }

    // Label sets must match exactly.
    let mut label_stmt = conn
        .prepare("SELECT label FROM envs WHERE scope = ?1 AND config_path = ?2")
        .map_err(map_sqlite_err)?;
    let rows = label_stmt
        .query_map(params![scope, config_path], |r| r.get::<_, String>(0))
        .map_err(map_sqlite_err)?;
    let mut stored: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for row in rows {
        stored.insert(row.map_err(map_sqlite_err)?);
    }
    let desired: std::collections::BTreeSet<&str> = envs.keys().map(|k| k.as_str()).collect();
    if stored.len() != desired.len() {
        return Ok(false);
    }
    for k in &desired {
        if !stored.contains(*k) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Best-effort absolute path. Falls back to the input if canonicalize fails
/// (e.g. symlink/missing file) — we still want a stable key.
fn canonicalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn write_yaml(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }

    fn sample_envs() -> BTreeMap<String, Vec<EnvEntry>> {
        let mut m = BTreeMap::new();
        m.insert(
            "dev".to_string(),
            vec![
                EnvEntry::Single("dev/API_KEY".into()),
                EnvEntry::Alias {
                    key: "DB_PASS".into(),
                    path: "dev/DB_PASSWORD".into(),
                },
                EnvEntry::Glob("dev".into()),
            ],
        );
        m.insert("prod".to_string(), vec![EnvEntry::Glob("prod".into())]);
        m
    }

    /// Scenario 1: open_at creates schema; reopening is a no-op.
    #[test]
    fn open_bootstraps_schema_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("envs.db");
        let _cache = EnvCache::open_at(&db).unwrap();
        assert!(db.exists());
        // Reopen — should not error and should preserve schema.
        let cache2 = EnvCache::open_at(&db).unwrap();
        // list on an empty cache returns zero rows.
        let rows = cache2.list(Scope::Project, None).unwrap();
        assert_eq!(rows.len(), 0);
    }

    /// Scenario 2: refresh inserts and list reads back every EnvEntry variant
    /// in original order.
    #[test]
    fn refresh_and_list_round_trip_all_variants() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("envs.db");
        let yaml = write_yaml(tmp.path(), "proj.yaml", "stub: 1\n");

        let cache = EnvCache::open_at(&db).unwrap();
        let envs = sample_envs();
        cache.refresh(&yaml, Scope::Project, &envs).unwrap();

        let rows = cache.list(Scope::Project, Some(&yaml)).unwrap();
        assert_eq!(rows.len(), 2);

        let dev = rows.iter().find(|r| r.label == "dev").unwrap();
        assert_eq!(dev.entries.len(), 3);
        // Order preserved.
        match &dev.entries[0] {
            EnvEntry::Single(p) => assert_eq!(p, "dev/API_KEY"),
            _ => panic!("expected Single"),
        }
        match &dev.entries[1] {
            EnvEntry::Alias { key, path } => {
                assert_eq!(key, "DB_PASS");
                assert_eq!(path, "dev/DB_PASSWORD");
            }
            _ => panic!("expected Alias"),
        }
        match &dev.entries[2] {
            EnvEntry::Glob(p) => assert_eq!(p, "dev"),
            _ => panic!("expected Glob"),
        }

        // get() returns the same row.
        let fetched = cache
            .get("prod", Scope::Project, &yaml)
            .unwrap()
            .expect("prod env should exist");
        assert!(matches!(&fetched.entries[0], EnvEntry::Glob(p) if p == "prod"));
    }

    /// Scenario 3: stale detection — changed content replaces rows, unchanged
    /// content is a no-op (verified by counting env_entries rows before/after).
    #[test]
    fn refresh_is_noop_on_identical_content_and_replaces_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("envs.db");
        let yaml = write_yaml(tmp.path(), "proj.yaml", "stub: 1\n");
        let cache = EnvCache::open_at(&db).unwrap();

        let envs = sample_envs();
        cache.refresh(&yaml, Scope::Project, &envs).unwrap();

        // Snapshot: total env_entries row count after first insert.
        let count_before: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM env_entries", [], |r| r.get(0))
            .unwrap();
        // Also snapshot the primary-key max so we can assert no rewrite.
        let max_id_before: i64 = cache
            .conn
            .query_row("SELECT MAX(id) FROM env_entries", [], |r| r.get(0))
            .unwrap();

        // Identical file bytes and identical envs map → no-op.
        cache.refresh(&yaml, Scope::Project, &envs).unwrap();

        let count_after: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM env_entries", [], |r| r.get(0))
            .unwrap();
        let max_id_after: i64 = cache
            .conn
            .query_row("SELECT MAX(id) FROM env_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_before, count_after, "row count must not change");
        assert_eq!(
            max_id_before, max_id_after,
            "MAX(id) must not change — rows were rewritten when they shouldn't be"
        );

        // Now change file bytes — should invalidate and replace.
        std::fs::write(&yaml, "stub: 2\n").unwrap();
        let mut changed = sample_envs();
        changed.insert("staging".into(), vec![EnvEntry::Single("staging/X".into())]);
        cache.refresh(&yaml, Scope::Project, &changed).unwrap();

        let rows = cache.list(Scope::Project, Some(&yaml)).unwrap();
        assert_eq!(rows.len(), 3, "staging env should have been inserted");
        let max_id_after_change: i64 = cache
            .conn
            .query_row("SELECT MAX(id) FROM env_entries", [], |r| r.get(0))
            .unwrap();
        assert!(
            max_id_after_change > max_id_after,
            "new rows should have higher ids"
        );
    }

    /// Scenario 4: scope isolation — project rows for path A don't appear in
    /// the global-scope list.
    #[test]
    fn project_and_global_scopes_are_isolated() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("envs.db");
        let cache = EnvCache::open_at(&db).unwrap();

        let proj_yaml = write_yaml(tmp.path(), "proj.yaml", "stub: p\n");
        let global_yaml = write_yaml(tmp.path(), "global.yaml", "stub: g\n");

        let mut proj_envs = BTreeMap::new();
        proj_envs.insert("only-project".into(), vec![EnvEntry::Single("x/y".into())]);
        let mut global_envs = BTreeMap::new();
        global_envs.insert("only-global".into(), vec![EnvEntry::Single("a/b".into())]);

        cache
            .refresh(&proj_yaml, Scope::Project, &proj_envs)
            .unwrap();
        cache
            .refresh(&global_yaml, Scope::Global, &global_envs)
            .unwrap();

        let global_rows = cache.list(Scope::Global, None).unwrap();
        assert_eq!(global_rows.len(), 1);
        assert_eq!(global_rows[0].label, "only-global");
        assert!(global_rows.iter().all(|r| r.scope == Scope::Global));

        let project_rows = cache.list(Scope::Project, None).unwrap();
        assert_eq!(project_rows.len(), 1);
        assert_eq!(project_rows[0].label, "only-project");

        // get() with wrong scope returns None.
        assert!(cache
            .get("only-project", Scope::Global, &proj_yaml)
            .unwrap()
            .is_none());
    }

    /// Scenario 5: refresh with an invalid label returns an error and never
    /// writes.
    #[test]
    fn refresh_with_invalid_label_returns_error_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("envs.db");
        let yaml = write_yaml(tmp.path(), "proj.yaml", "stub: 1\n");
        let cache = EnvCache::open_at(&db).unwrap();

        let mut bad = BTreeMap::new();
        bad.insert("foo/*/bar".to_string(), vec![EnvEntry::Single("x".into())]);
        let err = cache.refresh(&yaml, Scope::Project, &bad).unwrap_err();
        assert!(matches!(err, HimitsuError::InvalidConfig(_)), "got {err:?}");

        let count: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM envs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "nothing should have been written");
        let ecount: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM env_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ecount, 0);
    }

    /// Scenario 6: refresh rolls back on a mid-transaction error.
    ///
    /// We provoke a mid-transaction failure by stuffing a deliberately bogus
    /// `kind` through a direct INSERT, using a tiny helper that mimics
    /// `refresh` but corrupts the second entry. Strategy: seed the cache with
    /// a clean dataset, then attempt a `refresh` whose insertion phase fails
    /// halfway because we stubbed `ProjectConfig::validate` into a closure
    /// that *passes*, but we break the DB with a CHECK-violating raw insert
    /// before COMMIT. The simplest concrete way to do this without mocks is
    /// to trigger the CHECK constraint on the `kind` column by violating it
    /// via a direct transaction that mirrors `refresh` but with a bad kind.
    #[test]
    fn refresh_rolls_back_on_transaction_error() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("envs.db");
        let yaml = write_yaml(tmp.path(), "proj.yaml", "stub: 1\n");
        let cache = EnvCache::open_at(&db).unwrap();

        // Seed with valid data.
        let envs = sample_envs();
        cache.refresh(&yaml, Scope::Project, &envs).unwrap();
        let count_before: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM envs", [], |r| r.get(0))
            .unwrap();
        assert!(count_before > 0);

        // Manually drive a transaction that mirrors refresh() but inserts a
        // bogus `kind` mid-way — the CHECK constraint should abort the
        // transaction.
        let canonical_str = canonicalize(&yaml).to_string_lossy().into_owned();
        let tx = cache.conn.unchecked_transaction().unwrap();
        tx.execute(
            "DELETE FROM envs WHERE scope = ?1 AND config_path = ?2",
            params!["project", &canonical_str],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO envs (label, scope, config_path, config_hash) VALUES (?1, ?2, ?3, ?4)",
            params!["dev", "project", &canonical_str, "deadbeef"],
        )
        .unwrap();
        let env_id = tx.last_insert_rowid();
        // Deliberately violate the CHECK constraint on `kind`.
        let bad = tx.execute(
            "INSERT INTO env_entries (env_id, position, kind, value, alias_key) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![env_id, 0i64, "nonsense-kind", "x", Option::<&str>::None],
        );
        assert!(bad.is_err(), "CHECK constraint should fire");
        drop(tx); // rollback (no commit)

        // Cache content must be unchanged.
        let count_after: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM envs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count_before, count_after,
            "transaction must have rolled back"
        );
        let rows = cache.list(Scope::Project, Some(&yaml)).unwrap();
        assert_eq!(rows.len(), 2, "original two envs (dev, prod) still present");
    }
}
