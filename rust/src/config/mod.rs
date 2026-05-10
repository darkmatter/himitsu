use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HimitsuError, Result};
use crate::tui::keymap::KeyMap;

pub mod env_cache;
pub mod env_dsl;
pub mod env_resolver;
pub mod envs_mut;

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

    /// Environment definitions at global scope: env name → list of entry
    /// specs. Mirrors the `envs:` field on [`ProjectConfig`]; the two may
    /// coexist and the TUI / codegen walk layers resolve which scope wins
    /// for any given label.
    #[serde(default)]
    pub envs: BTreeMap<String, Vec<EnvEntry>>,
}

impl Config {
    /// Run cross-field validation that cannot be expressed in serde: currently
    /// just env-label grammar and capture-ref legality on [`Config::envs`].
    /// Called by consumers before writing the config back to disk.
    pub fn validate(&self) -> Result<()> {
        validate_envs(&self.envs)?;
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

    /// Recipients directory path override.
    #[serde(default)]
    pub recipients_path: Option<String>,
}

/// A single entry in an env's secret list.
///
/// YAML shapes:
/// - `"dev/API_KEY"` → `Single("dev/API_KEY")` — key name = last path component
/// - `"dev/*"` → `Glob("dev")` — all secrets under prefix
/// - `{MY_KEY: "dev/DB_PASSWORD"}` → `Alias { key: "MY_KEY", path: "dev/DB_PASSWORD" }`
/// - `"tag:pci"` or `{tag: pci}` → `Tag("pci")` — every secret carrying tag `pci`
/// - `{STRIPE: "tag:stripe"}` → `AliasTag { key: "STRIPE", tag: "stripe" }`
///
/// Tag entries select secrets by their encrypted `SecretValue.tags` field;
/// resolution requires decrypting candidate secrets (see
/// [`super::env_resolver::resolve_with_tags`]). Capture-ref interpolation
/// (`$1`) is **not** supported on tag entries — the tag string is opaque.
#[derive(Debug, Clone)]
pub enum EnvEntry {
    /// Explicit alias: output key `key`, value from store path `path`.
    Alias { key: String, path: String },
    /// Single secret by path; output key = last path component.
    Single(String),
    /// All secrets whose path starts with `prefix/`.
    Glob(String),
    /// All secrets carrying the named tag. Output key per-secret = last path
    /// component of the matched secret.
    Tag(String),
    /// Aliased tag selector: exactly one secret must carry the named tag,
    /// and its value is bound to the explicit output key. Errors when zero
    /// or more than one secret matches.
    AliasTag { key: String, tag: String },
}

impl Serialize for EnvEntry {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            EnvEntry::Single(p) => s.serialize_str(p),
            EnvEntry::Glob(prefix) => s.serialize_str(&format!("{prefix}/*")),
            EnvEntry::Tag(name) => s.serialize_str(&format!("tag:{name}")),
            EnvEntry::Alias { key, path } => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry(key, path)?;
                map.end()
            }
            EnvEntry::AliasTag { key, tag } => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry(key, &format!("tag:{tag}"))?;
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

        // Strip a leading `tag:` and validate the suffix against the shared
        // tag grammar. Returns `Some(validated_tag)` on the prefix path, or
        // `None` when the input was not a tag selector at all. Surfaces
        // grammar errors as `serde::de::Error::custom` so YAML callers see
        // a clean parse-time failure rather than a downstream resolver error.
        fn parse_tag_selector<E: serde::de::Error>(
            s: &str,
        ) -> std::result::Result<Option<String>, E> {
            let Some(rest) = s.strip_prefix("tag:") else {
                return Ok(None);
            };
            crate::crypto::tags::validate_tag(rest).map_err(serde::de::Error::custom)?;
            Ok(Some(rest.to_string()))
        }

