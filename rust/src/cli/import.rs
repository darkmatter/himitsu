use std::collections::HashMap;
use std::io::Write as _;
use std::process::Command;

use clap::Args;
use serde::Deserialize;

use super::Context;
use crate::cli::set::set_plaintext;
use crate::error::{HimitsuError, Result};
use crate::remote::store::secrets_dir;

/// Import secrets from external stores (1Password, SOPS).
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Target secret path, e.g. `prod/STRIPE_KEY`.
    ///
    /// Required for single-field imports (`op://vault/item/field`).
    /// Optional for whole-item or whole-vault imports — field labels are
    /// used as path segments when omitted.
    pub path: Option<String>,

    /// 1Password reference to import. Supports:
    ///   - `op://vault/item/field` — import a single field
    ///   - `op://vault/item` — import all fields from an item
    ///   - `op://vault` — import all items and fields from a vault
    #[arg(long, conflicts_with = "sops")]
    pub op: Option<String>,

    /// Path to a SOPS-encrypted file to import. The file is decrypted via
    /// `sops -d` and each leaf key-value pair is stored as a separate secret
    /// under `<path>/<flattened_key>` (nested keys joined with `/`).
    #[arg(long)]
    pub sops: Option<String>,

    /// Target prefix for imported secrets. `--to prod/stripe` stores keys
    /// under `prod/stripe/<key>`. Trailing slashes are stripped.
    /// Takes precedence over the positional PATH argument for bulk imports.
    #[arg(long = "to")]
    pub to: Option<String>,

    /// Glob filter — only import keys matching this pattern.
    /// Supports `foo/*` (direct children), `foo/**` (recursive),
    /// `*` (all), and exact matches.
    #[arg(long)]
    pub filter: Option<String>,

    /// Overwrite an existing secret at the target path.
    #[arg(long)]
    pub overwrite: bool,

    /// Skip git commit and push (useful for batch imports).
    #[arg(long)]
    pub no_push: bool,

    /// Preview what would be imported without writing anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Keep original key names (SOME_KEY) instead of normalizing to
    /// kebab-case (some-key).
    #[arg(long)]
    pub keep_names: bool,
}

/// A 1Password field from `op item get --format=json`.
#[derive(Debug, Deserialize)]
struct OpField {
    /// The label (display name) of the field.
    #[serde(default)]
    label: String,
    /// The plaintext value; may be absent for section headers or empty fields.
    #[serde(default)]
    value: Option<String>,
    /// Field type, e.g. "CONCEALED", "STRING", "SECTION_HEADER", etc.
    #[serde(default, rename = "type")]
    field_type: String,
}

/// A 1Password item from `op item get --format=json`.
#[derive(Debug, Deserialize)]
struct OpItem {
    /// The item title.
    #[allow(dead_code)]
    #[serde(default)]
    title: String,
    /// The fields on this item.
    #[serde(default)]
    fields: Vec<OpField>,
}

/// A 1Password item summary from `op item list --format=json`.
#[derive(Debug, Deserialize)]
struct OpItemSummary {
    /// The item's unique ID.
    id: String,
    /// The item title (used as a path segment).
    #[serde(default)]
    title: String,
}

/// A single planned import action.
struct ImportAction {
    /// The original key name from the source (used for filter matching).
    source_key: String,
    /// The himitsu path where the secret will be stored.
    target: String,
    /// The plaintext value.
    value: String,
}

