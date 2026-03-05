use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;

const SCHEMA: &str = include_str!("schema.sql");

/// Search result from the secret index.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub remote_id: String,
    pub env: String,
    pub key_name: String,
    pub path: String,
}

/// SQLite-backed secret index for cross-remote search.
pub struct SecretIndex {
    conn: Connection,
}

impl SecretIndex {
    /// Open (or create) the index database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(SecretIndex { conn })
    }

    /// Open an in-memory database (for testing).
    #[cfg(test)]
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(SecretIndex { conn })
    }

    /// Register a remote in the index.
    pub fn register_remote(&self, remote_id: &str, url: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO remotes (id, url, synced_at) VALUES (?1, ?2, datetime('now'))",
            rusqlite::params![remote_id, url],
        )?;
        Ok(())
    }

    /// Insert or update a secret entry.
    pub fn upsert(&self, remote_id: &str, env: &str, path: &str, key_name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO secrets (remote_id, env, path, key_name, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))
             ON CONFLICT(remote_id, path) DO UPDATE SET
               env = excluded.env,
               key_name = excluded.key_name,
               updated_at = excluded.updated_at",
            rusqlite::params![remote_id, env, path, key_name],
        )?;
        Ok(())
    }

    /// Search for secrets matching a query (partial key name match).
    /// If remote_filter is Some, only search within that remote.
    pub fn search(&self, query: &str, remote_filter: Option<&str>) -> Result<Vec<SearchResult>> {
        let pattern = format!("%{query}%");
        let mut results = vec![];

        if let Some(remote_id) = remote_filter {
            let mut stmt = self.conn.prepare(
                "SELECT remote_id, env, key_name, path FROM secrets
                 WHERE key_name LIKE ?1 AND remote_id = ?2
                 ORDER BY remote_id, env, key_name",
            )?;
            let rows = stmt.query_map(rusqlite::params![pattern, remote_id], |row| {
                Ok(SearchResult {
                    remote_id: row.get(0)?,
                    env: row.get(1)?,
                    key_name: row.get(2)?,
                    path: row.get(3)?,
                })
            })?;
            for row in rows {
                results.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT remote_id, env, key_name, path FROM secrets
                 WHERE key_name LIKE ?1
                 ORDER BY remote_id, env, key_name",
            )?;
            let rows = stmt.query_map(rusqlite::params![pattern], |row| {
                Ok(SearchResult {
                    remote_id: row.get(0)?,
                    env: row.get(1)?,
                    key_name: row.get(2)?,
                    path: row.get(3)?,
                })
            })?;
            for row in rows {
                results.push(row?);
            }
        }

        Ok(results)
    }

    /// Remove all secrets for a given remote (used before re-indexing).
    pub fn clear_remote(&self, remote_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM secrets WHERE remote_id = ?1",
            rusqlite::params![remote_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_inserts_new_entry() {
        let idx = SecretIndex::open_memory().unwrap();
        idx.register_remote("org/repo", None).unwrap();
        idx.upsert("org/repo", "prod", "vars/prod/API_KEY.age", "API_KEY")
            .unwrap();
        let results = idx.search("API_KEY", None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key_name, "API_KEY");
    }

    #[test]
    fn upsert_updates_existing() {
        let idx = SecretIndex::open_memory().unwrap();
        idx.register_remote("org/repo", None).unwrap();
        idx.upsert("org/repo", "prod", "vars/prod/API_KEY.age", "API_KEY")
            .unwrap();
        idx.upsert("org/repo", "prod", "vars/prod/API_KEY.age", "API_KEY")
            .unwrap();
        let results = idx.search("API_KEY", None).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_matches_partial_key_names() {
        let idx = SecretIndex::open_memory().unwrap();
        idx.register_remote("org/repo", None).unwrap();
        idx.upsert("org/repo", "prod", "vars/prod/STRIPE_KEY.age", "STRIPE_KEY")
            .unwrap();
        idx.upsert(
            "org/repo",
            "prod",
            "vars/prod/STRIPE_SECRET.age",
            "STRIPE_SECRET",
        )
        .unwrap();
        idx.upsert("org/repo", "prod", "vars/prod/DB_PASS.age", "DB_PASS")
            .unwrap();
        let results = idx.search("STRIPE", None).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_across_multiple_remotes() {
        let idx = SecretIndex::open_memory().unwrap();
        idx.register_remote("org/repo1", None).unwrap();
        idx.register_remote("org/repo2", None).unwrap();
        idx.upsert("org/repo1", "prod", "vars/prod/API_KEY.age", "API_KEY")
            .unwrap();
        idx.upsert("org/repo2", "dev", "vars/dev/API_KEY.age", "API_KEY")
            .unwrap();
        let results = idx.search("API_KEY", None).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_filtered_by_remote() {
        let idx = SecretIndex::open_memory().unwrap();
        idx.register_remote("org/repo1", None).unwrap();
        idx.register_remote("org/repo2", None).unwrap();
        idx.upsert("org/repo1", "prod", "vars/prod/KEY.age", "KEY")
            .unwrap();
        idx.upsert("org/repo2", "prod", "vars/prod/KEY.age", "KEY")
            .unwrap();
        let results = idx.search("KEY", Some("org/repo1")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].remote_id, "org/repo1");
    }
}
