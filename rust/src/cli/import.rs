use std::process::Command;

use clap::Args;

use super::Context;
use crate::cli::set::set_plaintext;
use crate::error::{HimitsuError, Result};
use crate::remote::store::secrets_dir;

/// Import secrets from external stores (1Password today; SOPS planned).
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Target secret path, e.g. `prod/STRIPE_KEY`.
    pub path: String,

    /// 1Password reference to import. Expects a full `op://vault/item/field`
    /// reference. Bulk whole-item import is not yet implemented.
    #[arg(long, conflicts_with = "sops")]
    pub op: Option<String>,

    /// (Not yet implemented) Path to a SOPS-encrypted file to import.
    #[arg(long)]
    pub sops: Option<String>,

    /// Overwrite an existing secret at the target path.
    #[arg(long)]
    pub overwrite: bool,

    /// Skip git commit and push (useful for batch imports).
    #[arg(long)]
    pub no_push: bool,
}

pub fn run(args: ImportArgs, ctx: &Context) -> Result<()> {
    if args.sops.is_some() {
        return Err(HimitsuError::NotSupported(
            "SOPS import is not yet implemented — use --op for now".into(),
        ));
    }

    let op_ref = args.op.as_deref().ok_or_else(|| {
        HimitsuError::InvalidReference(
            "missing source: pass --op <op://vault/item/field>".into(),
        )
    })?;

    // Validate the op reference shape. `op read` supports
    // `op://vault/item/field` (single field). A bare `op://vault/item`
    // refers to an entire item and would need `op item get` + field
    // enumeration — not yet implemented.
    let trimmed = op_ref
        .strip_prefix("op://")
        .ok_or_else(|| HimitsuError::InvalidReference(
            format!("1Password reference must start with `op://` (got {op_ref:?})"),
        ))?;
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    match segments.len() {
        3 => {} // vault/item/field — good
        2 => {
            return Err(HimitsuError::NotSupported(
                "whole-item import (op://vault/item) is not yet implemented — \
                 pass a field reference like op://vault/item/field"
                    .into(),
            ));
        }
        _ => {
            return Err(HimitsuError::InvalidReference(format!(
                "expected op://vault/item/field, got {op_ref:?}"
            )));
        }
    }

    // Guard against clobbering an existing secret.
    if !args.overwrite && secret_exists_at(&ctx.store, &args.path) {
        return Err(HimitsuError::InvalidReference(format!(
            "secret already exists at {}: pass --overwrite to replace it",
            args.path
        )));
    }

    let plaintext = op_read("op", op_ref)?;
    let stored = set_plaintext(ctx, &args.path, plaintext.as_bytes(), args.no_push)?;
    println!("Imported {stored} from {op_ref}");
    Ok(())
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
///
/// `program` is the `op` binary name (parameterized only so tests can point
/// at a non-existent path to exercise the "missing binary" error branch
/// without clobbering the process-wide `PATH`). Production callers always
/// pass `"op"` and rely on PATH lookup.
///
/// Surfaces the subprocess stderr verbatim on failure. `op` is not wrapped
/// or mocked — callers must have it installed and be signed in.
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
            // Common case: `[ERROR] ... You are not currently signed in.`
            format!("`op read` failed: {trimmed}")
        };
        return Err(HimitsuError::External(detail));
    }

    String::from_utf8(output.stdout).map_err(|e| {
        HimitsuError::External(format!("`op read` returned non-UTF-8 output: {e}"))
    })
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
        let TestCli { cmd: TestCmd::Import(a) } =
            TestCli::try_parse_from(full).expect("parse ok");
        a
    }

    #[test]
    fn parses_op_and_path() {
        let a = parse(&["--op", "op://Personal/Stripe/credential", "prod/STRIPE_KEY"]);
        assert_eq!(a.path, "prod/STRIPE_KEY");
        assert_eq!(a.op.as_deref(), Some("op://Personal/Stripe/credential"));
        assert!(!a.overwrite);
        assert!(!a.no_push);
    }

    #[test]
    fn parses_flags() {
        let a = parse(&[
            "--op",
            "op://v/i/f",
            "--overwrite",
            "--no-push",
            "prod/X",
        ]);
        assert!(a.overwrite);
        assert!(a.no_push);
    }

    #[test]
    fn op_and_sops_conflict() {
        let res = TestCli::try_parse_from([
            "test", "import", "--op", "op://v/i/f", "--sops", "x.yaml", "prod/X",
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
        };
        let err = run(args, &ctx).unwrap_err();
        assert!(
            matches!(err, HimitsuError::InvalidReference(ref m) if m.contains("missing source")),
            "got {err:?}"
        );
    }

    #[test]
    fn sops_branch_is_not_supported() {
        let args = ImportArgs {
            path: "prod/X".into(),
            op: None,
            sops: Some("secrets.enc.yaml".into()),
            overwrite: false,
            no_push: false,
        };
        let ctx = Context {
            data_dir: std::path::PathBuf::from("/tmp"),
            state_dir: std::path::PathBuf::from("/tmp"),
            store: std::path::PathBuf::new(),
            recipients_path: None,
        };
        let err = run(args, &ctx).unwrap_err();
        assert!(matches!(err, HimitsuError::NotSupported(_)), "got {err:?}");
    }

    #[test]
    fn rejects_non_op_reference() {
        let args = ImportArgs {
            path: "prod/X".into(),
            op: Some("https://example.com/foo".into()),
            sops: None,
            overwrite: false,
            no_push: false,
        };
        let ctx = Context {
            data_dir: std::path::PathBuf::from("/tmp"),
            state_dir: std::path::PathBuf::from("/tmp"),
            store: std::path::PathBuf::new(),
            recipients_path: None,
        };
        let err = run(args, &ctx).unwrap_err();
        assert!(matches!(err, HimitsuError::InvalidReference(_)), "got {err:?}");
    }

    #[test]
    fn whole_item_import_not_yet_implemented() {
        let args = ImportArgs {
            path: "prod/".into(),
            op: Some("op://Personal/Stripe".into()),
            sops: None,
            overwrite: false,
            no_push: false,
        };
        let ctx = Context {
            data_dir: std::path::PathBuf::from("/tmp"),
            state_dir: std::path::PathBuf::from("/tmp"),
            store: std::path::PathBuf::new(),
            recipients_path: None,
        };
        let err = run(args, &ctx).unwrap_err();
        assert!(matches!(err, HimitsuError::NotSupported(_)), "got {err:?}");
    }

    /// Exercises the real subprocess plumbing for `op_read` by pointing it
    /// at an absolute path we know does not exist. This verifies the
    /// "missing binary" error branch without touching the process-wide
    /// `PATH` (which would race with sibling tests).
    #[test]
    fn op_read_errors_when_binary_missing() {
        let fake = "/nonexistent/himitsu-test-op-binary";
        let err = op_read(fake, "op://v/i/f")
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
}
