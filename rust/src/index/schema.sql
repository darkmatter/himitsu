CREATE TABLE IF NOT EXISTS remotes (
    id        TEXT PRIMARY KEY,   -- store path or "org/repo" slug
    url       TEXT,
    synced_at TEXT                 -- ISO 8601
);

CREATE TABLE IF NOT EXISTS secrets (
    id          INTEGER PRIMARY KEY,
    remote_id   TEXT NOT NULL REFERENCES remotes(id),
    secret_path TEXT NOT NULL,     -- e.g. "prod/STRIPE_KEY"
    updated_at  TEXT,
    UNIQUE(remote_id, secret_path)
);