pub fn run(args: ImportArgs, ctx: &Context) -> Result<()> {
    if let Some(ref sops_file) = args.sops {
        return run_sops(sops_file, &args, ctx);
    }

    // Plain YAML file import: positional path is a file on disk (no --op/--sops).
    if args.op.is_none() && args.sops.is_none() {
        if let Some(ref file_path) = args.path {
            if std::path::Path::new(file_path).exists() {
                return run_yaml_file(file_path, &args, ctx);
            }
        }
        // Fall through to "missing source" error.
        return Err(HimitsuError::InvalidReference(
            "missing source: pass --op <op://vault/item/field> or --sops <file>".into(),
        ));
    }

    let op_ref = args.op.as_deref().ok_or_else(|| {
        HimitsuError::InvalidReference(
            "missing source: pass --op <op://vault/item/field> or --sops <file>".into(),
        )
    })?;

    // Validate the op reference shape.
    let trimmed = op_ref.strip_prefix("op://").ok_or_else(|| {
        HimitsuError::InvalidReference(format!(
            "1Password reference must start with `op://` (got {op_ref:?})"
        ))
    })?;
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();

    let actions = match segments.len() {
        3 => {
            // vault/item/field — single field import
            let field_name = segments[2];
            let target = if let Some(ref to) = args.to {
                let prefix = to.trim_end_matches('/');
                let leaf = if args.keep_names {
                    sanitize_label(field_name)
                } else {
                    normalize_key_name(&sanitize_label(field_name))
                };
                format!("{prefix}/{leaf}")
            } else if let Some(ref path) = args.path {
                path.clone()
            } else {
                return Err(HimitsuError::InvalidReference(
                    "PATH or --to is required for single-field import (op://vault/item/field)"
                        .into(),
                ));
            };
            if !args.dry_run && !args.overwrite && secret_exists_at(&ctx.store, &target) {
                return Err(HimitsuError::InvalidReference(format!(
                    "secret already exists at {target}: pass --overwrite to replace it",
                )));
            }
            let plaintext = op_read("op", op_ref)?;
            vec![ImportAction {
                source_key: field_name.to_string(),
                target,
                value: plaintext,
            }]
        }
        2 => {
            // vault/item — whole-item import
            let (vault, item) = (segments[0], segments[1]);
            let prefix = effective_prefix(&args, item);
            build_item_actions("op", vault, item, &prefix, op_ref, !args.keep_names)?
        }
        1 => {
            // vault — whole-vault import
            let vault = segments[0];
            let prefix = effective_prefix(&args, vault);
            build_vault_actions("op", vault, &prefix, !args.keep_names)?
        }
        _ => {
            return Err(HimitsuError::InvalidReference(format!(
                "expected op://vault, op://vault/item, or op://vault/item/field — got {op_ref:?}"
            )));
        }
    };

    // Apply glob filter if provided.
    let actions = if let Some(ref filter) = args.filter {
        actions
            .into_iter()
            .filter(|a| matches_filter(&a.source_key, filter))
            .collect()
    } else {
        actions
    };

    if actions.is_empty() {
        println!("No importable fields found in {op_ref}");
        return Ok(());
    }

    // Build the staging YAML content.
    let mut staging = format!(
        "# Imported from {op_ref} — edit paths and values, then save and close\n\
         # Remove lines you don't want to import\n"
    );
    for action in &actions {
        // Escape special YAML characters in the value using serde_yaml.
        let yaml_val =
            serde_yaml::to_string(&action.value).unwrap_or_else(|_| format!("{:?}", action.value));
        let yaml_val = yaml_val.trim_end();
        staging.push_str(&format!("{}: {yaml_val}\n", action.target));
    }

    // If dry-run: show staging content and exit without opening editor or importing.
    if args.dry_run {
        println!("# [dry-run] Staging file that would be opened for editing:\n");
        println!("{staging}");
        println!("# {} secret(s) would be imported", actions.len());
        return Ok(());
    }

    // Write staging file to a temp location.
    let mut tmp = tempfile::Builder::new()
        .suffix(".yaml")
        .tempfile()
        .map_err(|e| HimitsuError::External(format!("failed to create staging file: {e}")))?;
    tmp.write_all(staging.as_bytes())
        .map_err(|e| HimitsuError::External(format!("failed to write staging file: {e}")))?;
    tmp.flush()
        .map_err(|e| HimitsuError::External(format!("failed to flush staging file: {e}")))?;

    // Keep the file alive after NamedTempFile is persisted.
    let (_, tmp_path) = tmp
        .keep()
        .map_err(|e| HimitsuError::External(format!("failed to persist staging file: {e}")))?;

    // Open editor.
    open_editor(&tmp_path)?;

    // Read edited file back.
    let edited = std::fs::read_to_string(&tmp_path).map_err(|e| {
        HimitsuError::External(format!("failed to read staging file after edit: {e}"))
    })?;

    // Clean up temp file (best effort).
    let _ = std::fs::remove_file(&tmp_path);

    // Parse edited YAML.
    let entries = parse_staging_yaml(&edited)?;

    if entries.is_empty() {
        println!("No secrets to import.");
        return Ok(());
    }

    // Check for existing secrets and import.
    let mut count = 0;
    for (path, value) in &entries {
        if !args.overwrite && secret_exists_at(&ctx.store, path) {
            eprintln!("skipping {path}: already exists (use --overwrite to replace)");
            continue;
        }
        let stored = set_plaintext(ctx, path, value.as_bytes(), Vec::new())?;
        println!("Imported {stored}");
        count += 1;
    }
    println!("\n{count} secret(s) imported");
    Ok(())
}

// ── Editor workflow helpers ──────────────────────────────────────────────────

/// Open `$EDITOR` (or `vi`) with the given file path and wait for it to exit.
fn open_editor(path: &std::path::Path) -> Result<()> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(path)
        .status()
        .map_err(|e| HimitsuError::External(format!("failed to launch editor '{editor}': {e}")))?;
    if !status.success() {
        return Err(HimitsuError::External(format!(
            "editor '{editor}' exited with non-zero status"
        )));
    }
    Ok(())
}

