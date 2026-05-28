use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

use clap::Args;

use crate::cli::Context;
use crate::config::outputs::resolver::{
    resolve_outputs, Context as ResolverContext, SecretCandidate,
};
use crate::config::{self, load_project_config, ProjectConfig};
use crate::crypto::{age as crypto, secret_value};
use crate::error::{HimitsuError, Result};
use crate::remote::store;

/// Generate SOPS-encrypted (or plaintext) output files from outputs definitions.
#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Override output directory (default: from `generate.target` in project config).
    #[arg(long)]
    pub target: Option<String>,

    /// Generate only this output (default: all outputs defined in project config).
    #[arg(long)]
    pub output: Option<String>,

    /// Removed flag — use `--output` instead.
    #[arg(long, hide = true)]
    pub env: Option<String>,

    /// Print plaintext YAML to stdout instead of writing encrypted files.
    #[arg(long)]
    pub stdout: bool,
}

pub fn run(args: GenerateArgs, ctx: &Context) -> Result<()> {
    if args.env.is_some() {
        return Err(HimitsuError::GenerateError(
            "--env flag has been removed; use --output instead".to_string(),
        ));
    }

    let project_cfg = load_project_config().map(|(cfg, _)| cfg);

    let outputs_map = project_cfg
        .as_ref()
        .map(|c| c.outputs.clone())
        .unwrap_or_default();

    if outputs_map.is_empty() {
        return Err(HimitsuError::GenerateError(
            "no `outputs` defined in project config — \
             define outputs: blocks in himitsu.yaml or use `himitsu codegen`"
                .into(),
        ));
    }

    let available_secrets = store::list_secrets(&ctx.store, None)
        .unwrap_or_default()
        .into_iter()
        .map(|path| SecretCandidate { path, tags: vec![] })
        .collect();
    let resolver_ctx = ResolverContext { available_secrets };

    let all_outputs = resolve_outputs(&outputs_map, &resolver_ctx)?;

    let to_generate: Vec<_> = if let Some(ref name) = args.output {
        let filtered: Vec<_> = all_outputs
            .into_iter()
            .filter(|o| &o.name == name)
            .collect();
        if filtered.is_empty() {
            return Err(HimitsuError::GenerateError(format!(
                "output '{name}' not found in project config"
            )));
        }
        filtered
    } else {
        all_outputs
    };

    let identities = ctx.load_identities()?;

    for resolved_output in &to_generate {
        if resolved_output.entries.is_empty() {
            eprintln!(
                "warning: output '{}' resolved to no secrets — skipping",
                resolved_output.name
            );
            continue;
        }

        let mut output: BTreeMap<String, String> = BTreeMap::new();
        for entry in &resolved_output.entries {
            let effective_store = if let Some(ref slug) = entry.store_slug {
                config::ensure_store(slug)?
            } else {
                ctx.store.clone()
            };
            let payload = store::read_secret_payload(&effective_store, &entry.secret_path)?;
            let plaintext = match crypto::decrypt_with_identities(&payload.ciphertext, &identities)
            {
                Ok(p) => p,
                Err(_) if payload.legacy_proto_envelope => payload.ciphertext,
                Err(err) => return Err(err),
            };
            let decoded = secret_value::decode_with_legacy_environment(
                &plaintext,
                payload.legacy_environment.as_deref(),
            );
            super::get::warn_if_expired(&entry.secret_path, &decoded);
            let value = String::from_utf8(decoded.data).map_err(|e| {
                HimitsuError::DecryptionFailed(format!(
                    "non-UTF-8 secret at '{}': {e}",
                    entry.secret_path
                ))
            })?;
            if output.contains_key(&entry.env_key) {
                eprintln!(
                    "warning: duplicate key '{}' in output '{}' — using last value",
                    entry.env_key, resolved_output.name
                );
            }
            output.insert(entry.env_key.clone(), value);
        }

        let yaml = build_plaintext_yaml(&output, &resolved_output.name);

        if args.stdout {
            print!("{yaml}");
        } else {
            let project_cfg = project_cfg.as_ref().ok_or_else(|| {
                HimitsuError::ProjectConfigRequired(
                    "no project config found (himitsu.yaml); use --stdout or --target".into(),
                )
            })?;
            let target_dir = resolve_target(&args, project_cfg)?;
            write_env_file(&target_dir, &resolved_output.name, &yaml, project_cfg)?;
        }
    }

    Ok(())
}

fn build_plaintext_yaml(secrets: &BTreeMap<String, String>, output_name: &str) -> String {
    let mut out = format!("# Generated by himitsu for output: {output_name}\n");
    out.push_str("# Do not edit — regenerate with `himitsu generate`\n");
    for (k, v) in secrets {
        let escaped = v
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        out.push_str(&format!("{k}: \"{escaped}\"\n"));
    }
    out
}

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

fn write_env_file(
    target_dir: &Path,
    output_name: &str,
    yaml: &str,
    project_cfg: &ProjectConfig,
) -> Result<()> {
    std::fs::create_dir_all(target_dir)?;
    let out_path = target_dir.join(format!("{output_name}.sops.yaml"));

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