        match Raw::deserialize(d)? {
            Raw::Str(s) => {
                // Order matters: `tag:` prefix must be checked before the
                // path/glob branch, otherwise `tag:pci` would parse as a
                // literal `Single` path.
                if let Some(tag) = parse_tag_selector(&s)? {
                    Ok(EnvEntry::Tag(tag))
                } else if let Some(prefix) = s.strip_suffix("/*") {
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
                let (key, value) = m.into_iter().next().unwrap();
                // Map form `{ tag: pci }` — the literal key is `tag` and the
                // value is the tag name itself.
                if key == "tag" {
                    crate::crypto::tags::validate_tag(&value).map_err(serde::de::Error::custom)?;
                    return Ok(EnvEntry::Tag(value));
                }
                // Map form `{ STRIPE: tag:stripe }` — alias whose value is
                // a `tag:` selector rather than a path.
                if let Some(tag) = parse_tag_selector(&value)? {
                    return Ok(EnvEntry::AliasTag { key, tag });
                }
                Ok(EnvEntry::Alias { key, path: value })
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
            // Tag selectors are opaque tag names, not paths — capture refs
            // and `$1` checks don't apply. Tag grammar was enforced at
            // deserialize time via `crate::crypto::tags::validate_tag`.
            let path = match entry {
                EnvEntry::Single(p) | EnvEntry::Glob(p) => p,
                EnvEntry::Alias { path, .. } => path,
                EnvEntry::Tag(_) | EnvEntry::AliasTag { .. } => continue,
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

/// Merge global and project env definitions with project labels taking
/// precedence. This is the env lookup surface used by CLI consumers; the TUI
/// can still show scopes separately when scope matters for editing.
pub fn merge_envs(
    global: &BTreeMap<String, Vec<EnvEntry>>,
    project: Option<&BTreeMap<String, Vec<EnvEntry>>>,
) -> BTreeMap<String, Vec<EnvEntry>> {
    let mut merged = global.clone();
    if let Some(project) = project {
        merged.extend(project.clone());
    }
    merged
}

/// Load global + project env definitions for command resolution.
pub fn load_effective_envs() -> Result<BTreeMap<String, Vec<EnvEntry>>> {
    let global = Config::load(&config_path())?;
    let project = load_project_config().map(|(cfg, _)| cfg);
    Ok(merge_envs(
        &global.envs,
        project.as_ref().map(|cfg| &cfg.envs),
    ))
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
        let _guard = crate::config::envs_mut::HIMITSU_CONFIG_TEST_GUARD
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
        assert!(cfg.envs.is_empty());
        cfg.validate().unwrap();
    }

    #[test]
    fn config_load_returns_default_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config::load(&tmp.path().join("nonexistent.yaml")).unwrap();
        assert!(cfg.default_store.is_none());
    }

    #[test]
    fn config_envs_round_trip_serde() {
        // YAML with envs at global scope should deserialize into Config and
        // serialize back with the same shape (labels + entries preserved).
        let yaml = r#"
default_store: org/secrets
envs:
  dev:
    - dev/API_KEY
    - DB_PASS: dev/DB_PASSWORD
  prod/*:
    - POSTGRES: /$1/postgres-url
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.default_store.as_deref(), Some("org/secrets"));
        assert_eq!(cfg.envs.len(), 2);

        let dev = cfg.envs.get("dev").unwrap();
        assert_eq!(dev.len(), 2);
        assert!(matches!(&dev[0], EnvEntry::Single(p) if p == "dev/API_KEY"));
        assert!(
            matches!(&dev[1], EnvEntry::Alias { key, path } if key == "DB_PASS" && path == "dev/DB_PASSWORD")
        );

        cfg.validate().unwrap();

        // Round-trip through YAML and back.
        let serialized = serde_yaml::to_string(&cfg).unwrap();
        let cfg2: Config = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(cfg2.envs.len(), 2);
        assert!(cfg2.envs.contains_key("dev"));
        assert!(cfg2.envs.contains_key("prod/*"));
    }

    #[test]
    fn merge_envs_keeps_global_and_project_overrides_conflicts() {
        let mut global = BTreeMap::new();
        global.insert(
            "shared".to_string(),
            vec![EnvEntry::Single("global/SHARED".into())],
        );
        global.insert(
            "global-only".to_string(),
            vec![EnvEntry::Single("global/ONLY".into())],
        );

        let mut project = BTreeMap::new();
        project.insert(
            "shared".to_string(),
            vec![EnvEntry::Single("project/SHARED".into())],
        );

        let merged = merge_envs(&global, Some(&project));
        assert!(matches!(
            &merged["shared"][0],
            EnvEntry::Single(path) if path == "project/SHARED"
        ));
        assert!(merged.contains_key("global-only"));
    }

    #[test]
    fn config_validate_rejects_bad_env_label() {
        let mut cfg = Config::default();
        cfg.envs
            .insert("foo/*/bar".into(), vec![EnvEntry::Single("x".into())]);
        assert!(cfg.validate().is_err());
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

    // ── Tag selector entries ───────────────────────────────────────────

    #[test]
    fn env_entry_deserialize_tag_string_form() {
        // `- tag:pci` — bare string with the `tag:` prefix.
        let e: EnvEntry = serde_yaml::from_str("\"tag:pci\"").unwrap();
        assert!(matches!(e, EnvEntry::Tag(ref t) if t == "pci"));
    }

    #[test]
    fn env_entry_deserialize_tag_map_form() {
        // `- { tag: stripe }` — map whose literal key is `tag`.
        let e: EnvEntry = serde_yaml::from_str("{tag: stripe}").unwrap();
        assert!(matches!(e, EnvEntry::Tag(ref t) if t == "stripe"));
    }

    #[test]
    fn env_entry_deserialize_alias_tag_map_form() {
        // `- { STRIPE: tag:stripe }` — alias whose value is a `tag:` selector.
        let e: EnvEntry = serde_yaml::from_str("{STRIPE: \"tag:stripe\"}").unwrap();
        match e {
            EnvEntry::AliasTag { key, tag } => {
                assert_eq!(key, "STRIPE");
                assert_eq!(tag, "stripe");
            }
            other => panic!("expected AliasTag, got {other:?}"),
        }
    }

    #[test]
    fn env_entry_round_trip_tag_variants() {
        // Tag → string form `tag:foo`, round-trip preserves the variant.
        let e = EnvEntry::Tag("pci".into());
        let s = serde_yaml::to_string(&e).unwrap();
        assert_eq!(s.trim(), "tag:pci");
        let back: EnvEntry = serde_yaml::from_str(&s).unwrap();
        assert!(matches!(back, EnvEntry::Tag(ref t) if t == "pci"));

        // AliasTag → map form `{ STRIPE: tag:stripe }`, round-trip preserves
        // the variant (not lowered to a plain Alias).
        let e = EnvEntry::AliasTag {
            key: "STRIPE".into(),
            tag: "stripe".into(),
        };
        let s = serde_yaml::to_string(&e).unwrap();
        let back: EnvEntry = serde_yaml::from_str(&s).unwrap();
        assert!(
            matches!(back, EnvEntry::AliasTag { ref key, ref tag } if key == "STRIPE" && tag == "stripe")
        );
    }

    #[test]
    fn env_entry_rejects_invalid_tag_grammar_in_string_form() {
        // Whitespace is forbidden by the tag grammar.
        let err = serde_yaml::from_str::<EnvEntry>("\"tag:bad tag\"").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid character"), "msg: {msg}");
    }

    #[test]
    fn env_entry_rejects_invalid_tag_grammar_in_map_form() {
        // `{ tag: "bad tag" }` — same grammar check, alternate shape.
        let err = serde_yaml::from_str::<EnvEntry>("{tag: \"bad tag\"}").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid character"), "msg: {msg}");
    }

    #[test]
    fn env_entry_rejects_invalid_tag_in_alias_value() {
        // `{ STRIPE: "tag:bad tag" }` — invalid tag inside alias-rename form
        // also fails at parse time, not later in resolve.
        let err = serde_yaml::from_str::<EnvEntry>("{STRIPE: \"tag:bad tag\"}").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid character"), "msg: {msg}");
    }

    #[test]
    fn validate_envs_accepts_tag_entries_in_concrete_label() {
        // Tag entries skip the capture-ref check entirely (no `$1` to bind).
        let mut envs = BTreeMap::new();
        envs.insert(
            "dev".into(),
            vec![
                EnvEntry::Tag("pci".into()),
                EnvEntry::AliasTag {
                    key: "STRIPE".into(),
                    tag: "stripe".into(),
                },
            ],
        );
        validate_envs(&envs).unwrap();
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
        assert!(err.to_string().contains("$1"), "unexpected error: {err}");
    }

    #[test]
    fn validate_envs_rejects_bad_label() {
        let mut envs = BTreeMap::new();
        envs.insert("foo/*/bar".to_string(), vec![EnvEntry::Single("x".into())]);
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

        assert_eq!(cfg.recipients_path.as_deref(), Some("keys/recipients"));
    }
}
