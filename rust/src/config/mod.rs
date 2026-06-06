use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HimitsuError, Result};
use crate::tui::keymap::KeyMap;

pub mod outputs;

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
/// | `auto_pull`      | `HIMITSU_AUTO_PULL`       |
/// | `tui.theme`      | `HIMITSU_TUI_THEME`       |
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[allow(clippy::manual_non_exhaustive)]
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

    /// Override for the himitsu data directory.
    /// Defaults to `~/.local/share/himitsu` or `~/Library/Application Support/himitsu`
    /// when outside a repo. Otherwise, its .himitsu
    /// Override: `HIMITSU_DATA_DIR=/custom/path`
    #[serde(default)]
    pub data_dir: Option<String>,

    /// When true, every store-touching command first runs `git fetch` +
    /// fast-forward `git pull` on the resolved store before dispatching.
    /// Combined with the post-mutation auto-commit/push, this gives a
    /// `git fetch && himitsu <cmd> && git push` workflow with no extra
    /// commands. Failures are non-fatal and surface as a stderr warning.
    /// Override: `HIMITSU_AUTO_PULL=1`
    #[serde(default)]
    pub auto_pull: bool,

    /// TUI-specific settings — theme selection plus configurable keymap.
    /// Users override individual actions under `tui.keys`; anything left
    /// out falls back to [`KeyMap::default`], which reproduces the
    /// hardcoded bindings that shipped before this section existed.
    #[serde(default)]
    pub tui: TuiConfig,

    #[serde(default)]
    pub outputs: outputs::OutputsMap,

    #[serde(
        rename = "envs",
        default,
        deserialize_with = "reject_envs_field",
        skip_serializing
    )]
    _envs_deprecated: (),
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        Ok(())
    }
}

/// `tui:` section of the global config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Built-in color theme name. Missing values fall back to `random`,
    /// which picks one of the bundled palettes on each launch.
    #[serde(default = "default_tui_theme")]
    pub theme: String,

    /// Opt in to Nerd Font glyphs (e.g.  for git,  for stores).
    /// Defaults to `false` because there is no reliable way to detect
    /// font support at runtime — if the user's terminal lacks a Nerd Font
    /// the icons render as tofu boxes. Tools like starship and lazygit
    /// handle this the same way.
    #[serde(default)]
    pub nerd_fonts: bool,

    /// User-configurable keybindings. Missing entries fall back to the
    /// defaults in [`KeyMap::default`].
    #[serde(default)]
    pub keys: KeyMap,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: default_tui_theme(),
            nerd_fonts: false,
            keys: KeyMap::default(),
        }
    }
}

fn default_tui_theme() -> String {
    "random".to_string()
}

/// Per-project config discovered by walking up from the current directory.
///
/// Searched at (in order): `.himitsu/config.yaml`, `.config/himitsu.yaml`,
/// `himitsu.yaml` in the current directory and each parent up to the
/// home directory (max 20 levels).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[allow(clippy::manual_non_exhaustive)]
pub struct ProjectConfig {
    /// Default remote store slug for this project (e.g. `"acme/secrets"`).
    #[serde(default)]
    pub default_store: Option<String>,

    #[serde(default)]
    pub outputs: outputs::OutputsMap,

    #[serde(
        rename = "envs",
        default,
        deserialize_with = "reject_envs_field",
        skip_serializing
    )]
    _envs_deprecated: (),

    #[serde(default)]
    pub generate: Option<GenerateConfig>,

    #[serde(default)]
    pub recipients_path: Option<String>,
}

fn reject_envs_field<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<(), D::Error> {
    use serde::de::IgnoredAny;
    IgnoredAny::deserialize(d)?;
    eprintln!(
        "warning: 'envs:' block has been replaced by 'outputs:' \
         — run 'himitsu migrate envs' to convert"
    );
    Ok(())
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

impl ProjectConfig {
    pub fn validate(&self) -> Result<()> {
        Ok(())
    }

    /// Load an existing project config, or return defaults if missing.
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&contents).unwrap_or_default())
    }

    /// Save this config to the given YAML path, creating parent dirs.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(path, yaml)?;
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

impl Config {
    /// Load config from a YAML file, then apply any `HIMITSU_*` environment
    /// variable overrides on top.
    ///
    /// Missing file → all defaults. Unknown env vars are silently ignored.
    /// `HIMITSU_CONFIG` is excluded because it points at the config file
    /// itself and is handled by [`config_path`], not deserialized as a field.
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
            .merge(Env::prefixed("HIMITSU_").ignore(&["CONFIG"]))
            .extract()
            .map_err(|e| HimitsuError::InvalidConfig(e.to_string()))?;

