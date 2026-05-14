use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

use clap::Args;

use crate::cli::Context;
use crate::config::{load_project_config, ProjectConfig};
use crate::crypto::{age as crypto, secret_value};
use crate::error::{HimitsuError, Result};
use crate::remote::store;

/// Export secrets as SOPS-encrypted (or plaintext) YAML/JSON files.
///
/// Supports glob patterns to select which secrets to include:
///   himitsu export 'prod/*'          — all secrets under prod/
///   himitsu export '**/API_KEY'       — API_KEY at any depth
///   himitsu export 'prod/db/*'       — all secrets under prod/db/
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Glob pattern selecting secrets to export (e.g. `prod/*`, `**/API_KEY`).
    pub pattern: String,

    /// Output file path. Default: derived from pattern (e.g. `prod.sops.yaml`).
    #[arg(short, long)]
    pub output: Option<String>,

    /// Output format.
    #[arg(short, long, default_value = "yaml", value_parser = ["yaml", "json"])]
    pub format: String,

    /// Print plaintext to stdout instead of writing an encrypted file.
    #[arg(long)]
    pub stdout: bool,
}

pub fn run(args: ExportArgs, ctx: &Context) -> Result<()> {
    let identities = ctx.load_identities()?;

    // List all secrets in the store.
    let all_paths = store::list_secrets(&ctx.store, None)?;

    // Filter by glob pattern.
    let matched: Vec<&String> = all_paths
        .iter()
        .filter(|p| glob_match(&args.pattern, p))
        .collect();

    if matched.is_empty() {
        return Err(HimitsuError::SecretNotFound(format!(
            "no secrets matched pattern '{}'",
            args.pattern
        )));
    }

    eprintln!(
        "Matched {} secret{}",
        matched.len(),
        if matched.len() == 1 { "" } else { "s" }
    );

    // Decrypt each matched secret.
    let mut secrets: BTreeMap<String, String> = BTreeMap::new();
    for path in &matched {
        let ciphertext = store::read_secret(&ctx.store, path)?;
        let decoded =
            secret_value::decode(&crypto::decrypt_with_identities(&ciphertext, &identities)?);
        super::get::warn_if_expired(path, &decoded);
        let plaintext = String::from_utf8(decoded.data).map_err(|e| {
            HimitsuError::DecryptionFailed(format!("non-UTF-8 secret at '{path}': {e}"))
        })?;
        secrets.insert((*path).clone(), plaintext);
    }

    // Build the output document.
    let output_text = match args.format.as_str() {
        "json" => build_json(&secrets)?,
        _ => build_yaml(&secrets),
    };

    if args.stdout {
        print!("{output_text}");
        return Ok(());
    }

    // Encrypt via SOPS and write to file.
    let project_cfg = load_project_config().map(|(c, _)| c);
    let out_path = resolve_output_path(&args)?;
    write_encrypted(&out_path, &output_text, &args.format, project_cfg.as_ref())?;

    Ok(())
}

// ── Glob matching ───────────────────────────────────────────────────────────

/// Simple glob matcher supporting `*` (single segment) and `**` (any depth).
///
/// Patterns are matched against `/`-separated secret paths:
/// - `prod/*`        matches `prod/FOO` but not `prod/sub/FOO`
/// - `prod/**`       matches `prod/FOO` and `prod/sub/FOO`
/// - `**/API_KEY`    matches `API_KEY`, `prod/API_KEY`, `a/b/API_KEY`
/// - `prod/*/KEY`    matches `prod/sub/KEY` but not `prod/a/b/KEY`
fn glob_match(pattern: &str, path: &str) -> bool {
    let pat_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();
    glob_match_parts(&pat_parts, &path_parts)
}