/// Parse a staging YAML file into `(path, value)` pairs.
///
/// The file is a flat YAML mapping of `String → String`. Non-string values
/// are skipped (including empty values that result from comment-only files).
fn parse_staging_yaml(content: &str) -> Result<Vec<(String, String)>> {
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }

    let value: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|e| HimitsuError::External(format!("failed to parse edited staging YAML: {e}")))?;

    let mapping = match value {
        serde_yaml::Value::Mapping(m) => m,
        serde_yaml::Value::Null => return Ok(Vec::new()),
        other => {
            return Err(HimitsuError::External(format!(
                "staging YAML must be a flat mapping, got: {other:?}"
            )));
        }
    };

    let mut entries = Vec::new();
    for (k, v) in &mapping {
        let key = match k {
            serde_yaml::Value::String(s) => s.clone(),
            other => {
                eprintln!("skipping non-string key: {other:?}");
                continue;
            }
        };
        let val = match v {
            serde_yaml::Value::String(s) => s.clone(),
            serde_yaml::Value::Null => continue,
            other => yaml_value_to_string(other),
        };
        if val.is_empty() {
            continue;
        }
        entries.push((key, val));
    }
    Ok(entries)
}

// ── Plain YAML file import ───────────────────────────────────────────────────

/// Import secrets from a plain YAML file on disk (same format as the staging file).
fn run_yaml_file(file: &str, args: &ImportArgs, ctx: &Context) -> Result<()> {
    let content = std::fs::read_to_string(file)
        .map_err(|e| HimitsuError::External(format!("failed to read {file}: {e}")))?;

    let entries = parse_staging_yaml(&content)?;

    if entries.is_empty() {
        println!("No secrets to import from {file}");
        return Ok(());
    }

    let mut count = 0;
    for (path, value) in &entries {
        if args.dry_run {
            println!("[dry-run] would import {path}");
            continue;
        }
        if !args.overwrite && secret_exists_at(&ctx.store, path) {
            eprintln!("skipping {path}: already exists (use --overwrite to replace)");
            continue;
        }
        set_plaintext(ctx, path, value.as_bytes(), Vec::new())?;
        println!("Imported {path}");
        count += 1;
    }

    if args.dry_run {
        println!("[dry-run] {} secret(s) would be imported", entries.len());
    } else {
        println!("{count} secret(s) imported from {file}");
    }
    Ok(())
}

// ── SOPS import ─────────────────────────────────────────────────────────────

/// Decrypt a SOPS file and import all leaf key-value pairs as secrets.
fn run_sops(sops_file: &str, args: &ImportArgs, ctx: &Context) -> Result<()> {
    let decrypted = sops_decrypt("sops", sops_file)?;

    // Try YAML first (a superset of JSON), fall back to JSON explicitly.
    let value: serde_yaml::Value = serde_yaml::from_str(&decrypted)
        .map_err(|e| HimitsuError::External(format!("failed to parse SOPS output: {e}")))?;

    let mut pairs: Vec<(String, String)> = Vec::new();
    flatten_yaml("", &value, &mut pairs);

    if pairs.is_empty() {
        return Err(HimitsuError::External(
            "SOPS file decrypted but contained no leaf key-value pairs".into(),
        ));
    }

    let prefix = effective_prefix(args, "");
    let normalize = !args.keep_names;

    // Apply filter and normalization.
    let entries: Vec<(String, String)> = pairs
        .into_iter()
        .filter(|(key, _)| match args.filter.as_deref() {
            Some(f) => matches_filter(key, f),
            None => true,
        })
        .map(|(key, val)| {
            let normalized = if normalize {
                normalize_key_path(&key)
            } else {
                key
            };
            let full_path = if prefix.is_empty() {
                normalized
            } else {
                format!("{prefix}/{normalized}")
            };
            (full_path, val)
        })
        .collect();

    if entries.is_empty() {
        println!("No matching secrets found in {sops_file}");
        return Ok(());
    }

    let mut imported = 0usize;
    for (full_path, val) in &entries {
        if args.dry_run {
            println!("[dry-run] {full_path}");
            continue;
        }

        if !args.overwrite && secret_exists_at(&ctx.store, full_path) {
            eprintln!("skipping {full_path}: already exists (use --overwrite to replace)");
            continue;
        }

        set_plaintext(ctx, full_path, val.as_bytes(), Vec::new())?;
        imported += 1;
        println!("Imported {full_path}");
    }

    if args.dry_run {
        println!("[dry-run] {} secret(s) would be imported", entries.len());
        return Ok(());
    }

    println!("{imported} secret(s) imported from {sops_file}");
    Ok(())
}

/// Shell out to `sops -d <file>` and return the decrypted plaintext.
fn sops_decrypt(program: &str, file: &str) -> Result<String> {
    let output = Command::new(program)
        .args(["-d", file])
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => HimitsuError::External(
                "`sops` CLI not found on PATH — install from \
                 https://github.com/getsops/sops"
                    .into(),
            ),
            _ => HimitsuError::External(format!("failed to spawn `sops`: {e}")),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        let detail = if trimmed.is_empty() {
            format!("`sops -d` exited with status {}", output.status)
        } else {
            format!("`sops -d` failed: {trimmed}")
        };
        return Err(HimitsuError::External(detail));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| HimitsuError::External(format!("`sops -d` returned non-UTF-8 output: {e}")))
}

