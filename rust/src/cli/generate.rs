use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

use clap::Args;

use crate::cli::Context;
use crate::config::{load_project_config, EnvEntry, ProjectConfig};
use crate::crypto::age as crypto;
use crate::error::{HimitsuError, Result};
use crate::reference::SecretRef;
use crate::remote::store;

/// Generate SOPS-encrypted (or plaintext) output files from env definitions.
#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Override output directory (default: from `generate.target` in project config).
    #[arg(long)]
    pub target: Option<String>,

    /// Generate only this env (default: all envs defined in project config).
    #[arg(long)]
    pub env: Option<String>,

    /// Print plaintext YAML to stdout instead of writing encrypted files.
    #[arg(long)]
    pub stdout: bool,
}

pub fn run(args: GenerateArgs, ctx: &Context) -> Result<()> {
    // Load project config — required for generate.
    let (project_cfg, _cfg_path) = load_project_config().ok_or_else(|| {
        HimitsuError::ProjectConfigRequired(
            "no project config found (himitsu.yaml); run generate from a project root".into(),
        )
    })?;

    if project_cfg.envs.is_empty() {
        return Err(HimitsuError::GenerateError(
            "no `envs` defined in project config".into(),
        ));
    }

    // Load age identity for decryption.
    let identity = crypto::read_identity(&ctx.key_path())?;

    // Determine which envs to generate.
    let env_names: Vec<String> = if let Some(ref env_name) = args.env {
        if !project_cfg.envs.contains_key(env_name.as_str()) {
            return Err(HimitsuError::GenerateError(format!(
                "env '{env_name}' not found in project config"
            )));
        }
        vec![env_name.clone()]
    } else {
        project_cfg.envs.keys().cloned().collect()
    };

    for env_name in &env_names {
        let entries = project_cfg.envs.get(env_name.as_str()).unwrap();

        // Resolve entries to (output_key, store_path, optional_store_override) tuples.
        let mappings = resolve_entries(entries, env_name, &ctx.store)?;

        if mappings.is_empty() {
            eprintln!("warning: env '{env_name}' resolved to no secrets — skipping");
            continue;
        }

        // Decrypt each secret.
        let mut output: BTreeMap<String, String> = BTreeMap::new();
        for (key, path, store_override) in &mappings {
            let effective_store = store_override.as_deref().unwrap_or(&ctx.store);
            let ciphertext = store::read_secret(effective_store, path)?;
            let plaintext_bytes = crypto::decrypt(&ciphertext, &identity)?;
            let plaintext = String::from_utf8(plaintext_bytes).map_err(|e| {
                HimitsuError::DecryptionFailed(format!("non-UTF-8 secret at '{path}': {e}"))
            })?;
            if output.contains_key(key) {
                eprintln!("warning: duplicate key '{key}' in env '{env_name}' — using last value");
            }
            output.insert(key.clone(), plaintext);
        }

        let yaml = build_plaintext_yaml(&output, env_name);

        if args.stdout {
            print!("{yaml}");
        } else {
            let target_dir = resolve_target(&args, &project_cfg)?;
            write_env_file(&target_dir, env_name, &yaml, &project_cfg)?;
        }
    }

    Ok(())
}

// ── Entry resolution ─────────────────────────────────────────────────────────

/// Resolve environment entries to `(output_key, secret_path, store_override)` tuples.
///
/// - `Alias { key, path }` → `[(key, path, None)]`, or with a resolved store when
///   `path` is a qualified reference (`provider:org/repo/path`).
/// - `Single(path)` → `[(last_component(path), path, None)]`, with store override when qualified.
/// - `Glob(prefix)` → one tuple per secret found under `prefix/`, with store override when qualified.
///
/// The third element is `Some(store_path)` when the entry uses a provider-prefixed
/// qualified reference; callers should use it instead of `ctx.store` for that secret.
fn resolve_entries(
    entries: &[EnvEntry],
    env_name: &str,
    store_path: &Path,
) -> Result<Vec<(String, String, Option<PathBuf>)>> {
    let mut result = vec![];
    for entry in entries {
        match entry {
            EnvEntry::Alias { key, path } => {
                let secret_ref = SecretRef::parse(path)?;
                if secret_ref.is_qualified() {
                    let resolved_store = secret_ref.resolve_store()?;
                    let secret_path = secret_ref.path.ok_or_else(|| {
                        HimitsuError::InvalidReference(format!(
                            "alias '{key}' has a qualified store reference but no secret path: {path:?}"
                        ))
                    })?;
                    result.push((key.clone(), secret_path, Some(resolved_store)));
                } else {
                    result.push((key.clone(), path.clone(), None));
                }
            }
            EnvEntry::Single(path) => {
                let secret_ref = SecretRef::parse(path)?;
                if secret_ref.is_qualified() {
                    let resolved_store = secret_ref.resolve_store()?;
                    let secret_path = secret_ref.path.ok_or_else(|| {
                        HimitsuError::InvalidReference(format!(
                            "single entry has a qualified store reference but no secret path: {path:?}"
                        ))
                    })?;
                    let key = last_component(&secret_path);
                    result.push((key, secret_path, Some(resolved_store)));
                } else {
                    let key = last_component(path);
                    result.push((key, path.clone(), None));
                }
            }
            EnvEntry::Glob(prefix) => {
                let secret_ref = SecretRef::parse(prefix)?;
                if secret_ref.is_qualified() {
                    let resolved_store = secret_ref.resolve_store()?;
                    let path_prefix = secret_ref.path.as_deref();
                    let paths = store::list_secrets(&resolved_store, path_prefix)?;
                    if paths.is_empty() {
                        eprintln!(
                            "warning: glob '{prefix}/*' in env '{env_name}' matched no secrets"
                        );
                    }
                    for p in paths {
                        let key = last_component(&p);
                        result.push((key, p, Some(resolved_store.clone())));
                    }
                } else {
                    let paths = store::list_secrets(store_path, Some(prefix))?;
                    if paths.is_empty() {
                        eprintln!(
                            "warning: glob '{prefix}/*' in env '{env_name}' matched no secrets"
                        );
                    }
                    for p in paths {
                        let key = last_component(&p);
                        result.push((key, p, None));
                    }
                }
            }
            // Tag selectors require decrypting candidate secrets to read
            // their `SecretValue.tags` field. The legacy `generate` command
            // doesn't share `codegen`'s resolver pipeline — point users at
            // `himitsu codegen <env>` (which calls `resolve_with_tags`).
            EnvEntry::Tag(_) | EnvEntry::AliasTag { .. } => {
                return Err(HimitsuError::InvalidConfig(format!(
                    "env '{env_name}' uses a `tag:` selector — `himitsu generate` does not \
                     support tag-based selection; use `himitsu codegen <env>` instead"
                )));
            }
        }
    }
    Ok(result)
}