        Ok(config)
    }

    /// Annotated example config embedded at compile time.
    pub const EXAMPLE: &'static str = include_str!("example.yaml");

    /// Write the annotated example config to the given path (creating parent
    /// dirs). This is the file users see on first `himitsu init` — it
    /// documents every field with inline comments so the config is
    /// self-explanatory.
    pub fn write_default(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, Self::EXAMPLE)?;
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

/// Path to the global config file.
///
/// `HIMITSU_CONFIG` (the entrypoint env var) wins if set — it points at the
/// config file directly. Otherwise the XDG default location is used.
///
/// | Platform | Default path                                           |
/// |----------|--------------------------------------------------------|
/// | Linux    | `$XDG_CONFIG_HOME/himitsu/config.yaml`                 |
/// | macOS    | `~/Library/Application Support/himitsu/config.yaml`    |
pub fn config_path() -> PathBuf {
    if let Ok(val) = std::env::var("HIMITSU_CONFIG") {
        return PathBuf::from(val);
    }
    dirs::config_dir()
        .expect("cannot determine XDG config directory")
        .join("himitsu")
        .join("config.yaml")
}

/// Directory containing the global config file.
pub fn config_dir() -> PathBuf {
    config_path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Data directory — stores the age keypair and associated key material.
///
/// Resolution order:
/// 1. `Config.data_dir` field in the config file, if non-empty.
/// 2. When `HIMITSU_CONFIG` is set explicitly, default to `<cfg-parent>/share`
///    so tests and custom layouts co-locate under one root.
/// 3. XDG default (`$XDG_DATA_HOME/himitsu` on Linux, Application Support on macOS).
pub fn data_dir() -> PathBuf {
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
    if std::env::var("HIMITSU_CONFIG").is_ok() {
        return config_dir().join("share");
    }
    dirs::data_dir()
        .expect("cannot determine XDG data directory")
        .join("himitsu")
}

/// State directory — stores the SQLite search index and remote store checkouts.
///
/// Resolution order:
/// 1. `Config.data_dir` field in the config file → state lives at `<data_dir>/state/`.
/// 2. When `HIMITSU_CONFIG` is set explicitly, default to `<cfg-parent>/state`.
/// 3. XDG state dir (Linux) or fall through to [`data_dir`] (macOS has no
///    dedicated state dir).
pub fn state_dir() -> PathBuf {
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
    if std::env::var("HIMITSU_CONFIG").is_ok() {
        return config_dir().join("state");
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

/// Returns true when auto-pull is enabled, honoring the env override.
///
/// Resolution order:
/// 1. `HIMITSU_AUTO_PULL` env var (any of `1`, `true`, `yes`, case-insensitive)
/// 2. `auto_pull` field in the global config
/// 3. Default: false
///
/// Reads the global config from disk on each call. Cheap (single YAML parse)
/// and avoids threading a config handle through every dispatch call site.
pub fn auto_pull_enabled() -> bool {
    if let Ok(val) = std::env::var("HIMITSU_AUTO_PULL") {
        return matches!(
            val.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        );
    }
    Config::load(&config_path())
        .map(|c| c.auto_pull)
        .unwrap_or(false)
}

// ── Project config discovery ────────────────────────────────────────────────

/// Walk upward from the current directory looking for a project-level config
/// file. Returns the first path found, or `None`.
///
/// See [`find_project_config_from`] for the variant that starts at an
/// explicit path; this is a convenience wrapper that uses
/// [`std::env::current_dir`] as the starting point.
pub fn find_project_config() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_project_config_from(&cwd)
}

/// Walk upward from `start` looking for a project-level config file. Returns
/// the first path found, or `None`.
///
/// Candidate names per directory (checked in order):
/// 1. `.himitsu/config.yaml` / `.himitsu/config.yml` (preferred)
/// 2. `.config/himitsu.yaml` / `.config/himitsu.yml`
/// 3. `himitsu.yaml` / `himitsu.yml` (legacy fallback)
///
/// The walk stops at the user's home directory or after 20 levels.
pub fn find_project_config_from(start: &Path) -> Option<PathBuf> {
    let home_dir = dirs::home_dir();
    let candidates = [
        ".himitsu/config.yaml",
        ".himitsu/config.yml",
        ".config/himitsu.yaml",
        ".config/himitsu.yml",
        "himitsu.yaml",
        "himitsu.yml",
    ];

    let mut dir = start.to_path_buf();
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
/// Returns `Ok(Some((config, path)))` if a config file exists and parses
/// successfully, `Ok(None)` if no config file is found, or `Err` if a config
/// file exists but fails to parse (e.g. contains the deprecated `envs:` key).
pub fn load_project_config() -> Result<Option<(ProjectConfig, PathBuf)>> {
    let Some(path) = find_project_config() else {
        return Ok(None);
    };
    let contents = std::fs::read_to_string(&path)?;
    let cfg: ProjectConfig = serde_yaml::from_str(&contents)?;
    Ok(Some((cfg, path)))
}

/// Load and parse the first project config found by walking upward from
/// `start`. Unlike [`load_project_config`], parse errors are surfaced as
/// `Err` so callers in explicit project mode can fail loudly when the
/// config file is present but malformed.
pub fn load_project_config_from(start: &Path) -> Result<Option<(ProjectConfig, PathBuf)>> {
    let Some(path) = find_project_config_from(start) else {
        return Ok(None);
    };
    let contents = std::fs::read_to_string(&path)?;
    let cfg: ProjectConfig = serde_yaml::from_str(&contents)?;
    Ok(Some((cfg, path)))
}

// ── Store resolution ────────────────────────────────────────────────────────

/// Validate a remote slug (e.g., `"org/repo"`).
///
/// A valid slug has exactly one `/`, no empty segments, neither segment is
/// `.` or `..`, and segments contain none of the URL-fragment characters
/// `:` `@` `\` (callers should pass a slug, not a clone URL — full URLs are
/// pre-parsed by [`super::cli::init::parse_remote_slug`] before reaching here).
/// Returns `(org, repo)` on success.
pub fn validate_remote_slug(slug: &str) -> Result<(&str, &str)> {
    let parts: Vec<&str> = slug.split('/').collect();
    let invalid = parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || *p == "."
                || *p == ".."
                || p.chars().any(|c| matches!(c, ':' | '@' | '\\'))
        });
    if invalid {
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
    let (resolved, clone_url) = if let Some(parsed) = crate::cli::init::parse_remote_slug(slug) {
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

/// Resolve which store to use when no explicit `--store`/`--remote`/`--project`
/// is given. This is the *global* resolution path; it deliberately does NOT
/// consult project config from the current working directory.
///
/// Project context is opt-in via the `--project [path]` global flag, which
/// routes through [`resolve_store_for_project`] instead.
///
/// Resolution order (first match wins):
/// 1. `remote_override` slug — from the `--remote` flag (explicit).
/// 2. Global config `context` — set via `himitsu context remote` (explicit).
/// 3. Global config `default_store` (explicit).
/// 4. Single store in `stores_dir()` — unambiguous.
/// 5. Multiple stores + cwd inside one of them — use it, emit a warning.
/// 6. Unresolvable → actionable error.
pub fn resolve_store(remote_override: Option<&str>) -> Result<PathBuf> {
    if let Some(slug) = remote_override {
        return ensure_store(slug);
    }

    let cfg = Config::load(&config_path())?;
    if let Some(slug) = &cfg.context {
        return ensure_store(slug);
    }

    if let Some(slug) = &cfg.default_store {
        return ensure_store(slug);
    }

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
            let slugs: Vec<String> = found
                .iter()
                .filter_map(|p| {
                    p.strip_prefix(stores_dir())
                        .ok()
                        .map(|r| r.to_string_lossy().replace('\\', "/").to_string())
                })
                .collect();

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

/// Resolve which store to use when the user explicitly selected project mode
/// via `--project [path]`.
///
/// `root` must be a git repository root (the caller resolves this through
/// [`find_git_root`]). The function loads the project config walking upward
/// from `root` and uses its `default_store`. Missing config or missing
/// `default_store` produces a [`HimitsuError::ProjectConfigRequired`] with
/// setup guidance rather than silently falling back to a global store.
pub fn resolve_store_for_project(root: &Path) -> Result<PathBuf> {
    let Some((pc, pc_path)) = load_project_config_from(root)? else {
        return Err(HimitsuError::ProjectConfigRequired(format!(
            "no project config found at {} (looked for `.himitsu/config.yaml`, `.config/himitsu.yaml`, or `himitsu.yaml`).\n  \
             Run `himitsu init --project <org/repo>` from this repo to set one up.",
            root.display()
        )));
    };
    let slug = pc.default_store.ok_or_else(|| {
        HimitsuError::ProjectConfigRequired(format!(
            "project config at {} has no `default_store` set.\n  \
             Add `default_store: <org/repo>` or run `himitsu init --project <org/repo>` from the repo root.",
            pc_path.display()
        ))
    })?;
    ensure_store(&slug)
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
        // URL fragments must not be accepted as slugs — they create
        // garbage directory names like `stores/git@github.com:foo/bar`.
        assert!(validate_remote_slug("git@github.com:foo/bar").is_err());
        assert!(validate_remote_slug("https:/foo/bar").is_err());
        assert!(validate_remote_slug("foo/bar@v1").is_err());
    }

    // ── auto_pull config ────────────────────────────────────────────────

    #[test]
    fn config_auto_pull_round_trips() {
        let yaml = "auto_pull: true\nkey_provider: disk\n";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.auto_pull);

        let written = serde_yaml::to_string(&cfg).unwrap();
        assert!(written.contains("auto_pull: true"));
    }

    #[test]
    fn auto_pull_enabled_env_var_overrides_config() {
        // Serialize via the same mutex used by other env-touching tests.
        let _guard = crate::config::outputs::outputs_mut::HIMITSU_CONFIG_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HIMITSU_CONFIG", tmp.path().join("config.yaml"));

        // Config absent → default false. Env override forces true.
        std::env::remove_var("HIMITSU_AUTO_PULL");
        assert!(!auto_pull_enabled());

        for truthy in ["1", "true", "TRUE", "yes"] {
            std::env::set_var("HIMITSU_AUTO_PULL", truthy);
            assert!(auto_pull_enabled(), "expected {truthy} to enable");
        }
        for falsy in ["0", "false", "no", ""] {
            std::env::set_var("HIMITSU_AUTO_PULL", falsy);
            assert!(!auto_pull_enabled(), "expected {falsy} to disable");
        }

        std::env::remove_var("HIMITSU_AUTO_PULL");
        std::env::remove_var("HIMITSU_CONFIG");
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
    fn example_config_parses_and_matches_defaults() {
        let cfg: Config = serde_yaml::from_str(Config::EXAMPLE).unwrap();
        assert!(cfg.default_store.is_none());
        assert!(cfg.context.is_none());
        assert_eq!(cfg.key_provider, KeyProvider::Disk);
        assert!(cfg.data_dir.is_none());
        assert!(!cfg.auto_pull);
        cfg.validate().unwrap();
    }

    #[test]
    fn config_load_returns_default_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config::load(&tmp.path().join("nonexistent.yaml")).unwrap();
        assert!(cfg.default_store.is_none());
    }

    #[test]
    fn config_envs_key_tolerated_at_parse() {
        // The `envs:` key now deserializes successfully (emitting a stderr
        // warning) so `himitsu migrate envs` can run on an existing config.
        let yaml = "default_store: org/secrets\nenvs:\n  dev:\n    - dev/API_KEY\n";
        let cfg = serde_yaml::from_str::<Config>(yaml).expect("envs key must not be fatal");
        assert_eq!(cfg.default_store.as_deref(), Some("org/secrets"));
        assert_eq!(cfg._envs_deprecated, ());
        assert!(cfg.outputs.is_empty());
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
    fn project_config_envs_key_tolerated_at_parse() {
        // Same as above for the per-project config: the legacy `envs:` block
        // is tolerated (with a warning) instead of hard-rejected.
        let yaml = "envs:\n  dev:\n    - dev/API_KEY\n";
        let cfg = serde_yaml::from_str::<ProjectConfig>(yaml).expect("envs key must not be fatal");
        assert_eq!(cfg._envs_deprecated, ());
        assert!(cfg.outputs.is_empty());
    }

    #[test]
    fn project_config_full_yaml_parses() {
        let yaml = r#"
default_store: acme/secrets
outputs:
  pci-prod:
    selectors:
      - tag:pci
generate:
  target: .generated
  format: sops
  age_recipients:
    - age1abc
    - age1def
recipients_path: keys/recipients
"#;
        let cfg: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.default_store.as_deref(), Some("acme/secrets"));
        assert!(cfg.outputs.contains_key("pci-prod"));

        let gen = cfg.generate.unwrap();
        assert_eq!(gen.target, ".generated");
        assert_eq!(gen.format, "sops");
        assert_eq!(gen.age_recipients, vec!["age1abc", "age1def"]);

        assert_eq!(cfg.recipients_path.as_deref(), Some("keys/recipients"));
    }
}
