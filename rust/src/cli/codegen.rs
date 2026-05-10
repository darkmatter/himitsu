use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use clap::Args;
use tracing::{debug, info};

use super::Context;
use crate::config::{self, env_resolver, validate_env_label};
use crate::error::{HimitsuError, Result};
use crate::proto::{self, CodegenLang};

/// Generate typed config code *or* an encrypted `<env>.sops.yaml` file from
/// the resolved preset envs in the project config.
///
/// There are two distinct modes, selected by which arguments are provided:
///
/// - **Sops mode** (`himitsu codegen <env>`): takes a bare env label (that
///   passes [`validate_env_label`]), resolves it against `cfg.envs` +
///   store contents, and emits `<env>.sops.yaml` (or `--output` override)
///   encrypted via the local `sops` CLI.
/// - **Language mode** (`himitsu codegen --lang <ts|go|py|rust> [--env ...]`):
///   the legacy behaviour — scans the store for key names and emits a typed
///   stub file for the chosen language. Written to `--output` / the path
///   configured in `.himitsu.yaml`, or printed with `--stdout`.
///
/// When invoked with no positional and no `--lang`, falls back to reading
/// the codegen section of the project `.himitsu.yaml` just like before.
#[derive(Debug, Args)]
pub struct CodegenArgs {
    /// Env label to materialize as an encrypted `<env>.sops.yaml` file.
    /// Triggers sops mode when present (and `--lang` is not set).
    /// Examples: `foo`, `foo/bar`, `foo/*`.
    #[arg(value_name = "ENV")]
    pub env_positional: Option<String>,

    /// Target language (typescript, golang, python, rust). Overrides .himitsu.yaml.
    #[arg(long)]
    pub lang: Option<String>,

    /// Output file path. Overrides the default derived from the env label
    /// (sops mode) or the `.himitsu.yaml` codegen path (language mode).
    #[arg(long, short)]
    pub output: Option<String>,

    /// Language mode only: environment to narrow the generated key set to
    /// (e.g. "prod", "dev"). If omitted, emits the union across all envs.
    #[arg(long)]
    pub env: Option<String>,

    /// Language mode only: print generated code to stdout instead of writing.
    #[arg(long, default_value_t = false)]
    pub stdout: bool,

    /// Language mode only: merge the "common" env's keys with the target env.
    #[arg(long, default_value_t = true)]
    pub merge_common: bool,
}

/// Discovered secret metadata gathered by scanning the store.
#[derive(Debug, Clone)]
struct SecretInventory {
    /// All environments found (e.g. ["common", "dev", "prod"]).
    environments: BTreeSet<String>,
    /// Keys grouped by environment: env → sorted key names.
    keys_by_env: BTreeMap<String, BTreeSet<String>>,
    /// Global set of all key names across every environment.
    all_keys: BTreeSet<String>,
}

pub fn run(args: CodegenArgs, ctx: &Context) -> Result<()> {
    // Dispatch: sops mode (positional env, no --lang) vs language mode.
    if let Some(ref label) = args.env_positional {
        if args.lang.is_none() {
            return run_sops(label, args.output.as_deref(), ctx);
        }
    }

    // 1. Resolve language and output path from CLI flags or project config.
    let (lang, output_path) = resolve_config(&args, ctx)?;

    debug!(
        "codegen: lang={}, output={:?}, env={:?}",
        proto::codegen_lang_to_str(lang),
        output_path,
        args.env,
    );

    // 2. Scan the store for environments and key names.
    let inventory = scan_store(&ctx.store)?;

    if inventory.all_keys.is_empty() {
        return Err(HimitsuError::InvalidConfig(
            "no secrets found in store — nothing to generate".into(),
        ));
    }

    info!(
        "found {} keys across {} environments",
        inventory.all_keys.len(),
        inventory.environments.len(),
    );

    // 3. Compute the effective key set for the requested environment.
    let effective_keys = effective_keys(&inventory, args.env.as_deref(), args.merge_common);

    // 4. Generate code.
    let code = generate(lang, &inventory, &effective_keys, args.env.as_deref())?;

    // 5. Emit.
    if args.stdout {
        println!("{code}");
    } else if let Some(dest) = output_path {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, code.as_bytes())?;
        println!("wrote {} ({} bytes)", dest.display(), code.len());
    } else {
        println!("{code}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Sops mode: `himitsu codegen <env>` → `<env>.sops.yaml`
// ---------------------------------------------------------------------------

/// Resolve `label` against the project config's `envs` map, decrypt every
/// leaf via the active store, write plaintext YAML with the AUTO-GENERATED
/// banner, then shell out to `sops --encrypt --in-place <path>`.
fn run_sops(label: &str, output_override: Option<&str>, ctx: &Context) -> Result<()> {
    // Validate the label up front so we can give a precise error before
    // loading any config. `resolve` will also validate — this is defensive.
    validate_env_label(label)?;

    // Load project config — sops mode needs `cfg.envs` to resolve the label.
    let (project_cfg, _cfg_path) = config::load_project_config().ok_or_else(|| {
        HimitsuError::ProjectConfigRequired(
            "no project config found (himitsu.yaml); codegen <env> needs an `envs:` map".into(),
        )
    })?;

    if !project_cfg.envs.contains_key(label) {
        return Err(HimitsuError::InvalidConfig(format!("unknown env: {label}")));
    }

    // Enumerate store secrets so the resolver can expand wildcards/globs.
    let secrets = crate::remote::store::list_secrets(&ctx.store, None)?;

    // Resolve into the nested EnvNode tree.
    let tree = env_resolver::resolve(&project_cfg.envs, label, &secrets)?;

    // Walk the tree and decrypt each Leaf into a plaintext YAML value.
    let yaml_tree = materialize_tree(&tree, ctx)?;

    // Serialize with the AUTO-GENERATED banner on top.
    let body = serde_yaml::to_string(&yaml_tree)?;
    let mut out = gen_header("#", Some(label));
    out.push_str(&body);

    // Determine the output path (default: `<label>`.sops.yaml with `/`→`-`
    // and trailing `/*` stripped).
    let output_path = output_override
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default_sops_output_name(label)));

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&output_path, out.as_bytes())?;
    debug!("wrote plaintext to {}", output_path.display());

    // Invoke sops to encrypt in place. The user's `.sops.yaml` rules file
    // supplies the recipients; we don't pass `--age`/`--kms` ourselves.
    encrypt_with_sops(&output_path)?;

    println!("wrote {}", output_path.display());
    Ok(())
}

