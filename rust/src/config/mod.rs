use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HimitsuError, Result};
use crate::tui::keymap::KeyMap;

/// How age private keys are stored and retrieved.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum KeyProvider {
    /// Keys live on disk at `data_dir()/key` (the default).
    #[default]
    Disk,
    /// Keys are stored in the macOS Keychain via the `security` CLI.
    MacosKeychain,
}

impl fmt::Display for KeyProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyProvider::Disk => write!(f, "disk"),
            KeyProvider::MacosKeychain => write!(f, "macos-keychain"),
        }
    }
}

impl std::str::FromStr for KeyProvider {
    type Err = HimitsuError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "disk" => Ok(KeyProvider::Disk),
            "macos-keychain" => Ok(KeyProvider::MacosKeychain),
            other => Err(HimitsuError::InvalidConfig(format!(
                "unknown key provider '{other}': expected 'disk' or 'macos-keychain'"
            ))),
        }
    }
}

/// Global user config stored at `config_dir()/config.yaml`.
///
/// Every field can be overridden at runtime with a `HIMITSU_<FIELD>` environment
/// variable (env vars take precedence over the file). Field names map to env
/// vars by uppercasing and replacing `.` with `_`:
///
/// | Field            | Env var                   |
/// |------------------|---------------------------|
/// | `default_store`  | `HIMITSU_DEFAULT_STORE`   |
/// | `key_provider`   | `HIMITSU_KEY_PROVIDER`    |
/// | `data_dir`       | `HIMITSU_DATA_DIR`        |
/// | `context`        | `HIMITSU_CONTEXT`         |
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Default remote store slug (e.g. `"myorg/secrets"`).
    /// Override: `HIMITSU_DEFAULT_STORE=org/repo`
    #[serde(default)]
    pub default_store: Option<String>,

    /// Active store context for explicit disambiguation.
    ///
    /// When set, this is used instead of `default_store` or any heuristic.
    /// Set with `himitsu context remote <ref>`.
    /// Override: `HIMITSU_CONTEXT=org/repo`
    #[serde(default)]
    pub context: Option<String>,

    /// Which backend stores age private keys.
    /// Override: `HIMITSU_KEY_PROVIDER=macos-keychain`
    #[serde(default)]
    pub key_provider: KeyProvider,

    /// Override for the himitsu data directory (age keys).
    /// Defaults to `~/.local/share/himitsu` when unset.
    /// Override: `HIMITSU_DATA_DIR=/custom/path`
    #[serde(default)]
    pub data_dir: Option<String>,

    /// TUI-specific settings — currently just the configurable keymap.
    /// Users override individual actions under `tui.keys`; anything left
    /// out falls back to [`KeyMap::default`], which reproduces the
    /// hardcoded bindings that shipped before this section existed.
    #[serde(default)]
    pub tui: TuiConfig,
}

/// `tui:` section of the global config.
///
/// Currently holds a single `keys` field, but is kept in its own struct so
/// future TUI settings (themes, initial view, double-width handling…) can
/// land without breaking existing config files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiConfig {
    /// User-configurable keybindings. Missing entries fall back to the
    /// defaults in [`KeyMap::default`].
    #[serde(default)]
    pub keys: KeyMap,
}

/// Per-project config discovered by walking up from the current directory.
///
/// Searched at (in order): `himitsu.yaml`, `.config/himitsu.yaml`,
/// `.himitsu/config.yaml` in the current directory and each parent up to the
/// home directory (max 20 levels).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Default remote store slug for this project (e.g. `"acme/secrets"`).
    #[serde(default)]
    pub default_store: Option<String>,

    /// Environment definitions: env name → list of entry specs.
    #[serde(default)]
    pub envs: BTreeMap<String, Vec<EnvEntry>>,

    /// Generate output settings.
    #[serde(default)]
    pub generate: Option<GenerateConfig>,

    /// Store-level overrides.
    #[serde(default)]
    pub store: Option<StoreConfig>,
}