/// Recursively flatten a YAML/JSON value into `(key, string_value)` pairs.
///
/// Nested maps are joined with `/` as a separator. Array elements are
/// indexed as `key/0`, `key/1`, etc. Null values are skipped.
fn flatten_yaml(prefix: &str, value: &serde_yaml::Value, out: &mut Vec<(String, String)>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                let key_str = yaml_key_to_string(k);
                let child = if prefix.is_empty() {
                    key_str
                } else {
                    format!("{prefix}/{key_str}")
                };
                flatten_yaml(&child, v, out);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for (i, v) in seq.iter().enumerate() {
                let child = if prefix.is_empty() {
                    i.to_string()
                } else {
                    format!("{prefix}/{i}")
                };
                flatten_yaml(&child, v, out);
            }
        }
        serde_yaml::Value::Null => {
            // Skip null values.
        }
        _ => {
            let s = yaml_value_to_string(value);
            out.push((prefix.to_owned(), s));
        }
    }
}

/// Convert a YAML map key to a string for use in secret paths.
fn yaml_key_to_string(key: &serde_yaml::Value) -> String {
    match key {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

/// Convert a YAML leaf value to its string representation.
fn yaml_value_to_string(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Null => String::new(),
        other => format!("{other:?}"),
    }
}

// ── 1Password helpers ──────────────────────────────────────────────────────

/// Build import actions for all fields of a single item.
fn build_item_actions(
    program: &str,
    vault: &str,
    item: &str,
    prefix: &str,
    op_ref: &str,
    normalize: bool,
) -> Result<Vec<ImportAction>> {
    let op_item = op_item_get(program, vault, item)?;
    let mut actions = Vec::new();
    let mut seen_labels: HashMap<String, usize> = HashMap::new();

    for field in &op_item.fields {
        if should_skip_field(field) {
            continue;
        }
        let value = match &field.value {
            Some(v) if !v.is_empty() => v.clone(),
            _ => continue,
        };

        let sanitized = sanitize_label(&field.label);
        let label = if normalize {
            normalize_key_name(&sanitized)
        } else {
            sanitized.clone()
        };
        let unique_label = deduplicate_label(&label, &mut seen_labels);
        let target = format!("{prefix}/{unique_label}");

        actions.push(ImportAction {
            source_key: sanitized,
            target,
            value,
        });
    }

    if actions.is_empty() {
        eprintln!("Warning: no importable fields found in {op_ref}");
    }

    Ok(actions)
}

/// Build import actions for all items in a vault.
fn build_vault_actions(
    program: &str,
    vault: &str,
    prefix: &str,
    normalize: bool,
) -> Result<Vec<ImportAction>> {
    let items = op_item_list(program, vault)?;
    let mut all_actions = Vec::new();

    for item_summary in &items {
        let sanitized = sanitize_label(&item_summary.title);
        if sanitized.is_empty() {
            continue;
        }
        let item_label = if normalize {
            normalize_key_name(&sanitized)
        } else {
            sanitized
        };
        let item_prefix = format!("{prefix}/{item_label}");
        let item_ref = format!("op://{vault}/{}", item_summary.title);
        match build_item_actions(
            program,
            vault,
            &item_summary.id,
            &item_prefix,
            &item_ref,
            normalize,
        ) {
            Ok(mut item_actions) => all_actions.append(&mut item_actions),
            Err(e) => {
                eprintln!("Warning: skipping item {:?}: {e}", item_summary.title);
            }
        }
    }

    Ok(all_actions)
}

/// Returns true if a field should be skipped during import.
fn should_skip_field(field: &OpField) -> bool {
    // Skip section headers / separators
    let ft = field.field_type.to_uppercase();
    if ft == "SECTION_HEADER" || ft == "SECTION" {
        return true;
    }
    // Skip fields with no label (can't derive a meaningful path)
    if field.label.trim().is_empty() {
        return true;
    }
    false
}

/// Sanitize a label for use as a path segment. Replaces spaces and special
/// characters with underscores, lowercases, and strips leading/trailing
/// underscores.
fn sanitize_label(label: &str) -> String {
    let sanitized: String = label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    sanitized.trim_matches('_').to_string()
}

/// Ensure label uniqueness by appending `_2`, `_3`, etc. on collision.
fn deduplicate_label(label: &str, seen: &mut HashMap<String, usize>) -> String {
    let count = seen.entry(label.to_string()).or_insert(0);
    *count += 1;
    if *count == 1 {
        label.to_string()
    } else {
        format!("{label}_{count}")
    }
}

fn secret_exists_at(store: &std::path::Path, secret_path: &str) -> bool {
    if store.as_os_str().is_empty() {
        return false;
    }
    let dir = secrets_dir(store);
    dir.join(format!("{secret_path}.yaml")).exists()
        || dir.join(format!("{secret_path}.age")).exists()
}

/// Shell out to `op read <reference>` and return the plaintext value.
fn op_read(program: &str, op_ref: &str) -> Result<String> {
    let output = Command::new(program)
        .args(["read", "--no-newline", op_ref])
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => HimitsuError::External(
                "`op` CLI not found on PATH — install 1Password CLI from \
                 https://developer.1password.com/docs/cli/get-started/"
                    .into(),
            ),
            _ => HimitsuError::External(format!("failed to spawn `op`: {e}")),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        let detail = if trimmed.is_empty() {
            format!("`op read` exited with status {}", output.status)
        } else {
            format!("`op read` failed: {trimmed}")
        };
        return Err(HimitsuError::External(detail));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| HimitsuError::External(format!("`op read` returned non-UTF-8 output: {e}")))
}

