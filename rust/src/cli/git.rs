use clap::Args;
use tracing::debug;

use super::Context;
use crate::config;
use crate::error::{HimitsuError, Result};

/// Run git commands inside a store checkout (or all stores with --all).
///
/// Examples:
///   himitsu git status
///   himitsu git log --oneline
///   himitsu --remote org/repo git push
///   himitsu git --all status
#[derive(Debug, Args)]
pub struct GitArgs {
    /// Run the git command in all stores.
    #[arg(long)]
    pub all: bool,
    /// Arguments forwarded to git (e.g. `status`, `log --oneline`, `push`).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub fn run(args: GitArgs, ctx: &Context) -> Result<()> {
    // Default to `git status` when no sub-args are provided.
    let git_args: Vec<String> = if args.args.is_empty() {
        debug!("no git args provided, defaulting to `git status`");
        vec!["status".to_string()]
    } else {
        args.args
    };

    // ── --all: run in every known store ──────────────────────────────────────
    if args.all {
        let remotes = crate::remote::list_remotes()?;
        if remotes.is_empty() {
            return Err(HimitsuError::Remote(
                "no stores found; use `himitsu remote add <org/repo>` to add one".into(),
            ));
        }
        for slug in &remotes {
            let (org, repo) = config::validate_remote_slug(slug)?;
            let store_path = config::store_checkout(org, repo);
            println!("=== {slug} ===");
            exec_git(&store_path, &git_args)?;
        }
        return Ok(());
    }

    // ── Use the resolved ctx.store if non-empty ───────────────────────────────
    if !ctx.store.as_os_str().is_empty() {
        let git_root = ctx.git_root().ok_or_else(|| {
            HimitsuError::Git(format!(
                "store at {} is not a git repository",
                ctx.store.display()
            ))
        })?;
        return exec_git(&git_root, &git_args);
    }

    // ── Try to resolve a default store ───────────────────────────────────────
    match config::resolve_store(None) {
        Ok(store_path) => {
            let git_root = if store_path.join(".git").exists() {
                store_path.clone()
            } else {
                config::find_git_root(&store_path).ok_or_else(|| {
                    HimitsuError::Git(format!(
                        "store at {} is not a git repository",
                        store_path.display()
                    ))
                })?
            };
            exec_git(&git_root, &git_args)
        }
        Err(_) => Err(HimitsuError::Git(
            "no store resolved; use --remote, --store, or --all".into(),
        )),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