/// Derive the default output file name from an env label.
///
/// - `foo` → `foo.sops.yaml`
/// - `foo/bar` → `foo-bar.sops.yaml`
/// - `foo/*` → `foo.sops.yaml` (strip trailing `/*` first)
/// - `foo/bar/*` → `foo-bar.sops.yaml`
fn default_sops_output_name(label: &str) -> String {
    let trimmed = label.strip_suffix("/*").unwrap_or(label);
    format!("{}.sops.yaml", trimmed.replace('/', "-"))
}

/// Walk an [`EnvNode`] tree and decrypt every `Leaf` into its plaintext
/// string, producing a parallel [`serde_yaml::Value`] tree. `Branch`es
/// become YAML mappings; `Leaf`s become YAML strings.
fn materialize_tree(node: &env_resolver::EnvNode, ctx: &Context) -> Result<serde_yaml::Value> {
    match node {
        env_resolver::EnvNode::Leaf { secret_path } => {
            let bytes = crate::cli::get::get_plaintext(ctx, secret_path)?;
            let s = String::from_utf8(bytes).map_err(|e| {
                HimitsuError::DecryptionFailed(format!("non-UTF-8 secret at '{secret_path}': {e}"))
            })?;
            Ok(serde_yaml::Value::String(s))
        }
        env_resolver::EnvNode::Branch(map) => {
            let mut m = serde_yaml::Mapping::with_capacity(map.len());
            for (k, child) in map {
                m.insert(
                    serde_yaml::Value::String(k.clone()),
                    materialize_tree(child, ctx)?,
                );
            }
            Ok(serde_yaml::Value::Mapping(m))
        }
    }
}