/// Shell out to `op item get <item> --vault=<vault> --format=json` and
/// deserialize the result into an `OpItem`.
fn op_item_get(program: &str, vault: &str, item: &str) -> Result<OpItem> {
    let output = Command::new(program)
        .args([
            "item",
            "get",
            item,
            &format!("--vault={vault}"),
            "--format=json",
        ])
        .output()
        .map_err(|e| op_spawn_error(e, "op item get"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HimitsuError::External(format!(
            "`op item get` failed: {}",
            stderr.trim()
        )));
    }

    let json = String::from_utf8(output.stdout).map_err(|e| {
        HimitsuError::External(format!("`op item get` returned non-UTF-8 output: {e}"))
    })?;
    serde_json::from_str(&json)
        .map_err(|e| HimitsuError::External(format!("`op item get` returned invalid JSON: {e}")))
}

/// Shell out to `op item list --vault=<vault> --format=json` and deserialize
/// the result into a list of item summaries.
fn op_item_list(program: &str, vault: &str) -> Result<Vec<OpItemSummary>> {
    let output = Command::new(program)
        .args(["item", "list", &format!("--vault={vault}"), "--format=json"])
        .output()
        .map_err(|e| op_spawn_error(e, "op item list"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HimitsuError::External(format!(
            "`op item list` failed: {}",
            stderr.trim()
        )));
    }

    let json = String::from_utf8(output.stdout).map_err(|e| {
        HimitsuError::External(format!("`op item list` returned non-UTF-8 output: {e}"))
    })?;
    serde_json::from_str(&json)
        .map_err(|e| HimitsuError::External(format!("`op item list` returned invalid JSON: {e}")))
}

/// Normalize a key name: lowercase and replace underscores with hyphens.
/// `SOME_KEY` → `some-key`, `DATABASE_URL` → `database-url`.
fn normalize_key_name(s: &str) -> String {
    s.to_lowercase().replace('_', "-")
}

/// Normalize each segment of a `/`-delimited key path independently.
/// `database/HOST_NAME` → `database/host-name`.
fn normalize_key_path(path: &str) -> String {
    path.split('/')
        .map(normalize_key_name)
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolve the effective target prefix from `--to`, positional `path`, or a
/// fallback default. Trailing slashes are stripped.
fn effective_prefix(args: &ImportArgs, default: &str) -> String {
    let raw = args
        .to
        .as_deref()
        .or(args.path.as_deref())
        .unwrap_or(default);
    raw.trim_end_matches('/').to_string()
}

/// Check whether a source key matches a simple glob filter.
fn matches_filter(key: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern == "**" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return key.starts_with(&format!("{prefix}/")) || key == prefix;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        if let Some(rest) = key.strip_prefix(&format!("{prefix}/")) {
            return !rest.contains('/');
        }
        return false;
    }
    key == pattern
}

