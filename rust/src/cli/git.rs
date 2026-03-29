use std::io::{self, Write};

use clap::Args;
use tracing::debug;

use super::Context;
use crate::error::{HimitsuError, Result};

/// Run git commands inside the himitsu user directory (~/.himitsu).
///
/// This is a convenience passthrough — equivalent to:
///   cd ~/.himitsu && git <args>
///
/// Examples:
///   himitsu git status
///   himitsu git log --oneline
///   himitsu git remote add origin git@github.com:you/secrets.git
///   himitsu git push
#[derive(Debug, Args)]
pub struct GitArgs {
    /// Arguments forwarded to git (e.g. `status`, `log --oneline`, `push`).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub fn run(args: GitArgs, ctx: &Context) -> Result<()> {
    let home = &ctx.data_dir;

    // ── 1. Is himitsu initialized at all? ────────────────────────
    if !home.join("key").exists() {
        return prompt_init(ctx);
    }

    // ── 2. Is the home directory a git repo? ─────────────────────
    if !home.join(".git").exists() {
        eprintln!(
            "The himitsu directory at {} is not a git repository.",
            home.display()
        );
        eprint!("Would you like to initialize it as one now? [y/N] ");
        io::stderr().flush()?;

        if confirm_prompt()? {
            let output = std::process::Command::new("git")
                .args(["init"])
                .current_dir(home)
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
                .map_err(|e| HimitsuError::Git(format!("failed to run git init: {e}")))?;

            if !output.success() {
                return Err(HimitsuError::Git("git init failed".into()));
            }
            eprintln!();

            // If the user only ran `himitsu git` with no args, they probably
            // just wanted the init — we're done.
            if args.args.is_empty() {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    }

    // ── 3. Run the requested git command ─────────────────────────
    if args.args.is_empty() {
        // Bare `himitsu git` — show status as a sensible default.
        debug!("no git args provided, defaulting to `git status`");
        return exec_git(home, &["status".to_string()]);
    }

    exec_git(home, &args.args)
}

/// Execute git with full stdio inheritance so interactive commands
/// (editors, pagers, prompts) work correctly.
fn exec_git(cwd: &std::path::Path, args: &[String]) -> Result<()> {
    debug!("git {} in {}", args.join(" "), cwd.display());

    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| HimitsuError::Git(format!("failed to execute git: {e}")))?;

    if !status.success() {
        let code = status.code().unwrap_or(1);
        std::process::exit(code);
    }

    Ok(())
}

/// The himitsu home directory doesn't exist at all — offer to initialize.
fn prompt_init(ctx: &Context) -> Result<()> {
    eprintln!("You have not initialized your secrets directory (~/.himitsu).");
    eprint!("Would you like to do so now? [y/N] ");
    io::stderr().flush()?;

    if confirm_prompt()? {
        eprintln!();
        let init_args = super::init::InitArgs { json: false };
        super::init::run(init_args, ctx)?;

        eprintln!();
        eprintln!("Tip: run `himitsu git init` to version-control your secrets directory.");
    }

    Ok(())
}

/// Read a single line from stdin and return true if it starts with 'y' or 'Y'.
fn confirm_prompt() -> Result<bool> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_prompt_recognises_yes() {
        // Unit-testable: the parsing logic itself is trivial,
        // but the interactive prompt is not easily testable without
        // a pty. We test the git execution in integration tests.
        assert!(confirm_logic("y"));
        assert!(confirm_logic("Y"));
        assert!(confirm_logic("yes"));
        assert!(confirm_logic("YES"));
        assert!(!confirm_logic("n"));
        assert!(!confirm_logic(""));
        assert!(!confirm_logic("no"));
        assert!(!confirm_logic("yolo"));
    }

    /// Extracted logic from `confirm_prompt` for testability.
    fn confirm_logic(input: &str) -> bool {
        let trimmed = input.trim();
        trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
    }

    #[test]
    fn exec_git_in_repo() {
        let tmp = tempfile::tempdir().unwrap();
        crate::git::init(tmp.path()).unwrap();

        // Should succeed — `git status` in a valid repo.
        let result = exec_git(tmp.path(), &["status".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn exec_git_version_needs_no_repo() {
        let tmp = tempfile::tempdir().unwrap();

        // `git version` works anywhere, no repo needed.
        let result = exec_git(tmp.path(), &["version".to_string()]);
        assert!(result.is_ok());
    }
}