fn glob_match_parts(pat: &[&str], path: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }

    let p = pat[0];

    if p == "**" {
        // `**` at the end matches everything remaining.
        if pat.len() == 1 {
            return true;
        }
        // Try matching the rest of the pattern at every position.
        for i in 0..=path.len() {
            if glob_match_parts(&pat[1..], &path[i..]) {
                return true;
            }
        }
        return false;
    }

    if path.is_empty() {
        return false;
    }

    if segment_match(p, path[0]) {
        glob_match_parts(&pat[1..], &path[1..])
    } else {
        false
    }
}

/// Match a single segment: `*` matches any single segment, otherwise literal match.
fn segment_match(pattern: &str, segment: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Support patterns like `DB_*` or `*_KEY` with a simple wildcard in segment.
    if pattern.contains('*') {
        return wildcard_match(pattern, segment);
    }
    pattern == segment
}

/// Simple wildcard match within a single segment (no `/`).
/// Supports `*` as a wildcard for zero or more characters.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }

    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(idx) => {
                // First part must match at the start.
                if i == 0 && idx != 0 {
                    return false;
                }
                pos += idx + part.len();
            }
            None => return false,
        }
    }
    // Last part must match at the end.
    if let Some(last) = parts.last() {
        if !last.is_empty() && !text.ends_with(last) {
            return false;
        }
    }
    true
}

// ── Output formatting ───────────────────────────────────────────────────────

/// Build a flat YAML document from the secrets map.
fn build_yaml(secrets: &BTreeMap<String, String>) -> String {
    let mut out = String::from("# Exported by himitsu\n");
    for (k, v) in secrets {
        // Use the last path component as a simple key, or the full path
        // with slashes replaced by underscores for uniqueness.
        let key = path_to_key(k);
        let escaped = v
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        out.push_str(&format!("{key}: \"{escaped}\"\n"));
    }
    out
}

/// Build a JSON document from the secrets map.
fn build_json(secrets: &BTreeMap<String, String>) -> Result<String> {
    let map: BTreeMap<String, &str> = secrets
        .iter()
        .map(|(k, v)| (path_to_key(k), v.as_str()))
        .collect();
    serde_json::to_string_pretty(&map).map_err(|e| e.into())
}

/// Convert a secret path to a flat key name.
///
/// If the path has multiple components, join them with `_` so the output
/// stays a valid YAML/JSON key without nesting.
/// e.g. `prod/db/PASSWORD` → `prod_db_PASSWORD`
fn path_to_key(path: &str) -> String {
    path.replace('/', "_")
}

// ── Output path ─────────────────────────────────────────────────────────────

/// Determine the output file path.
fn resolve_output_path(args: &ExportArgs) -> Result<PathBuf> {
    if let Some(ref out) = args.output {
        return Ok(PathBuf::from(out));
    }

    // Derive from pattern: strip glob chars, take first meaningful segment.
    let stem = args
        .pattern
        .split('/')
        .find(|s| !s.is_empty() && *s != "*" && *s != "**")
        .unwrap_or("export");

    let ext = match args.format.as_str() {
        "json" => "sops.json",
        _ => "sops.yaml",
    };
    Ok(PathBuf::from(format!("{stem}.{ext}")))
}

// ── SOPS encryption ─────────────────────────────────────────────────────────