/// Shared helper for subprocess spawn errors.
fn op_spawn_error(e: std::io::Error, cmd: &str) -> HimitsuError {
    match e.kind() {
        std::io::ErrorKind::NotFound => HimitsuError::External(
            "`op` CLI not found on PATH — install 1Password CLI from \
             https://developer.1password.com/docs/cli/get-started/"
                .into(),
        ),
        _ => HimitsuError::External(format!("failed to spawn `{cmd}`: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: TestCmd,
    }

    #[derive(Debug, clap::Subcommand)]
    enum TestCmd {
        Import(ImportArgs),
    }

    fn parse(args: &[&str]) -> ImportArgs {
        let mut full = vec!["test", "import"];
        full.extend_from_slice(args);
        let TestCli {
            cmd: TestCmd::Import(a),
        } = TestCli::try_parse_from(full).expect("parse ok");
        a
    }

    #[test]
    fn parses_op_and_path() {
        let a = parse(&["--op", "op://Personal/Stripe/credential", "prod/STRIPE_KEY"]);
        assert_eq!(a.path.as_deref(), Some("prod/STRIPE_KEY"));
        assert_eq!(a.op.as_deref(), Some("op://Personal/Stripe/credential"));
        assert!(!a.overwrite);
        assert!(!a.no_push);
        assert!(!a.dry_run);
    }

    #[test]
    fn parses_flags() {
        let a = parse(&[
            "--op",
            "op://v/i/f",
            "--overwrite",
            "--no-push",
            "--dry-run",
            "prod/X",
        ]);
        assert!(a.overwrite);
        assert!(a.no_push);
        assert!(a.dry_run);
    }

    #[test]
    fn op_and_sops_conflict() {
        let res = TestCli::try_parse_from([
            "test",
            "import",
            "--op",
            "op://v/i/f",
            "--sops",
            "x.yaml",
            "prod/X",
        ]);
        assert!(res.is_err(), "clap should reject --op with --sops");
    }

    #[test]
    fn missing_source_errors_cleanly() {
        let args = parse(&["prod/X"]);
        let ctx = Context {
            data_dir: std::path::PathBuf::from("/tmp"),
            state_dir: std::path::PathBuf::from("/tmp"),
            store: std::path::PathBuf::new(),
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };
        let err = run(args, &ctx).unwrap_err();
        assert!(
            matches!(err, HimitsuError::InvalidReference(ref m) if m.contains("missing source")),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_non_op_reference() {
        let args = ImportArgs {
            path: Some("prod/X".into()),
            op: Some("https://example.com/foo".into()),
            sops: None,
            to: None,
            filter: None,
            overwrite: false,
            no_push: false,
            dry_run: false,
            keep_names: false,
        };
        let ctx = Context {
            data_dir: std::path::PathBuf::from("/tmp"),
            state_dir: std::path::PathBuf::from("/tmp"),
            store: std::path::PathBuf::new(),
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };
        let err = run(args, &ctx).unwrap_err();
        assert!(
            matches!(err, HimitsuError::InvalidReference(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn path_required_for_single_field() {
        // op://vault/item/field with no positional path and no --to should error
        let args = ImportArgs {
            path: None,
            op: Some("op://Personal/Stripe/credential".into()),
            sops: None,
            to: None,
            filter: None,
            overwrite: false,
            no_push: false,
            dry_run: false,
            keep_names: false,
        };
        let ctx = Context {
            data_dir: std::path::PathBuf::from("/tmp"),
            state_dir: std::path::PathBuf::from("/tmp"),
            store: std::path::PathBuf::new(),
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        };
        let err = run(args, &ctx).unwrap_err();
        assert!(
            matches!(err, HimitsuError::InvalidReference(ref m) if m.contains("PATH or --to is required")),
            "got {err:?}"
        );
    }

    #[test]
    fn path_optional_for_whole_item_parses() {
        // op://vault/item with no path should parse fine
        let a = parse(&["--op", "op://Personal/Stripe"]);
        assert!(a.path.is_none());
        assert_eq!(a.op.as_deref(), Some("op://Personal/Stripe"));
    }

    #[test]
    fn path_optional_for_whole_vault_parses() {
        // op://vault with no path should parse fine
        let a = parse(&["--op", "op://Personal"]);
        assert!(a.path.is_none());
        assert_eq!(a.op.as_deref(), Some("op://Personal"));
    }

    #[test]
    fn dry_run_flag_parses() {
        let a = parse(&["--op", "op://v/i/f", "--dry-run", "prod/X"]);
        assert!(a.dry_run);
    }

    /// Exercises the real subprocess plumbing for `op_read` by pointing it
    /// at an absolute path we know does not exist. This verifies the
    /// "missing binary" error branch without touching the process-wide
    /// `PATH` (which would race with sibling tests).
    #[test]
    fn op_read_errors_when_binary_missing() {
        let fake = "/nonexistent/himitsu-test-op-binary";
        let err = op_read(fake, "op://v/i/f").expect_err("expected error when binary is missing");
        match err {
            HimitsuError::External(msg) => {
                assert!(
                    msg.contains("not found") || msg.contains("failed to spawn"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected External error, got {other:?}"),
        }
    }

    // ── SOPS tests ─────────────────────────────────────────────────────────

    #[test]
    fn sops_errors_when_binary_missing() {
        let err = sops_decrypt("/nonexistent/himitsu-test-sops-binary", "file.yaml")
            .expect_err("expected error when binary is missing");
        match err {
            HimitsuError::External(msg) => {
                assert!(
                    msg.contains("not found") || msg.contains("failed to spawn"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected External error, got {other:?}"),
        }
    }

    #[test]
    fn parses_sops_flag() {
        let a = parse(&["--sops", "secrets.enc.yaml", "prod"]);
        assert_eq!(a.sops.as_deref(), Some("secrets.enc.yaml"));
        assert_eq!(a.path.as_deref(), Some("prod"));
    }

    #[test]
    fn flatten_yaml_nested_map() {
        let yaml = "database:\n  host: localhost\n  port: 5432\napi_key: secret123";
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let mut pairs = Vec::new();
        flatten_yaml("", &value, &mut pairs);
        assert_eq!(pairs.len(), 3);
        assert!(pairs.contains(&("database/host".into(), "localhost".into())));
        assert!(pairs.contains(&("database/port".into(), "5432".into())));
        assert!(pairs.contains(&("api_key".into(), "secret123".into())));
    }

    #[test]
    fn flatten_yaml_skips_nulls() {
        let yaml = "a: 1\nb: null\nc: three";
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let mut pairs = Vec::new();
        flatten_yaml("", &value, &mut pairs);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|(k, _)| k != "b"));
    }

    #[test]
    fn flatten_yaml_sequences() {
        let yaml = "hosts:\n  - alpha\n  - beta";
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let mut pairs = Vec::new();
        flatten_yaml("", &value, &mut pairs);
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&("hosts/0".into(), "alpha".into())));
        assert!(pairs.contains(&("hosts/1".into(), "beta".into())));
    }

    #[test]
    fn flatten_yaml_booleans_and_numbers() {
        let yaml = "enabled: true\ncount: 42\nratio: 3.14";
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let mut pairs = Vec::new();
        flatten_yaml("", &value, &mut pairs);
        assert_eq!(pairs.len(), 3);
        assert!(pairs.contains(&("enabled".into(), "true".into())));
        assert!(pairs.contains(&("count".into(), "42".into())));
        assert!(pairs.contains(&("ratio".into(), "3.14".into())));
    }

    // ── 1Password helper tests ─────────────────────────────────────────────

    #[test]
    fn sanitize_label_replaces_special_chars() {
        assert_eq!(sanitize_label("API Key"), "API_Key");
        assert_eq!(sanitize_label("  foo  "), "foo");
        assert_eq!(sanitize_label("my.secret.key"), "my_secret_key");
        assert_eq!(sanitize_label("hello-world"), "hello-world");
    }

    #[test]
    fn deduplicate_label_appends_suffix() {
        let mut seen = HashMap::new();
        assert_eq!(deduplicate_label("username", &mut seen), "username");
        assert_eq!(deduplicate_label("username", &mut seen), "username_2");
        assert_eq!(deduplicate_label("username", &mut seen), "username_3");
        assert_eq!(deduplicate_label("password", &mut seen), "password");
    }

    #[test]
    fn should_skip_field_skips_section_headers() {
        let section = OpField {
            label: "Login Details".into(),
            value: None,
            field_type: "SECTION_HEADER".into(),
        };
        assert!(should_skip_field(&section));

        let section2 = OpField {
            label: "Other".into(),
            value: None,
            field_type: "section".into(),
        };
        assert!(should_skip_field(&section2));
    }

    #[test]
    fn should_skip_field_skips_empty_labels() {
        let empty = OpField {
            label: "  ".into(),
            value: Some("secret".into()),
            field_type: "STRING".into(),
        };
        assert!(should_skip_field(&empty));
    }

    #[test]
    fn should_skip_field_keeps_normal_fields() {
        let normal = OpField {
            label: "password".into(),
            value: Some("hunter2".into()),
            field_type: "CONCEALED".into(),
        };
        assert!(!should_skip_field(&normal));
    }

    #[test]
    fn op_item_get_errors_when_binary_missing() {
        let fake = "/nonexistent/himitsu-test-op-binary";
        let err = op_item_get(fake, "Personal", "Stripe")
            .expect_err("expected error when binary is missing");
        assert!(matches!(err, HimitsuError::External(_)), "got {err:?}");
    }

    #[test]
    fn op_item_list_errors_when_binary_missing() {
        let fake = "/nonexistent/himitsu-test-op-binary";
        let err =
            op_item_list(fake, "Personal").expect_err("expected error when binary is missing");
        assert!(matches!(err, HimitsuError::External(_)), "got {err:?}");
    }

    #[test]
    fn deserialize_op_item_json() {
        let json = r#"{
            "title": "Stripe",
            "fields": [
                {"label": "username", "value": "admin", "type": "STRING"},
                {"label": "password", "value": "secret", "type": "CONCEALED"},
                {"label": "", "value": null, "type": "SECTION_HEADER"},
                {"label": "notes", "value": "", "type": "STRING"}
            ]
        }"#;
        let item: OpItem = serde_json::from_str(json).expect("parse");
        assert_eq!(item.title, "Stripe");
        assert_eq!(item.fields.len(), 4);
        assert_eq!(item.fields[0].label, "username");
        assert_eq!(item.fields[0].value, Some("admin".into()));
    }

    #[test]
    fn deserialize_op_item_list_json() {
        let json = r#"[
            {"id": "abc123", "title": "Stripe"},
            {"id": "def456", "title": "AWS"}
        ]"#;
        let items: Vec<OpItemSummary> = serde_json::from_str(json).expect("parse");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "abc123");
        assert_eq!(items[0].title, "Stripe");
    }

    // ── Key normalization tests ───────────────────────────────────────────

    #[test]
    fn normalize_key_name_screaming_snake_to_kebab() {
        assert_eq!(normalize_key_name("SOME_KEY"), "some-key");
        assert_eq!(normalize_key_name("DATABASE_URL"), "database-url");
        assert_eq!(normalize_key_name("API_KEY"), "api-key");
    }

    #[test]
    fn normalize_key_name_preserves_hyphens() {
        assert_eq!(normalize_key_name("already-kebab"), "already-kebab");
        assert_eq!(normalize_key_name("some-key"), "some-key");
    }

    #[test]
    fn normalize_key_name_mixed_case() {
        assert_eq!(normalize_key_name("SomeKey"), "somekey");
        assert_eq!(normalize_key_name("some_Key"), "some-key");
    }

    #[test]
    fn normalize_key_path_normalizes_each_segment() {
        assert_eq!(
            normalize_key_path("database/HOST_NAME"),
            "database/host-name"
        );
        assert_eq!(normalize_key_path("PROD/API_KEY"), "prod/api-key");
        assert_eq!(normalize_key_path("simple"), "simple");
    }

    // ── Glob filter tests ─────────────────────────────────────────────────

    #[test]
    fn matches_filter_exact() {
        assert!(matches_filter("foo", "foo"));
        assert!(!matches_filter("foo", "bar"));
        assert!(!matches_filter("foo/bar", "foo"));
    }

    #[test]
    fn matches_filter_star_glob() {
        assert!(matches_filter("foo/bar", "foo/*"));
        assert!(matches_filter("foo/baz", "foo/*"));
        assert!(!matches_filter("foo/bar/baz", "foo/*"));
        assert!(!matches_filter("other/bar", "foo/*"));
    }

    #[test]
    fn matches_filter_double_star_glob() {
        assert!(matches_filter("foo/bar", "foo/**"));
        assert!(matches_filter("foo/bar/baz", "foo/**"));
        assert!(!matches_filter("other/bar", "foo/**"));
    }

    #[test]
    fn matches_filter_wildcard_all() {
        assert!(matches_filter("anything", "*"));
        assert!(matches_filter("foo/bar", "*"));
        assert!(matches_filter("a/b/c", "**"));
    }

    // ── --to prefix tests ─────────────────────────────────────────────────

    #[test]
    fn effective_prefix_prefers_to_over_path() {
        let args = ImportArgs {
            path: Some("old-prefix".into()),
            op: None,
            sops: None,
            to: Some("new-prefix".into()),
            filter: None,
            overwrite: false,
            no_push: false,
            dry_run: false,
            keep_names: false,
        };
        assert_eq!(effective_prefix(&args, "default"), "new-prefix");
    }

    #[test]
    fn effective_prefix_falls_back_to_path() {
        let args = ImportArgs {
            path: Some("from-path".into()),
            op: None,
            sops: None,
            to: None,
            filter: None,
            overwrite: false,
            no_push: false,
            dry_run: false,
            keep_names: false,
        };
        assert_eq!(effective_prefix(&args, "default"), "from-path");
    }

    #[test]
    fn effective_prefix_strips_trailing_slash() {
        let args = ImportArgs {
            path: None,
            op: None,
            sops: None,
            to: Some("foo/bar/".into()),
            filter: None,
            overwrite: false,
            no_push: false,
            dry_run: false,
            keep_names: false,
        };
        assert_eq!(effective_prefix(&args, "default"), "foo/bar");
    }

    #[test]
    fn effective_prefix_uses_default_when_none() {
        let args = ImportArgs {
            path: None,
            op: None,
            sops: None,
            to: None,
            filter: None,
            overwrite: false,
            no_push: false,
            dry_run: false,
            keep_names: false,
        };
        assert_eq!(effective_prefix(&args, "fallback"), "fallback");
    }

    // ── CLI parsing of new flags ──────────────────────────────────────────

    #[test]
    fn parses_to_flag() {
        let a = parse(&["--sops", "f.yaml", "--to", "prod/stripe"]);
        assert_eq!(a.to.as_deref(), Some("prod/stripe"));
    }

    #[test]
    fn parses_filter_flag() {
        let a = parse(&["--sops", "f.yaml", "--filter", "database/*"]);
        assert_eq!(a.filter.as_deref(), Some("database/*"));
    }

    #[test]
    fn parses_keep_names_flag() {
        let a = parse(&["--sops", "f.yaml", "--keep-names"]);
        assert!(a.keep_names);
    }
}
