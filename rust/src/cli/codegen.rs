use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::Args;
use tracing::{debug, info};

use super::Context;
use crate::error::{HimitsuError, Result};
use crate::proto::{self, CodegenLang};

/// Generate typed config code from secrets.
///
/// When run without arguments, reads language and output path from
/// the project's .himitsu.yaml codegen config.
#[derive(Debug, Args)]
pub struct CodegenArgs {
    /// Target language (typescript, golang, python, rust). Overrides .himitsu.yaml.
    #[arg(long)]
    pub lang: Option<String>,

    /// Output file path. Overrides .himitsu.yaml.
    #[arg(long, short)]
    pub output: Option<String>,

    /// Environment to generate for (e.g. "prod", "dev").
    /// If omitted, generates a union of all environments.
    #[arg(long)]
    pub env: Option<String>,

    /// Print generated code to stdout instead of writing to a file.
    #[arg(long, default_value_t = false)]
    pub stdout: bool,

    /// Include "common" environment keys merged with the target env.
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
// Config resolution
// ---------------------------------------------------------------------------

/// Resolve the target language and output path.
///
/// CLI flags take precedence. If not provided, fall back to the project's
/// `.himitsu.yaml` codegen section.
fn resolve_config(args: &CodegenArgs, ctx: &Context) -> Result<(CodegenLang, Option<PathBuf>)> {
    // Try loading project config from the git root.
    let project_codegen = ctx.git_root().and_then(|root| {
        let cfg_path = root.join(".himitsu.yaml");
        crate::config::Config::load(&cfg_path)
            .ok()
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

/// Scan the store's `vars/` directory to discover environments and key names.
fn scan_store(store: &Path) -> Result<SecretInventory> {
    let vars_dir = store.join("vars");
    let mut inventory = SecretInventory {
        environments: BTreeSet::new(),
        keys_by_env: BTreeMap::new(),
        all_keys: BTreeSet::new(),
    };

    if !vars_dir.exists() {
        debug!("no vars/ directory in store at {}", store.display());
        return Ok(inventory);
    }

    for entry in std::fs::read_dir(&vars_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let env_name = entry.file_name().to_string_lossy().to_string();
        inventory.environments.insert(env_name.clone());

        let mut keys = BTreeSet::new();
        for file in std::fs::read_dir(entry.path())? {
            let file = file?;
            let fname = file.file_name().to_string_lossy().to_string();
            if let Some(key) = fname.strip_suffix(".age") {
                keys.insert(key.to_string());
                inventory.all_keys.insert(key.to_string());
            }
        }
        inventory.keys_by_env.insert(env_name, keys);
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

    fn make_store(tmp: &Path, envs: &[(&str, &[&str])]) {
        let vars = tmp.join("vars");
        for (env, keys) in envs {
            let env_dir = vars.join(env);
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
        let store = tmp.path().join(".himitsu");
        let vars = store.join("vars/prod");
        std::fs::create_dir_all(&vars).unwrap();
        std::fs::write(vars.join("MY_SECRET.age"), b"ciphertext").unwrap();

        let ctx = Context {
            user_home: tmp.path().to_path_buf(),
            store,
        };

        let args = CodegenArgs {
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
        let store = tmp.path().join(".himitsu");
        std::fs::create_dir_all(&store).unwrap();

        let ctx = Context {
            user_home: tmp.path().to_path_buf(),
            store,
        };

        let args = CodegenArgs {
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
        let store = tmp.path().join(".himitsu");
        let vars = store.join("vars/prod");
        std::fs::create_dir_all(&vars).unwrap();
        std::fs::write(vars.join("X.age"), b"cipher").unwrap();

        let ctx = Context {
            user_home: tmp.path().to_path_buf(),
            store,
        };

        let args = CodegenArgs {
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
        let store = tmp.path().join(".himitsu");
        let vars = store.join("vars/prod");
        std::fs::create_dir_all(&vars).unwrap();
        std::fs::write(vars.join("TOKEN.age"), b"cipher").unwrap();

        let output = tmp.path().join("generated/secrets.ts");

        let ctx = Context {
            user_home: tmp.path().to_path_buf(),
            store,
        };

        let args = CodegenArgs {
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
}