/// Shell out to `sops --encrypt --in-place <path>`. Maps missing binary to
/// an actionable error and bubbles sops stderr on non-zero exit.
fn encrypt_with_sops(path: &Path) -> Result<()> {
    let output = StdCommand::new("sops")
        .args(["--encrypt", "--in-place"])
        .arg(path)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HimitsuError::External(
                    "sops not found on PATH; install sops from getsops.io".into(),
                )
            } else {
                HimitsuError::External(format!("failed to launch sops: {e}"))
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HimitsuError::External(format!(
            "sops --encrypt failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Config resolution
// ---------------------------------------------------------------------------

/// Local project config for codegen (kept separate from the global Config).
#[derive(Debug, serde::Deserialize, Default)]
struct ProjectConfig {
    #[serde(default)]
    codegen: Option<LocalCodegenConfig>,
}

#[derive(Debug, serde::Deserialize)]
struct LocalCodegenConfig {
    lang: String,
    path: String,
}

/// Resolve the target language and output path.
///
/// CLI flags take precedence. If not provided, fall back to the project's
/// `.himitsu.yaml` codegen section.
fn resolve_config(args: &CodegenArgs, ctx: &Context) -> Result<(CodegenLang, Option<PathBuf>)> {
    // Try loading project config from the git root.
    let project_codegen = ctx.git_root().and_then(|root| {
        let cfg_path = root.join(".himitsu.yaml");
        std::fs::read_to_string(&cfg_path)
            .ok()
            .and_then(|s| serde_yaml::from_str::<ProjectConfig>(&s).ok())
            .and_then(|c| c.codegen)
    });

    let lang = if let Some(ref lang_str) = args.lang {
        let l = proto::codegen_lang_from_str(lang_str);
        if l == CodegenLang::Unspecified {
            return Err(HimitsuError::InvalidConfig(format!(
                "unsupported codegen language: {lang_str}"
            )));
        }
        l
    } else if let Some(ref pc) = project_codegen {
        let l = proto::codegen_lang_from_str(&pc.lang);
        if l == CodegenLang::Unspecified {
            return Err(HimitsuError::InvalidConfig(format!(
                "unsupported codegen language in .himitsu.yaml: {}",
                pc.lang
            )));
        }
        l
    } else {
        return Err(HimitsuError::InvalidConfig(
            "codegen language not specified: use --lang or set codegen.lang in .himitsu.yaml"
                .into(),
        ));
    };

    let output_path = if let Some(ref out) = args.output {
        Some(PathBuf::from(out))
    } else if let Some(ref pc) = project_codegen {
        ctx.git_root().map(|root| root.join(&pc.path))
    } else {
        None
    };

    Ok((lang, output_path))
}

// ---------------------------------------------------------------------------
// Store scanning
// ---------------------------------------------------------------------------

/// Scan the store's `.himitsu/secrets/` directory to discover environments and key names.
fn scan_store(store: &Path) -> Result<SecretInventory> {
    let mut inventory = SecretInventory {
        environments: BTreeSet::new(),
        keys_by_env: BTreeMap::new(),
        all_keys: BTreeSet::new(),
    };

    let paths = crate::remote::store::list_secrets(store, None)?;
    if paths.is_empty() {
        debug!("no secrets in store at {}", store.display());
        return Ok(inventory);
    }

    for path in &paths {
        // Parse "env/key" or "key" from secret path
        if let Some((env, key)) = path.split_once('/') {
            inventory.environments.insert(env.to_string());
            inventory
                .keys_by_env
                .entry(env.to_string())
                .or_default()
                .insert(key.to_string());
            inventory.all_keys.insert(key.to_string());
        } else {
            // Top-level key with no env prefix
            inventory.all_keys.insert(path.to_string());
        }
    }

    Ok(inventory)
}

/// Compute the effective set of keys for the given environment.
///
/// If `merge_common` is true, the "common" environment's keys are included
/// first, then the target env's keys override / extend.
fn effective_keys(
    inventory: &SecretInventory,
    env: Option<&str>,
    merge_common: bool,
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();

    if merge_common {
        if let Some(common) = inventory.keys_by_env.get("common") {
            keys.extend(common.iter().cloned());
        }
    }

    match env {
        Some(e) => {
            if let Some(env_keys) = inventory.keys_by_env.get(e) {
                keys.extend(env_keys.iter().cloned());
            }
        }
        None => {
            // No env filter → union of all keys.
            keys.extend(inventory.all_keys.iter().cloned());
        }
    }

    keys
}

// ---------------------------------------------------------------------------
// Code generation dispatch
// ---------------------------------------------------------------------------

fn generate(
    lang: CodegenLang,
    inventory: &SecretInventory,
    keys: &BTreeSet<String>,
    env: Option<&str>,
) -> Result<String> {
    match lang {
        CodegenLang::Typescript => Ok(gen_typescript(inventory, keys, env)),
        CodegenLang::Golang => Ok(gen_golang(inventory, keys, env)),
        CodegenLang::Python => Ok(gen_python(inventory, keys, env)),
        CodegenLang::Rust => Ok(gen_rust(inventory, keys, env)),
        CodegenLang::Unspecified => Err(HimitsuError::InvalidConfig(
            "codegen language not specified".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// TypeScript
// ---------------------------------------------------------------------------

fn gen_typescript(
    inventory: &SecretInventory,
    keys: &BTreeSet<String>,
    env: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str(&gen_header("//", env));

    // Environment union type
    let envs: Vec<&str> = inventory.environments.iter().map(|s| s.as_str()).collect();
    out.push_str("/** All environments discovered in the secret store. */\n");
    out.push_str("export type HimitsuEnvironment =\n");
    for (i, e) in envs.iter().enumerate() {
        if i < envs.len() - 1 {
            out.push_str(&format!("  | \"{e}\"\n"));
        } else {
            out.push_str(&format!("  | \"{e}\";\n"));
        }
    }
    out.push('\n');

    // Key union type
    out.push_str("/** All secret key names. */\n");
    out.push_str("export type HimitsuKey =\n");
    let keys_vec: Vec<&String> = keys.iter().collect();
    for (i, k) in keys_vec.iter().enumerate() {
        if i < keys_vec.len() - 1 {
            out.push_str(&format!("  | \"{k}\"\n"));
        } else {
            out.push_str(&format!("  | \"{k}\";\n"));
        }
    }
    out.push('\n');

    // Interface with each key as a property
    out.push_str("/** Typed interface for secret values. */\n");
    out.push_str("export interface HimitsuSecrets {\n");
    for key in keys {
        out.push_str(&format!("  readonly {}: string;\n", to_camel_case(key)));
    }
    out.push_str("}\n\n");

    // Constant array of key names
    out.push_str("/** All secret key names as a constant array. */\n");
    out.push_str("export const HIMITSU_KEYS = [\n");
    for key in keys {
        out.push_str(&format!("  \"{key}\",\n"));
    }
    out.push_str("] as const satisfies readonly HimitsuKey[];\n\n");

    // Per-environment key sets
    out.push_str("/** Secret keys available in each environment. */\n");
    out.push_str(
        "export const HIMITSU_KEYS_BY_ENV: Record<HimitsuEnvironment, readonly string[]> = {\n",
    );
    for e in &envs {
        let env_keys = inventory.keys_by_env.get(*e);
        out.push_str(&format!("  \"{e}\": [\n"));
        if let Some(ek) = env_keys {
            for k in ek {
                out.push_str(&format!("    \"{k}\",\n"));
            }
        }
        out.push_str("  ],\n");
    }
    out.push_str("};\n");

    out
}

// ---------------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------------

fn gen_golang(inventory: &SecretInventory, keys: &BTreeSet<String>, env: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&gen_header("//", env));
    out.push_str("package secrets\n\n");

    // Environment constants
    out.push_str("// Environments discovered in the secret store.\n");
    out.push_str("const (\n");
    for e in &inventory.environments {
        let const_name = format!("Env{}", to_pascal_case(e));
        out.push_str(&format!("\t{const_name} = \"{e}\"\n"));
    }
    out.push_str(")\n\n");

    // Key constants
    out.push_str("// Secret key name constants.\n");
    out.push_str("const (\n");
    for key in keys {
        let const_name = format!("Key{}", to_pascal_case(key));
        out.push_str(&format!("\t{const_name} = \"{key}\"\n"));
    }
    out.push_str(")\n\n");

    // Struct
    out.push_str("// HimitsuSecrets holds typed secret values.\n");
    out.push_str("type HimitsuSecrets struct {\n");
    for key in keys {
        let field_name = to_pascal_case(key);
        out.push_str(&format!(
            "\t{field_name} string `json:\"{key}\" yaml:\"{key}\"`\n"
        ));
    }
    out.push_str("}\n\n");

    // AllKeys slice
    out.push_str("// AllKeys contains every secret key name.\n");
    out.push_str("var AllKeys = []string{\n");
    for key in keys {
        out.push_str(&format!("\t\"{key}\",\n"));
    }
    out.push_str("}\n");

    out
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

fn gen_python(inventory: &SecretInventory, keys: &BTreeSet<String>, env: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&gen_header("#", env));

    out.push_str("from __future__ import annotations\n\n");
    out.push_str("from dataclasses import dataclass\n");
    out.push_str("from enum import Enum\n");
    out.push_str("from typing import ClassVar, FrozenSet\n\n\n");

    // Environment enum
    out.push_str("class HimitsuEnvironment(str, Enum):\n");
    out.push_str("    \"\"\"Environments discovered in the secret store.\"\"\"\n\n");
    for e in &inventory.environments {
        let member = e.to_uppercase();
        out.push_str(&format!("    {member} = \"{e}\"\n"));
    }
    out.push_str("\n\n");

    // Dataclass
    out.push_str("@dataclass(frozen=True)\n");
    out.push_str("class HimitsuSecrets:\n");
    out.push_str("    \"\"\"Typed secret values.\"\"\"\n\n");
    for key in keys {
        let field = key.to_lowercase();
        out.push_str(&format!("    {field}: str\n"));
    }
    out.push('\n');

    // Class-level constant with all key names
    out.push_str("    ALL_KEYS: ClassVar[FrozenSet[str]] = frozenset({\n");
    for key in keys {
        out.push_str(&format!("        \"{key}\",\n"));
    }
    out.push_str("    })\n\n\n");

    // Per-environment key sets
    out.push_str("KEYS_BY_ENV: dict[HimitsuEnvironment, frozenset[str]] = {\n");
    for e in &inventory.environments {
        let member = e.to_uppercase();
        let env_keys = inventory.keys_by_env.get(e.as_str());
        out.push_str(&format!("    HimitsuEnvironment.{member}: frozenset({{\n"));
        if let Some(ek) = env_keys {
            for k in ek {
                out.push_str(&format!("        \"{k}\",\n"));
            }
        }
        out.push_str("    }),\n");
    }
    out.push_str("}\n");

    out
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

fn gen_rust(inventory: &SecretInventory, keys: &BTreeSet<String>, env: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&gen_header("//", env));
    out.push_str("#![allow(dead_code)]\n\n");

    // Environment enum
    out.push_str("/// Environments discovered in the secret store.\n");
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n");
    out.push_str("pub enum HimitsuEnvironment {\n");
    for e in &inventory.environments {
        let variant = to_pascal_case(e);
        out.push_str(&format!("    /// `{e}`\n"));
        out.push_str(&format!("    {variant},\n"));
    }
    out.push_str("}\n\n");

    out.push_str("impl HimitsuEnvironment {\n");
    out.push_str("    /// The canonical string name of this environment.\n");
    out.push_str("    pub const fn as_str(&self) -> &'static str {\n");
    out.push_str("        match self {\n");
    for e in &inventory.environments {
        let variant = to_pascal_case(e);
        out.push_str(&format!("            Self::{variant} => \"{e}\",\n"));
    }
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    // Key enum
    out.push_str("/// Secret key names.\n");
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n");
    out.push_str("pub enum HimitsuKey {\n");
    for key in keys {
        let variant = to_pascal_case(key);
        out.push_str(&format!("    /// `{key}`\n"));
        out.push_str(&format!("    {variant},\n"));
    }
    out.push_str("}\n\n");

    out.push_str("impl HimitsuKey {\n");
    out.push_str("    /// The canonical string name of this key.\n");
    out.push_str("    pub const fn as_str(&self) -> &'static str {\n");
    out.push_str("        match self {\n");
    for key in keys {
        let variant = to_pascal_case(key);
        out.push_str(&format!("            Self::{variant} => \"{key}\",\n"));
    }
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");

    // Struct
    out.push_str("/// Typed container for secret values.\n");
    out.push_str("pub struct HimitsuSecrets {\n");
    for key in keys {
        let field = key.to_lowercase();
        out.push_str(&format!("    /// `{key}`\n"));
        out.push_str(&format!("    pub {field}: String,\n"));
    }
    out.push_str("}\n\n");

    // ALL_KEYS constant
    out.push_str("/// All secret key names.\n");
    out.push_str(&format!(
        "pub const ALL_KEYS: [HimitsuKey; {}] = [\n",
        keys.len()
    ));
    for key in keys {
        let variant = to_pascal_case(key);
        out.push_str(&format!("    HimitsuKey::{variant},\n"));
    }
    out.push_str("];\n\n");

    // ALL_KEY_NAMES constant
    out.push_str("/// All secret key names as string slices.\n");
    out.push_str(&format!(
        "pub const ALL_KEY_NAMES: [&str; {}] = [\n",
        keys.len()
    ));
    for key in keys {
        out.push_str(&format!("    \"{key}\",\n"));
    }
    out.push_str("];\n");

    out
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Common header banner placed at the top of every generated file.
fn gen_header(comment: &str, env: Option<&str>) -> String {
    let env_note = match env {
        Some(e) => format!(" (environment: {e})"),
        None => " (all environments)".to_string(),
    };
    format!(
        "{comment} =============================================================================\n\
         {comment} AUTO-GENERATED by `himitsu codegen` — do not edit manually.\n\
         {comment}{env_note}\n\
         {comment} =============================================================================\n\n",
    )
}

/// Convert `SCREAMING_SNAKE_CASE` or `snake_case` to `PascalCase`.
fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => {
                    let mut word = c.to_uppercase().to_string();
                    word.extend(chars.map(|c| c.to_ascii_lowercase()));
                    word
                }
                None => String::new(),
            }
        })
        .collect()
}

/// Convert `SCREAMING_SNAKE_CASE` to `camelCase`.
fn to_camel_case(s: &str) -> String {
    let pascal = to_pascal_case(s);
    let mut chars = pascal.chars();
    match chars.next() {
        Some(c) => {
            let mut out = c.to_lowercase().to_string();
            out.extend(chars);
            out
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helper tests --

    #[test]
    fn to_pascal_case_screaming_snake() {
        assert_eq!(to_pascal_case("STRIPE_KEY"), "StripeKey");
        assert_eq!(to_pascal_case("DB_PASS"), "DbPass");
        assert_eq!(to_pascal_case("A"), "A");
        assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
    }

    #[test]
    fn to_camel_case_screaming_snake() {
        assert_eq!(to_camel_case("STRIPE_KEY"), "stripeKey");
        assert_eq!(to_camel_case("DB_PASS"), "dbPass");
        assert_eq!(to_camel_case("API_URL"), "apiUrl");
    }

    #[test]
    fn to_pascal_case_handles_empty_parts() {
        assert_eq!(to_pascal_case("__FOO__BAR__"), "FooBar");
        assert_eq!(to_pascal_case(""), "");
    }

    // -- Scanning tests --

    /// Helper: populate `<tmp>/.himitsu/secrets/<env>/<key>.age`.
    fn make_store(tmp: &Path, envs: &[(&str, &[&str])]) {
        let secrets = crate::remote::store::secrets_dir(tmp);
        for (env, keys) in envs {
            let env_dir = secrets.join(env);
            std::fs::create_dir_all(&env_dir).unwrap();
            for key in *keys {
                std::fs::write(env_dir.join(format!("{key}.age")), b"cipher").unwrap();
            }
        }
    }

    #[test]
    fn scan_store_discovers_envs_and_keys() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(
            tmp.path(),
            &[
                ("common", &["API_URL"]),
                ("prod", &["API_URL", "DB_PASS"]),
                ("dev", &["API_URL", "DEBUG_TOKEN"]),
            ],
        );

        let inv = scan_store(tmp.path()).unwrap();
        assert_eq!(
            inv.environments,
            BTreeSet::from(["common".into(), "dev".into(), "prod".into(),])
        );
        assert_eq!(inv.all_keys.len(), 3); // API_URL, DB_PASS, DEBUG_TOKEN
        assert_eq!(inv.keys_by_env["prod"].len(), 2);
    }

    #[test]
    fn scan_store_empty_returns_empty_inventory() {
        let tmp = tempfile::tempdir().unwrap();
        let inv = scan_store(tmp.path()).unwrap();
        assert!(inv.environments.is_empty());
        assert!(inv.all_keys.is_empty());
    }

    // -- Effective keys --

    #[test]
    fn effective_keys_merges_common() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(
            tmp.path(),
            &[("common", &["SHARED_KEY"]), ("prod", &["PROD_KEY"])],
        );
        let inv = scan_store(tmp.path()).unwrap();

        let keys = effective_keys(&inv, Some("prod"), true);
        assert!(keys.contains("SHARED_KEY"));
        assert!(keys.contains("PROD_KEY"));
    }

    #[test]
    fn effective_keys_no_merge() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(
            tmp.path(),
            &[("common", &["SHARED_KEY"]), ("prod", &["PROD_KEY"])],
        );
        let inv = scan_store(tmp.path()).unwrap();

        let keys = effective_keys(&inv, Some("prod"), false);
        assert!(!keys.contains("SHARED_KEY"));
        assert!(keys.contains("PROD_KEY"));
    }

    #[test]
    fn effective_keys_no_env_returns_all() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(
            tmp.path(),
            &[("common", &["A"]), ("prod", &["B"]), ("dev", &["C"])],
        );
        let inv = scan_store(tmp.path()).unwrap();

        let keys = effective_keys(&inv, None, true);
        assert_eq!(keys.len(), 3);
    }

    // -- TypeScript generation --

    #[test]
    fn gen_typescript_produces_valid_output() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(tmp.path(), &[("prod", &["STRIPE_KEY", "DB_PASS"])]);
        let inv = scan_store(tmp.path()).unwrap();

        let code = gen_typescript(&inv, &inv.all_keys, Some("prod"));
        assert!(code.contains("AUTO-GENERATED"));
        assert!(code.contains("export type HimitsuEnvironment ="));
        assert!(code.contains("\"prod\""));
        assert!(code.contains("export interface HimitsuSecrets"));
        assert!(code.contains("readonly stripeKey: string;"));
        assert!(code.contains("readonly dbPass: string;"));
        assert!(code.contains("export const HIMITSU_KEYS = ["));
        assert!(code.contains("\"STRIPE_KEY\""));
        assert!(code.contains("\"DB_PASS\""));
        assert!(code.contains("as const satisfies readonly HimitsuKey[]"));
    }

    #[test]
    fn gen_typescript_includes_env_keys_map() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(
            tmp.path(),
            &[("common", &["SHARED"]), ("prod", &["PROD_ONLY"])],
        );
        let inv = scan_store(tmp.path()).unwrap();

        let code = gen_typescript(&inv, &inv.all_keys, None);
        assert!(code.contains("HIMITSU_KEYS_BY_ENV"));
        assert!(code.contains("\"common\""));
        assert!(code.contains("\"SHARED\""));
    }

    // -- Go generation --

    #[test]
    fn gen_golang_produces_valid_output() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(tmp.path(), &[("prod", &["API_KEY"])]);
        let inv = scan_store(tmp.path()).unwrap();

        let code = gen_golang(&inv, &inv.all_keys, Some("prod"));
        assert!(code.contains("package secrets"));
        assert!(code.contains("EnvProd"));
        assert!(code.contains("KeyApiKey"));
        assert!(code.contains("type HimitsuSecrets struct"));
        assert!(code.contains("ApiKey string `json:\"API_KEY\" yaml:\"API_KEY\"`"));
        assert!(code.contains("var AllKeys = []string{"));
    }

    // -- Python generation --

    #[test]
    fn gen_python_produces_valid_output() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(tmp.path(), &[("dev", &["TOKEN", "SECRET"])]);
        let inv = scan_store(tmp.path()).unwrap();

        let code = gen_python(&inv, &inv.all_keys, Some("dev"));
        assert!(code.contains("from dataclasses import dataclass"));
        assert!(code.contains("class HimitsuEnvironment(str, Enum):"));
        assert!(code.contains("DEV = \"dev\""));
        assert!(code.contains("@dataclass(frozen=True)"));
        assert!(code.contains("class HimitsuSecrets:"));
        assert!(code.contains("secret: str"));
        assert!(code.contains("token: str"));
        assert!(code.contains("ALL_KEYS: ClassVar[FrozenSet[str]]"));
        assert!(code.contains("KEYS_BY_ENV"));
    }

    // -- Rust generation --

    #[test]
    fn gen_rust_produces_valid_output() {
        let tmp = tempfile::tempdir().unwrap();
        make_store(tmp.path(), &[("staging", &["DB_URL", "REDIS_URL"])]);
        let inv = scan_store(tmp.path()).unwrap();

        let code = gen_rust(&inv, &inv.all_keys, Some("staging"));
        assert!(code.contains("#![allow(dead_code)]"));
        assert!(code.contains("pub enum HimitsuEnvironment"));
        assert!(code.contains("Staging,"));
        assert!(code.contains("pub enum HimitsuKey"));
        assert!(code.contains("DbUrl,"));
        assert!(code.contains("RedisUrl,"));
        assert!(code.contains("pub struct HimitsuSecrets"));
        assert!(code.contains("pub db_url: String,"));
        assert!(code.contains("pub const ALL_KEYS: [HimitsuKey; 2]"));
        assert!(code.contains("pub const ALL_KEY_NAMES: [&str; 2]"));
    }

    // -- Header --

    #[test]
    fn gen_header_includes_env_note() {
        let h = gen_header("//", Some("prod"));
        assert!(h.contains("(environment: prod)"));

        let h2 = gen_header("#", None);
        assert!(h2.contains("(all environments)"));
    }

    // -- Full run via CLI with --stdout --

    #[test]
    fn run_with_stdout_flag_and_explicit_lang() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().to_path_buf();
        // New layout: .himitsu/secrets/prod/MY_SECRET.age
        let secrets = crate::remote::store::secrets_dir(&store).join("prod");
        std::fs::create_dir_all(&secrets).unwrap();
        std::fs::write(secrets.join("MY_SECRET.age"), b"ciphertext").unwrap();

        let ctx = Context {
            data_dir: tmp.path().join("share"),
            state_dir: tmp.path().join("state"),
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };

        let args = CodegenArgs {
            env_positional: None,
            lang: Some("typescript".into()),
            output: None,
            env: Some("prod".into()),
            stdout: true,
            merge_common: true,
        };

        // Should succeed (prints to stdout).
        let result = run(args, &ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn run_fails_on_empty_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().to_path_buf();
        std::fs::create_dir_all(&store).unwrap();

        let ctx = Context {
            data_dir: tmp.path().join("share"),
            state_dir: tmp.path().join("state"),
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };

        let args = CodegenArgs {
            env_positional: None,
            lang: Some("typescript".into()),
            output: None,
            env: None,
            stdout: true,
            merge_common: true,
        };

        let result = run(args, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn run_fails_on_unknown_language() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().to_path_buf();
        let secrets = crate::remote::store::secrets_dir(&store).join("prod");
        std::fs::create_dir_all(&secrets).unwrap();
        std::fs::write(secrets.join("X.age"), b"cipher").unwrap();

        let ctx = Context {
            data_dir: tmp.path().join("share"),
            state_dir: tmp.path().join("state"),
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };

        let args = CodegenArgs {
            env_positional: None,
            lang: Some("cobol".into()),
            output: None,
            env: None,
            stdout: true,
            merge_common: true,
        };

        let result = run(args, &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn run_writes_output_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().to_path_buf();
        let secrets = crate::remote::store::secrets_dir(&store).join("prod");
        std::fs::create_dir_all(&secrets).unwrap();
        std::fs::write(secrets.join("TOKEN.age"), b"cipher").unwrap();

        let output = tmp.path().join("generated/secrets.ts");

        let ctx = Context {
            data_dir: tmp.path().join("share"),
            state_dir: tmp.path().join("state"),
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };

        let args = CodegenArgs {
            env_positional: None,
            lang: Some("typescript".into()),
            output: Some(output.to_string_lossy().into()),
            env: Some("prod".into()),
            stdout: false,
            merge_common: true,
        };

        run(args, &ctx).unwrap();
        assert!(output.exists());

        let content = std::fs::read_to_string(&output).unwrap();
        assert!(content.contains("export interface HimitsuSecrets"));
        assert!(content.contains("readonly token: string;"));
    }

    // -- Sops-mode output path derivation --

    #[test]
    fn default_sops_output_name_concrete_top_level() {
        assert_eq!(default_sops_output_name("foo"), "foo.sops.yaml");
    }

    #[test]
    fn default_sops_output_name_nested_concrete() {
        assert_eq!(default_sops_output_name("foo/bar"), "foo-bar.sops.yaml");
        assert_eq!(
            default_sops_output_name("foo/bar/baz"),
            "foo-bar-baz.sops.yaml"
        );
    }

    #[test]
    fn default_sops_output_name_strips_trailing_wildcard() {
        assert_eq!(default_sops_output_name("foo/*"), "foo.sops.yaml");
        assert_eq!(default_sops_output_name("foo/bar/*"), "foo-bar.sops.yaml");
    }

    // -- Tree materialization --

    /// Stubbed-leaf walker with the same shape as `materialize_tree` but
    /// replacing the live decrypt call with a fixed string. Lets us assert
    /// structure without standing up a full store + age identity.
    fn materialize_tree_stub(node: &env_resolver::EnvNode, leaf_value: &str) -> serde_yaml::Value {
        match node {
            env_resolver::EnvNode::Leaf { .. } => serde_yaml::Value::String(leaf_value.to_string()),
            env_resolver::EnvNode::Branch(map) => {
                let mut m = serde_yaml::Mapping::with_capacity(map.len());
                for (k, child) in map {
                    m.insert(
                        serde_yaml::Value::String(k.clone()),
                        materialize_tree_stub(child, leaf_value),
                    );
                }
                serde_yaml::Value::Mapping(m)
            }
        }
    }

    #[test]
    fn materialize_tree_serializes_concrete_env_as_flat_yaml() {
        use crate::config::EnvEntry;
        let mut envs: BTreeMap<String, Vec<EnvEntry>> = BTreeMap::new();
        envs.insert(
            "dev".into(),
            vec![
                EnvEntry::Single("dev/API_KEY".into()),
                EnvEntry::Alias {
                    key: "DB".into(),
                    path: "dev/DB_PASS".into(),
                },
            ],
        );
        let tree = env_resolver::resolve(&envs, "dev", &[]).unwrap();
        let value = materialize_tree_stub(&tree, "PLAINTEXT");

        let yaml = serde_yaml::to_string(&value).unwrap();
        // Shape: dev: { API_KEY: PLAINTEXT, DB: PLAINTEXT }
        assert!(yaml.contains("dev:"), "yaml=\n{yaml}");
        assert!(yaml.contains("API_KEY: PLAINTEXT"), "yaml=\n{yaml}");
        assert!(yaml.contains("DB: PLAINTEXT"), "yaml=\n{yaml}");
    }

    #[test]
    fn materialize_tree_serializes_wildcard_env_as_nested_yaml() {
        use crate::config::EnvEntry;
        let mut envs: BTreeMap<String, Vec<EnvEntry>> = BTreeMap::new();
        envs.insert(
            "foo/*".into(),
            vec![EnvEntry::Alias {
                key: "POSTGRES".into(),
                path: "$1/postgres-url".into(),
            }],
        );
        let secrets = vec!["dev/postgres-url".to_string(), "prod/postgres-url".into()];
        let tree = env_resolver::resolve(&envs, "foo/*", &secrets).unwrap();
        let value = materialize_tree_stub(&tree, "PW");

        let yaml = serde_yaml::to_string(&value).unwrap();
        // Shape: foo: { dev: { POSTGRES: PW }, prod: { POSTGRES: PW } }
        assert!(yaml.contains("foo:"), "yaml=\n{yaml}");
        assert!(yaml.contains("dev:"), "yaml=\n{yaml}");
        assert!(yaml.contains("prod:"), "yaml=\n{yaml}");
        assert!(yaml.contains("POSTGRES: PW"), "yaml=\n{yaml}");
    }

    // -- Unknown env label --

    #[test]
    fn run_sops_unknown_env_label_errors() {
        // Build an isolated project root (doubles as CWD and git root) with
        // a himitsu.yaml that *doesn't* declare the label we'll ask for.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        // .git so the walk stops here; project config discovery still finds
        // himitsu.yaml at this level.
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::write(
            project.join("himitsu.yaml"),
            "envs:\n  dev:\n    - dev/API_KEY\n",
        )
        .unwrap();

        // Chdir into the project so `load_project_config` finds it.
        let saved_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&project).unwrap();

        let ctx = Context {
            data_dir: tmp.path().join("share"),
            state_dir: tmp.path().join("state"),
            store: project.clone(),
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };
        let result = run_sops("ghost", None, &ctx);

        // Always restore CWD before asserting so a panic here doesn't leak.
        std::env::set_current_dir(saved_cwd).unwrap();

        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown env") && msg.contains("ghost"),
            "unexpected error: {msg}"
        );
    }

    // -- Header banner present in plaintext --

    #[test]
    fn sops_plaintext_contains_auto_generated_header() {
        // The plaintext we feed to `sops --encrypt --in-place` is constructed
        // as `gen_header("#", Some(label)) + serde_yaml::to_string(tree)`.
        // Rather than stand up sops + age here, verify the composition rule
        // by re-running the same assembly with a stub tree.
        use crate::config::EnvEntry;
        let mut envs: BTreeMap<String, Vec<EnvEntry>> = BTreeMap::new();
        envs.insert("dev".into(), vec![EnvEntry::Single("dev/API_KEY".into())]);
        let tree = env_resolver::resolve(&envs, "dev", &[]).unwrap();
        let value = materialize_tree_stub(&tree, "XXX");
        let body = serde_yaml::to_string(&value).unwrap();
        let mut out = gen_header("#", Some("dev"));
        out.push_str(&body);

        assert!(out.starts_with("# ="), "plaintext must start with header");
        assert!(out.contains("AUTO-GENERATED"));
        assert!(out.contains("(environment: dev)"));
        // And the body is below the banner.
        let header_end = out.find("\n\n").expect("banner ends with blank line");
        assert!(
            out[header_end..].contains("dev:"),
            "body should follow banner"
        );
    }

    // -- sops shell-out: gated behind #[ignore] (requires sops + keys). --

    #[test]
    #[ignore]
    fn encrypt_with_sops_smoke() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.sops.yaml");
        std::fs::write(&p, "foo: bar\n").unwrap();
        // This will fail without a .sops.yaml rules file + recipients on
        // PATH — the test is ignored by default.
        encrypt_with_sops(&p).unwrap();
    }
}
