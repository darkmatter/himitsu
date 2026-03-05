CREATE TABLE IF NOT EXISTS remotes (
    id        TEXT PRIMARY KEY,   -- "org/repo"
    url       TEXT,
    synced_at TEXT                 -- ISO 8601
);

CREATE TABLE IF NOT EXISTS secrets (
    id        INTEGER PRIMARY KEY,
    remote_id TEXT NOT NULL REFERENCES remotes(id),
    env       TEXT NOT NULL,       -- "prod", "dev", "common", etc.
    path      TEXT NOT NULL,       -- "vars/prod/STRIPE_KEY.age"
    key_name  TEXT NOT NULL,       -- "STRIPE_KEY"
    updated_at TEXT,
    UNIQUE(remote_id, path)
);