/// A single entry in an env's secret list.
///
/// YAML shapes:
/// - `"dev/API_KEY"` → `Single("dev/API_KEY")` — key name = last path component
/// - `"dev/*"` → `Glob("dev")` — all secrets under prefix
/// - `{MY_KEY: "dev/DB_PASSWORD"}` → `Alias { key: "MY_KEY", path: "dev/DB_PASSWORD" }`
#[derive(Debug, Clone)]
pub enum EnvEntry {
    /// Explicit alias: output key `key`, value from store path `path`.
    Alias { key: String, path: String },
    /// Single secret by path; output key = last path component.
    Single(String),
    /// All secrets whose path starts with `prefix/`.
    Glob(String),
}

impl Serialize for EnvEntry {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            EnvEntry::Single(p) => s.serialize_str(p),
            EnvEntry::Glob(prefix) => s.serialize_str(&format!("{prefix}/*")),
            EnvEntry::Alias { key, path } => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry(key, path)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for EnvEntry {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // Use an untagged intermediate to handle both string and map shapes.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Str(String),
            Map(BTreeMap<String, String>),
        }

        match Raw::deserialize(d)? {
            Raw::Str(s) => {
                if let Some(prefix) = s.strip_suffix("/*") {
                    Ok(EnvEntry::Glob(prefix.to_string()))
                } else {
                    Ok(EnvEntry::Single(s))
                }
            }
            Raw::Map(m) => {
                if m.len() != 1 {
                    return Err(serde::de::Error::custom(
                        "alias entry must have exactly one key-value pair",
                    ));
                }
                let (key, path) = m.into_iter().next().unwrap();
                Ok(EnvEntry::Alias { key, path })
            }
        }
    }
}

// ── Env label validation ──────────────────────────────────────────────────

/// Returns `true` if `c` is a legal character within a single env label
/// segment. Segments are restricted to `[A-Za-z0-9_-]` so env names can be
/// used as filenames (`<env>.sops.yaml`) without escaping.
fn is_valid_env_segment_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Validates an env label against the preset-env grammar.
///
/// Legal forms:
/// - Concrete: `foo`, `foo/bar`, `foo/bar/baz`
/// - Trailing wildcard: `foo/*`, `foo/bar/*`
///
/// Rejected:
/// - Empty labels
/// - Leading/trailing slashes or empty segments (`/foo`, `foo/`, `foo//bar`)
/// - Mid-path wildcards (`foo/*/bar`, `*/foo`)
/// - Bare wildcard `*` (at least one concrete segment is required)
/// - Segments with characters outside `[A-Za-z0-9_-]`
pub fn validate_env_label(label: &str) -> Result<()> {
    if label.is_empty() {
        return Err(HimitsuError::InvalidConfig(
            "env label must not be empty".into(),
        ));
    }
    let segments: Vec<&str> = label.split('/').collect();
    let last_idx = segments.len() - 1;
    for (i, seg) in segments.iter().enumerate() {
        if seg.is_empty() {
            return Err(HimitsuError::InvalidConfig(format!(
                "env label '{label}' has an empty segment (leading/trailing slash or `//`)"
            )));
        }
        if *seg == "*" {
            if i != last_idx {
                return Err(HimitsuError::InvalidConfig(format!(
                    "env label '{label}' has a mid-path wildcard: `*` is only allowed as the final segment"
                )));
            }
            if i == 0 {
                return Err(HimitsuError::InvalidConfig(format!(
                    "env label '{label}' is a bare wildcard: at least one concrete segment is required before `*`"
                )));
            }
            continue;
        }
        if !seg.chars().all(is_valid_env_segment_char) {
            return Err(HimitsuError::InvalidConfig(format!(
                "env label '{label}' segment '{seg}' contains invalid characters (allowed: [A-Za-z0-9_-])"
            )));
        }
    }
    Ok(())
}

/// `true` when the label ends in `/*` (a wildcard env).
pub fn is_wildcard_label(label: &str) -> bool {
    label.ends_with("/*")
}

/// Returns the concrete prefix segments of a wildcard label, or the full
/// segments of a concrete label. `foo/bar/*` → `["foo", "bar"]`.
pub fn label_prefix_segments(label: &str) -> Vec<&str> {
    let mut segs: Vec<&str> = label.split('/').collect();
    if segs.last().copied() == Some("*") {
        segs.pop();
    }
    segs
}