/// Encrypt content via `sops` and write to the output path.
/// Reuses the same SOPS invocation pattern as generate.rs.
fn write_encrypted(
    out_path: &Path,
    content: &str,
    format: &str,
    project_cfg: Option<&ProjectConfig>,
) -> Result<()> {
    let recipients: Vec<String> = project_cfg
        .and_then(|c| c.generate.as_ref())
        .map(|g| g.age_recipients.clone())
        .unwrap_or_default();

    if recipients.is_empty() {
        return Err(HimitsuError::GenerateError(
            "no `generate.age_recipients` configured in himitsu.yaml; \
             add recipients or use --stdout for plaintext"
                .into(),
        ));
    }

    let age_arg = recipients.join(",");
    let input_type = format;
    let output_type = format;

    let mut child = StdCommand::new("sops")
        .args([
            "--encrypt",
            "--age",
            &age_arg,
            "--input-type",
            input_type,
            "--output-type",
            output_type,
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
        .write_all(content.as_bytes())
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

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(out_path, &result.stdout)?;
    eprintln!("Exported: {}", out_path.display());
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_star_matches_single_level() {
        assert!(glob_match("prod/*", "prod/API_KEY"));
        assert!(glob_match("prod/*", "prod/DB_PASS"));
        assert!(!glob_match("prod/*", "prod/sub/KEY"));
        assert!(!glob_match("prod/*", "staging/API_KEY"));
    }

    #[test]
    fn glob_doublestar_matches_any_depth() {
        assert!(glob_match("**/API_KEY", "API_KEY"));
        assert!(glob_match("**/API_KEY", "prod/API_KEY"));
        assert!(glob_match("**/API_KEY", "a/b/c/API_KEY"));
        assert!(!glob_match("**/API_KEY", "prod/DB_PASS"));
    }

    #[test]
    fn glob_doublestar_at_end() {
        assert!(glob_match("prod/**", "prod/FOO"));
        assert!(glob_match("prod/**", "prod/sub/FOO"));
        assert!(!glob_match("prod/**", "staging/FOO"));
    }

    #[test]
    fn glob_mixed_patterns() {
        assert!(glob_match("prod/*/KEY", "prod/sub/KEY"));
        assert!(!glob_match("prod/*/KEY", "prod/a/b/KEY"));
    }

    #[test]
    fn glob_exact_match() {
        assert!(glob_match("prod/API_KEY", "prod/API_KEY"));
        assert!(!glob_match("prod/API_KEY", "prod/DB_PASS"));
    }

    #[test]
    fn wildcard_within_segment() {
        assert!(glob_match("prod/DB_*", "prod/DB_PASS"));
        assert!(glob_match("prod/DB_*", "prod/DB_HOST"));
        assert!(!glob_match("prod/DB_*", "prod/API_KEY"));
    }

    #[test]
    fn path_to_key_conversion() {
        assert_eq!(path_to_key("prod/API_KEY"), "prod_API_KEY");
        assert_eq!(path_to_key("API_KEY"), "API_KEY");
        assert_eq!(path_to_key("a/b/c"), "a_b_c");
    }

    #[test]
    fn output_path_from_pattern() {
        let args = ExportArgs {
            pattern: "prod/*".into(),
            output: None,
            format: "yaml".into(),
            stdout: false,
        };
        assert_eq!(
            resolve_output_path(&args).unwrap(),
            PathBuf::from("prod.sops.yaml")
        );
    }

    #[test]
    fn output_path_explicit() {
        let args = ExportArgs {
            pattern: "prod/*".into(),
            output: Some("out/secrets.yaml".into()),
            format: "yaml".into(),
            stdout: false,
        };
        assert_eq!(
            resolve_output_path(&args).unwrap(),
            PathBuf::from("out/secrets.yaml")
        );
    }

    #[test]
    fn output_path_json_format() {
        let args = ExportArgs {
            pattern: "staging/**".into(),
            output: None,
            format: "json".into(),
            stdout: false,
        };
        assert_eq!(
            resolve_output_path(&args).unwrap(),
            PathBuf::from("staging.sops.json")
        );
    }

    #[test]
    fn build_yaml_output() {
        let mut secrets = BTreeMap::new();
        secrets.insert("prod/API_KEY".into(), "secret123".into());
        secrets.insert("prod/DB_PASS".into(), "p@ss".into());
        let yaml = build_yaml(&secrets);
        assert!(yaml.contains("prod_API_KEY: \"secret123\""));
        assert!(yaml.contains("prod_DB_PASS: \"p@ss\""));
    }

    #[test]
    fn build_json_output() {
        let mut secrets = BTreeMap::new();
        secrets.insert("prod/API_KEY".into(), "secret123".into());
        let json = build_json(&secrets).unwrap();
        assert!(json.contains("\"prod_API_KEY\": \"secret123\""));
    }
}