/// Extract the last path component as a key name  
/// (`"dev/API_KEY"` → `"API_KEY"`).
fn last_component(path: &str) -> String {
    path.split('/').next_back().unwrap_or(path).to_string()
}

// ── YAML output ──────────────────────────────────────────────────────────────

/// Build the plaintext YAML document for one env.
fn build_plaintext_yaml(secrets: &BTreeMap<String, String>, env_name: &str) -> String {
    let mut out = format!("# Generated by himitsu for env: {env_name}\n");
    out.push_str("# Do not edit — regenerate with `himitsu generate`\n");
    for (k, v) in secrets {
        // Escape only the chars that break a YAML double-quoted string.
        let escaped = v
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        out.push_str(&format!("{k}: \"{escaped}\"\n"));
    }
    out
}

// ── Target resolution ────────────────────────────────────────────────────────

fn resolve_target(args: &GenerateArgs, project_cfg: &ProjectConfig) -> Result<PathBuf> {
    if let Some(ref t) = args.target {
        return Ok(PathBuf::from(t));
    }
    if let Some(ref gen_cfg) = project_cfg.generate {
        return Ok(PathBuf::from(&gen_cfg.target));
    }
    Err(HimitsuError::GenerateError(
        "no output target specified; use --target or set `generate.target` in himitsu.yaml".into(),
    ))
}

// ── File output ──────────────────────────────────────────────────────────────

/// Write one env file to `target/<env>.sops.yaml`, encrypting via `sops`.
fn write_env_file(
    target_dir: &Path,
    env_name: &str,
    yaml: &str,
    project_cfg: &ProjectConfig,
) -> Result<()> {
    std::fs::create_dir_all(target_dir)?;
    let out_path = target_dir.join(format!("{env_name}.sops.yaml"));

    let recipients: Vec<String> = project_cfg
        .generate
        .as_ref()
        .map(|g| g.age_recipients.clone())
        .unwrap_or_default();

    if recipients.is_empty() {
        return Err(HimitsuError::GenerateError(
            "no `generate.age_recipients` configured; \
             add recipients to himitsu.yaml or use --stdout for plaintext"
                .into(),
        ));
    }

    let age_arg = recipients.join(",");

    // Pipe plaintext YAML into `sops --encrypt` — no plaintext is written to disk.
    let mut child = StdCommand::new("sops")
        .args([
            "--encrypt",
            "--age",
            &age_arg,
            "--input-type",
            "yaml",
            "--output-type",
            "yaml",
            "/dev/stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HimitsuError::GenerateError(
                    "sops not found in PATH; install sops to generate encrypted output, \
                     or use --stdout for plaintext"
                        .into(),
                )
            } else {
                HimitsuError::GenerateError(format!("failed to launch sops: {e}"))
            }
        })?;

    child
        .stdin
        .take()
        .expect("stdin piped")
        .write_all(yaml.as_bytes())
        .map_err(|e| HimitsuError::GenerateError(format!("failed to write to sops stdin: {e}")))?;

    let result = child
        .wait_with_output()
        .map_err(|e| HimitsuError::GenerateError(format!("sops process error: {e}")))?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(HimitsuError::GenerateError(format!(
            "sops encryption failed: {stderr}"
        )));
    }

    std::fs::write(&out_path, &result.stdout)?;
    eprintln!("Generated: {}", out_path.display());
    Ok(())
}