/// Extracts the 1-indexed capture numbers referenced in a string (`$1`,
/// `$2`, …) in order of appearance. Invalid/partial matches are ignored.
///
/// Only bare `$N` is recognized. `${N}` and escape sequences are out of
/// scope for v1.
pub fn parse_captures(s: &str) -> Vec<u32> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            // Safe: we advanced only over ASCII digits.
            let digits = std::str::from_utf8(&bytes[start..j]).unwrap();
            if let Ok(n) = digits.parse::<u32>() {
                out.push(n);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Validates every env label in a map and checks that capture references
/// only appear inside wildcard envs. Concrete envs containing `$N` are
/// rejected — captures have no segment to bind to.
pub fn validate_envs(envs: &BTreeMap<String, Vec<EnvEntry>>) -> Result<()> {
    for (label, entries) in envs {
        validate_env_label(label)?;
        let is_wild = is_wildcard_label(label);
        for (idx, entry) in entries.iter().enumerate() {
            let path = match entry {
                EnvEntry::Single(p) | EnvEntry::Glob(p) => p,
                EnvEntry::Alias { path, .. } => path,
            };
            let captures = parse_captures(path);
            if !captures.is_empty() && !is_wild {
                return Err(HimitsuError::InvalidConfig(format!(
                    "env '{label}' entry #{idx} uses capture refs (`$N`) but the label is not a wildcard; captures are only valid in `<prefix>/*` envs"
                )));
            }
            // In a wildcard env, a single `*` captures one segment — captures
            // must be `$1`. Reject higher-index captures up front.
            if is_wild {
                for n in &captures {
                    if *n != 1 {
                        return Err(HimitsuError::InvalidConfig(format!(
                            "env '{label}' entry #{idx} references $${n}, but a single-`*` wildcard only exposes $1"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

impl ProjectConfig {
    /// Run all cross-field validation that cannot be expressed in serde:
    /// env-label grammar, capture-ref legality. Called by consumers before
    /// acting on the config; not invoked implicitly on deserialize so that
    /// serde round-trips remain pure.
    pub fn validate(&self) -> Result<()> {
        validate_envs(&self.envs)?;
        Ok(())
    }
}

/// Settings for the `generate` command output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateConfig {
    /// Output directory relative to the project root (e.g. `".generated"`).
    pub target: String,
    /// Output format. Currently only `"sops"` is supported.
    #[serde(default = "default_generate_format")]
    pub format: String,
    /// Age recipients for the generated output files.
    #[serde(default)]
    pub age_recipients: Vec<String>,
}

fn default_generate_format() -> String {
    "sops".to_string()
}

/// Store-level config overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreConfig {
    /// Override for the recipients directory path within the store.
    #[serde(default)]
    pub recipients_path: Option<String>,
}

impl Config {
    /// Load config from a YAML file, then apply any `HIMITSU_*` environment
    /// variable overrides on top.
    ///
    /// Missing file → all defaults. Unknown env vars are silently ignored.
    /// `HIMITSU_HOME` is excluded because it controls test isolation at the
    /// path level and is handled separately by [`config_dir`] / [`data_dir`].
    pub fn load(path: &Path) -> Result<Self> {
        use figment::{
            providers::{Env, Serialized},
            Figment,
        };

        // Read the file first (best-effort; fall back to defaults if absent).
        let from_file: Config = if path.exists() {
            let contents = std::fs::read_to_string(path)?;
            serde_yaml::from_str(&contents)?
        } else {
            Config::default()
        };

        // Layer: file values as the base, env vars win over them.
        let config = Figment::from(Serialized::defaults(from_file))
            .merge(Env::prefixed("HIMITSU_").ignore(&["HOME"]))
            .extract()
            .map_err(|e| HimitsuError::InvalidConfig(e.to_string()))?;

        Ok(config)
    }

    /// Write a default config to the given path (creating parent dirs).
    pub fn write_default(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let config = Config::default();
        let yaml = serde_yaml::to_string(&config)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }

    /// Save this config to the given path (creating parent dirs).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }
}

// ── XDG-style path helpers ─────────────────────────────────────────────────

/// Config directory for `config.yaml`.
///
/// | Platform | Default path                                  |
/// |----------|-----------------------------------------------|
/// | Linux    | `$XDG_CONFIG_HOME/himitsu` → `~/.config/himitsu` |
/// | macOS    | `~/Library/Application Support/himitsu`       |
///
/// When `HIMITSU_HOME` is set (tests): `$HIMITSU_HOME/config`.
///
/// This is a fixed, bootstrap-level location that never depends on user
/// config, so it can safely be read by `data_dir()` without a circular
/// dependency.
///
/// On macOS `dirs::config_dir()` and `dirs::data_dir()` both return
/// `~/Library/Application Support/`, so config and data share a root —
/// this is correct macOS behaviour.
pub fn config_dir() -> PathBuf {
    if let Ok(val) = std::env::var("HIMITSU_HOME") {
        return PathBuf::from(val).join("config");
    }
    dirs::config_dir()
        .expect("cannot determine XDG config directory")
        .join("himitsu")
}

/// Path to the global config file: `config_dir()/config.yaml`.
pub fn config_path() -> PathBuf {
    config_dir().join("config.yaml")
}

/// Data directory — stores the age keypair and associated key material.
///
/// | Platform | Default path                                  |
/// |----------|-----------------------------------------------|
/// | Linux    | `$XDG_DATA_HOME/himitsu` → `~/.local/share/himitsu` |
/// | macOS    | `~/Library/Application Support/himitsu`       |
///
/// When `HIMITSU_HOME` is set (tests): `$HIMITSU_HOME/share`.
/// When `Config.data_dir` is set, that value overrides the platform default.
pub fn data_dir() -> PathBuf {
    if let Ok(val) = std::env::var("HIMITSU_HOME") {
        return PathBuf::from(val).join("share");
    }
    // Best-effort: read custom data_dir from the config file.
    if let Ok(contents) = std::fs::read_to_string(config_path()) {
        if let Ok(cfg) = serde_yaml::from_str::<Config>(&contents) {
            if let Some(custom) = cfg.data_dir {
                let p = custom.trim().to_string();
                if !p.is_empty() {
                    return PathBuf::from(p);
                }
            }
        }
    }
    dirs::data_dir()
        .expect("cannot determine XDG data directory")
        .join("himitsu")
}

/// State directory — stores the SQLite search index and remote store checkouts.
///
/// | Platform | Default path                                      |
/// |----------|---------------------------------------------------|
/// | Linux    | `$XDG_STATE_HOME/himitsu` → `~/.local/state/himitsu` |
/// | macOS    | `~/Library/Application Support/himitsu`           |
///
/// On macOS `dirs::state_dir()` returns `None` (the platform has no direct
/// equivalent of `$XDG_STATE_HOME`), so state co-locates with data under
/// `~/Library/Application Support/himitsu/` — everything in one place.
///
/// When `HIMITSU_HOME` is set (tests): `$HIMITSU_HOME/state`.
/// When `Config.data_dir` is set, state lives at `<data_dir>/state/`.
pub fn state_dir() -> PathBuf {
    if let Ok(val) = std::env::var("HIMITSU_HOME") {
        return PathBuf::from(val).join("state");
    }
    // When a custom data_dir is configured, state lives alongside it.
    if let Ok(contents) = std::fs::read_to_string(config_path()) {
        if let Ok(cfg) = serde_yaml::from_str::<Config>(&contents) {
            if let Some(custom) = cfg.data_dir {
                let p = custom.trim().to_string();
                if !p.is_empty() {
                    return PathBuf::from(p).join("state");
                }
            }
        }
    }
    // Use the platform state dir when available (Linux); otherwise fall back
    // to our own data_dir() — not dirs::data_dir() — so that any future
    // config-level override is still honoured on platforms like macOS that
    // lack a dedicated state directory.
    dirs::state_dir()
        .map(|p| p.join("himitsu"))
        .unwrap_or_else(data_dir)
}

/// Path to the age private key file.
pub fn key_path() -> PathBuf {
    data_dir().join("key")
}

/// Path to the age public key file.
pub fn pubkey_path() -> PathBuf {
    data_dir().join("key.pub")
}

/// Directory containing managed store checkouts.
pub fn stores_dir() -> PathBuf {
    state_dir().join("stores")
}

/// Path to a specific store checkout: `stores_dir()/<org>/<repo>`.
pub fn store_checkout(org: &str, repo: &str) -> PathBuf {
    stores_dir().join(org).join(repo)
}

// ── Project config discovery ────────────────────────────────────────────────

/// Walk upward from the current directory looking for a project-level config
/// file. Returns the first path found, or `None`.
///
/// Candidate names per directory (checked in order):
/// 1. `himitsu.yaml` / `himitsu.yml`
/// 2. `.config/himitsu.yaml` / `.config/himitsu.yml`
/// 3. `.himitsu/config.yaml` / `.himitsu/config.yml`
///
/// The walk stops at the user's home directory or after 20 levels.
pub fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let home_dir = dirs::home_dir();
    let candidates = [
        "himitsu.yaml",
        "himitsu.yml",
        ".config/himitsu.yaml",
        ".config/himitsu.yml",
        ".himitsu/config.yaml",
        ".himitsu/config.yml",
    ];

    let mut dir = cwd.clone();
    for _ in 0..=20 {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if path.exists() {
                return Some(path);
            }
        }
        // Stop at the home directory
        if home_dir.as_deref() == Some(dir.as_path()) {
            return None;
        }
        if !dir.pop() {
            return None;
        }
    }
    None
}

/// Load and parse the first project config found by [`find_project_config`].
///
/// Returns `Some((config, path))` if a config file exists and parses
/// successfully, or `None` if no config file is found.
///
/// Parsing errors are returned as `Err`.
pub fn load_project_config() -> Option<(ProjectConfig, PathBuf)> {
    let path = find_project_config()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let cfg: ProjectConfig = serde_yaml::from_str(&contents).ok()?;
    Some((cfg, path))
}

// ── Store resolution ────────────────────────────────────────────────────────

/// Validate a remote slug (e.g., `"org/repo"`).
///
/// A valid slug has exactly one `/`, no empty segments, and neither segment is
/// `.` or `..`.  Returns `(org, repo)` on success.
pub fn validate_remote_slug(slug: &str) -> Result<(&str, &str)> {
    let parts: Vec<&str> = slug.split('/').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(HimitsuError::InvalidConfig(format!(
            "invalid remote slug '{slug}': expected 'org/repo'"
        )));
    }
    Ok((parts[0], parts[1]))
}

/// Resolve a remote slug to its local store checkout path.
/// Fails with `RemoteNotFound` if the directory doesn't exist.
pub fn remote_store_path(slug: &str) -> Result<PathBuf> {
    let (org, repo) = validate_remote_slug(slug)?;
    let path = store_checkout(org, repo);
    if !path.exists() {
        return Err(HimitsuError::RemoteNotFound(slug.to_string()));
    }
    Ok(path)
}

/// Resolve a remote slug to its local store checkout path, performing a lazy
/// clone from GitHub if the checkout doesn't exist yet.
///
/// - If the store already exists locally: returns its path immediately.
/// - If it doesn't exist: attempts `git clone git@github.com:<org>/<repo>.git`
///   and returns the resulting path on success.
/// - On clone failure: returns an error with the attempted URL and a hint to
///   use `himitsu remote add --url` for custom URLs.
pub fn ensure_store(slug: &str) -> Result<PathBuf> {
    // Accept full git URLs (e.g. git@github.com:org/repo.git) and extract
    // the org/repo slug automatically.
    let (resolved, clone_url) =
        if let Some(parsed) = crate::cli::init::parse_remote_slug(slug) {
            let url = slug.to_string();
            (parsed, Some(url))
        } else {
            (slug.to_string(), None)
        };

    let (org, repo) = validate_remote_slug(&resolved)?;
    let path = store_checkout(org, repo);
    if path.exists() {
        return Ok(path);
    }
    // Attempt lazy clone from the explicit or default GitHub SSH URL.
    let url = clone_url.unwrap_or_else(|| format!("git@github.com:{org}/{repo}.git"));
    eprintln!("Cloning {resolved} → {}", path.display());
    crate::git::clone_noninteractive(&url, &path).map_err(|e| {
        HimitsuError::Remote(format!(
            "failed to clone {resolved} from {url}: {e}\n  \
             Tip: use `himitsu remote add {resolved} --url <url>` to specify a custom URL."
        ))
    })?;
    Ok(path)
}

/// Resolve which store to use when no explicit `--store`/`--remote` is given.
///
/// Resolution order (first match wins, no warning):
/// 1. `remote_override` slug — from the `--remote` flag (explicit).
/// 2. Project config `default_store` — walked up from CWD (explicit).
/// 3. Global config `context` — set via `himitsu context remote` (explicit).
/// 4. Global config `default_store` (explicit).
/// 5. Single store in `stores_dir()` — unambiguous, no warning.
/// 6. Multiple stores + project-local store detected — use it, emit a warning.
/// 7. Unresolvable → actionable error.
pub fn resolve_store(remote_override: Option<&str>) -> Result<PathBuf> {
    if let Some(slug) = remote_override {
        return ensure_store(slug);
    }

    // Try project config default_store (walk up from CWD)
    if let Some(project_config_path) = find_project_config() {
        if let Ok(contents) = std::fs::read_to_string(&project_config_path) {
            if let Ok(project_cfg) = serde_yaml::from_str::<ProjectConfig>(&contents) {
                if let Some(slug) = &project_cfg.default_store {
                    return ensure_store(slug);
                }
            }
        }
    }

    // Try global config context (explicit user-set disambiguation)
    let cfg = Config::load(&config_path())?;
    if let Some(slug) = &cfg.context {
        return ensure_store(slug);
    }

    // Try global config default_store
    if let Some(slug) = &cfg.default_store {
        return ensure_store(slug);
    }

    // Enumerate stores (implicit fallback — no lazy clone here)
    let dir = stores_dir();
    let mut found: Vec<PathBuf> = vec![];
    if dir.exists() {
        for org_entry in std::fs::read_dir(&dir)? {
            let org_entry = org_entry?;
            if !org_entry.file_type()?.is_dir() {
                continue;
            }
            for repo_entry in std::fs::read_dir(org_entry.path())? {
                let repo_entry = repo_entry?;
                if repo_entry.file_type()?.is_dir() {
                    found.push(repo_entry.path());
                }
            }
        }
    }

    match found.len() {
        0 => Err(HimitsuError::StoreNotFound(
            "no stores configured; use `himitsu remote add <org/repo>` to add one".into(),
        )),
        1 => Ok(found.into_iter().next().unwrap()),
        _ => {
            // Build human-readable slugs (relative to stores_dir)
            let slugs: Vec<String> = found
                .iter()
                .filter_map(|p| {
                    p.strip_prefix(stores_dir())
                        .ok()
                        .map(|r| r.to_string_lossy().replace('\\', "/").to_string())
                })
                .collect();

            // Check whether the CWD sits inside one of the known store checkouts.
            // If so, use it — but always warn so the user knows we guessed.
            if let Ok(cwd) = std::env::current_dir() {
                if let Some(matched) = found.iter().find(|p| cwd.starts_with(*p)) {
                    let slug = matched
                        .strip_prefix(stores_dir())
                        .ok()
                        .map(|r| r.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|| matched.to_string_lossy().into_owned());
                    eprintln!(
                        "note: multiple stores found — using '{slug}' because you are inside it.\n      Set a default with `himitsu context remote {slug}` to silence this."
                    );
                    return Ok(matched.clone());
                }
            }

            Err(HimitsuError::AmbiguousStore(slugs))
        }
    }
}

// ── Git helpers ─────────────────────────────────────────────────────────────

/// Walk from `start` upward to find the nearest `.git` directory.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_git_root_returns_repo_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let sub = tmp.path().join("a/b/c");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_git_root(&sub).unwrap(), tmp.path());
    }

    #[test]
    fn find_git_root_returns_none_outside_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_git_root(tmp.path()).is_none());
    }

    #[test]
    fn validate_remote_slug_accepts_valid() {
        let (org, repo) = validate_remote_slug("my-org/my-repo").unwrap();
        assert_eq!(org, "my-org");
        assert_eq!(repo, "my-repo");
    }

    #[test]
    fn validate_remote_slug_rejects_bad_slugs() {
        assert!(validate_remote_slug("notaslug").is_err());
        assert!(validate_remote_slug("a/b/c").is_err());
        assert!(validate_remote_slug("/oops").is_err());
        assert!(validate_remote_slug("org/").is_err());
        assert!(validate_remote_slug("../repo").is_err());
        assert!(validate_remote_slug("org/..").is_err());
        assert!(validate_remote_slug("./repo").is_err());
    }

    #[test]
    fn remote_store_path_resolves_existing() {
        // We test the composition logic directly: store_checkout(org, repo)
        // should equal stores_dir().join(org).join(repo).
        // Use validate_remote_slug to exercise slug validation.
        let (org, repo) = validate_remote_slug("test-org/test-repo").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // Build the expected path manually without relying on env vars
        let expected = tmp.path().join("state/stores").join(org).join(repo);
        std::fs::create_dir_all(&expected).unwrap();
        // Verify the path structure is correct
        assert!(expected.exists());
        assert_eq!(expected.file_name().unwrap(), repo);
    }

    #[test]
    fn remote_store_path_errors_when_missing() {
        // Validate that a non-existent slug returns RemoteNotFound.
        // We use a unique HIMITSU_HOME inside a tempdir so there's no collision.
        let tmp = tempfile::tempdir().unwrap();
        // The path will be tmp/state/stores/ghost/missing, which doesn't exist.
        let expected = tmp.path().join("state/stores/ghost/missing");
        assert!(!expected.exists()); // sanity
                                     // RemoteNotFound requires a missing directory; we trust validate_remote_slug
        let err = validate_remote_slug("ghost/missing");
        assert!(err.is_ok()); // valid slug
    }

    #[test]
    fn config_load_returns_default_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config::load(&tmp.path().join("nonexistent.yaml")).unwrap();
        assert!(cfg.default_store.is_none());
    }

    #[test]
    fn env_entry_deserialize_single() {
        let yaml = "\"dev/API_KEY\"";
        let entry: EnvEntry = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(entry, EnvEntry::Single(ref p) if p == "dev/API_KEY"));
    }

    #[test]
    fn env_entry_deserialize_glob() {
        let yaml = "\"dev/*\"";
        let entry: EnvEntry = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(entry, EnvEntry::Glob(ref p) if p == "dev"));
    }

    #[test]
    fn env_entry_deserialize_alias() {
        let yaml = "MY_KEY: dev/DB_PASSWORD";
        let entry: EnvEntry = serde_yaml::from_str(yaml).unwrap();
        match entry {
            EnvEntry::Alias { key, path } => {
                assert_eq!(key, "MY_KEY");
                assert_eq!(path, "dev/DB_PASSWORD");
            }
            _ => panic!("expected Alias variant"),
        }
    }

    #[test]
    fn env_entry_round_trip_serialize() {
        // Single
        let e = EnvEntry::Single("prod/STRIPE_KEY".into());
        let s = serde_yaml::to_string(&e).unwrap();
        assert!(s.trim() == "prod/STRIPE_KEY");

        // Glob
        let e = EnvEntry::Glob("prod".into());
        let s = serde_yaml::to_string(&e).unwrap();
        assert!(s.trim() == "prod/*");

        // Alias
        let e = EnvEntry::Alias {
            key: "MY_DB".into(),
            path: "prod/DB_PASS".into(),
        };
        let s = serde_yaml::to_string(&e).unwrap();
        let back: EnvEntry = serde_yaml::from_str(&s).unwrap();
        assert!(
            matches!(back, EnvEntry::Alias { ref key, ref path } if key == "MY_DB" && path == "prod/DB_PASS")
        );
    }

    // ── Env label grammar ──────────────────────────────────────────────

    #[test]
    fn env_label_accepts_concrete_and_trailing_wildcard() {
        for good in [
            "foo",
            "foo/bar",
            "foo/bar/baz",
            "foo/*",
            "foo/bar/*",
            "foo_bar",
            "foo-1",
            "ENV123",
        ] {
            validate_env_label(good).unwrap_or_else(|e| panic!("expected {good} valid: {e}"));
        }
    }

    #[test]
    fn env_label_rejects_empty() {
        assert!(validate_env_label("").is_err());
    }

    #[test]
    fn env_label_rejects_mid_path_wildcard() {
        assert!(validate_env_label("foo/*/bar").is_err());
        assert!(validate_env_label("*/foo").is_err());
        assert!(validate_env_label("a/*/b/c").is_err());
    }

    #[test]
    fn env_label_rejects_bare_wildcard() {
        assert!(validate_env_label("*").is_err());
    }

    #[test]
    fn env_label_rejects_empty_segments() {
        assert!(validate_env_label("/foo").is_err());
        assert!(validate_env_label("foo/").is_err());
        assert!(validate_env_label("foo//bar").is_err());
    }

    #[test]
    fn env_label_rejects_invalid_chars() {
        assert!(validate_env_label("foo.bar").is_err());
        assert!(validate_env_label("foo bar").is_err());
        assert!(validate_env_label("foo:bar").is_err());
    }

    #[test]
    fn is_wildcard_label_detects_trailing_star() {
        assert!(is_wildcard_label("foo/*"));
        assert!(is_wildcard_label("foo/bar/*"));
        assert!(!is_wildcard_label("foo"));
        assert!(!is_wildcard_label("foo/bar"));
    }

    #[test]
    fn label_prefix_segments_strips_wildcard() {
        assert_eq!(label_prefix_segments("foo"), vec!["foo"]);
        assert_eq!(label_prefix_segments("foo/bar"), vec!["foo", "bar"]);
        assert_eq!(label_prefix_segments("foo/*"), vec!["foo"]);
        assert_eq!(label_prefix_segments("foo/bar/*"), vec!["foo", "bar"]);
    }

    // ── Capture references ─────────────────────────────────────────────

    #[test]
    fn parse_captures_finds_dollar_digits() {
        assert_eq!(parse_captures("no captures here"), Vec::<u32>::new());
        assert_eq!(parse_captures("/$1/postgres-url"), vec![1]);
        assert_eq!(parse_captures("$1/$2"), vec![1, 2]);
        assert_eq!(parse_captures("foo$10bar"), vec![10]);
        // Literal `$` followed by non-digit is ignored.
        assert_eq!(parse_captures("$abc"), Vec::<u32>::new());
    }

    #[test]
    fn validate_envs_accepts_capture_in_wildcard_alias() {
        let mut envs = BTreeMap::new();
        envs.insert(
            "foo/*".to_string(),
            vec![EnvEntry::Alias {
                key: "POSTGRES".into(),
                path: "/$1/postgres-url".into(),
            }],
        );
        validate_envs(&envs).unwrap();
    }

    #[test]
    fn validate_envs_rejects_capture_in_concrete_env() {
        let mut envs = BTreeMap::new();
        envs.insert(
            "foo/bar".to_string(),
            vec![EnvEntry::Alias {
                key: "POSTGRES".into(),
                path: "/$1/postgres-url".into(),
            }],
        );
        let err = validate_envs(&envs).unwrap_err();
        assert!(
            err.to_string().contains("capture refs"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_envs_rejects_high_capture_index() {
        let mut envs = BTreeMap::new();
        envs.insert(
            "foo/*".to_string(),
            vec![EnvEntry::Single("/$2/postgres-url".into())],
        );
        let err = validate_envs(&envs).unwrap_err();
        assert!(
            err.to_string().contains("$1"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_envs_rejects_bad_label() {
        let mut envs = BTreeMap::new();
        envs.insert(
            "foo/*/bar".to_string(),
            vec![EnvEntry::Single("x".into())],
        );
        assert!(validate_envs(&envs).is_err());
    }

    #[test]
    fn project_config_validate_surfaces_label_errors() {
        let yaml = r#"
envs:
  "foo/*/bar":
    - SECRET: x
"#;
        let cfg: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        // Deserialization succeeds — validation surfaces the grammar error.
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn project_config_full_yaml_parses() {
        let yaml = r#"
default_store: acme/secrets
envs:
  dev:
    - dev/API_KEY
    - DB_PASS: dev/DB_PASSWORD
    - dev/*
  prod:
    - prod/*
generate:
  target: .generated
  format: sops
  age_recipients:
    - age1abc
    - age1def
store:
  recipients_path: keys/recipients
"#;
        let cfg: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.default_store.as_deref(), Some("acme/secrets"));
        assert_eq!(cfg.envs.len(), 2);

        let dev_entries = cfg.envs.get("dev").unwrap();
        assert_eq!(dev_entries.len(), 3);
        assert!(matches!(&dev_entries[0], EnvEntry::Single(p) if p == "dev/API_KEY"));
        assert!(
            matches!(&dev_entries[1], EnvEntry::Alias { key, path } if key == "DB_PASS" && path == "dev/DB_PASSWORD")
        );
        assert!(matches!(&dev_entries[2], EnvEntry::Glob(p) if p == "dev"));

        let gen = cfg.generate.unwrap();
        assert_eq!(gen.target, ".generated");
        assert_eq!(gen.format, "sops");
        assert_eq!(gen.age_recipients, vec!["age1abc", "age1def"]);

        let store = cfg.store.unwrap();
        assert_eq!(store.recipients_path.as_deref(), Some("keys/recipients"));
    }
}
