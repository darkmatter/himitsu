use std::path::Path;
use std::process::Command;

use crate::error::{HimitsuError, Result};

/// Run a git command with the given arguments in the specified directory.
/// Returns the stdout on success, or an error with stderr on failure.
///
/// All invocations are forced into non-interactive mode and have commit/tag
/// signing disabled. Two interactions with the surrounding environment would
/// otherwise let himitsu hang indefinitely on what should be a fast operation:
///
///   * `commit.gpgsign = true` with an SSH signer (notably 1Password's
///     `op-ssh-sign`) opens a native biometric/password dialog on every
///     commit — including the silent auto-commits himitsu issues from `set`,
///     `init`, `rekey`, etc. We aren't asking the user to sign these on
///     their own behalf, so we strip signing for himitsu-spawned commits.
///   * Stalled SSH/HTTPS fetches can prompt for credentials over the TTY.
///     `GIT_TERMINAL_PROMPT=0` + an `echo` askpass make those fail fast
///     instead of blocking.
fn git_command(args: &[&str], cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo");
    cmd
}

pub fn run(args: &[&str], cwd: &Path) -> Result<String> {
    let output = git_command(args, cwd)
        .output()
        .map_err(|e| HimitsuError::Git(format!("failed to execute git: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(HimitsuError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )))
    }
}

/// Clone a repository into the destination directory.
pub fn clone(url: &str, dest: &Path) -> Result<String> {
    let dest_str = dest.to_string_lossy();
    let parent = dest.parent().unwrap_or(dest);
    std::fs::create_dir_all(parent)?;

    let output = Command::new("git")
        .args(["clone", url, &dest_str])
        .output()
        .map_err(|e| HimitsuError::Git(format!("failed to execute git clone: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(HimitsuError::Git(format!(
            "git clone failed: {}",
            stderr.trim()
        )))
    }
}

/// Clone a repository in non-interactive mode (no SSH prompts, no terminal
/// password dialogs). Used for lazy cloning where a fast failure is preferred
/// over blocking indefinitely.
pub fn clone_noninteractive(url: &str, dest: &Path) -> Result<String> {
    let dest_str = dest.to_string_lossy();
    let parent = dest.parent().unwrap_or(dest);
    std::fs::create_dir_all(parent)?;

    let output = Command::new("git")
        .args(["clone", url, &dest_str])
        // Disable interactive prompts so the call fails fast instead of hanging.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "echo")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=3 -o ConnectionAttempts=1",
        )
        .output()
        .map_err(|e| HimitsuError::Git(format!("failed to execute git clone: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(HimitsuError::Git(format!(
            "git clone failed: {}",
            stderr.trim()
        )))
    }
}

/// Stage all changes and commit with the given message.
pub fn commit(cwd: &Path, message: &str) -> Result<String> {
    run(&["add", "-A"], cwd)?;
    run(&["commit", "-m", message], cwd)
}

/// Push to the remote origin.
pub fn push(cwd: &Path) -> Result<String> {
    run(&["push"], cwd)
}

/// Pull from the remote origin.
pub fn pull(cwd: &Path) -> Result<String> {
    run(&["pull"], cwd)
}

/// Fetch origin and update the working tree to the remote default branch.
///
/// Uses a normal fast-forward pull when an upstream is configured. For local
/// stores that were initialized before an upstream existed, falls back to
/// checking out `origin/HEAD` (or `origin/main`/`origin/master`) so adding an
/// existing store restores the files instead of leaving the checkout empty.
pub fn pull_or_checkout_origin(cwd: &Path) -> Result<()> {
    run(&["fetch", "--quiet", "origin"], cwd)?;
    if run(&["pull", "--ff-only", "--recurse-submodules"], cwd).is_ok() {
        return Ok(());
    }

    let branch = origin_default_branch(cwd)?;
    run(
        &["checkout", "-B", &branch, &format!("origin/{branch}")],
        cwd,
    )?;
    let _ = run(&["submodule", "update", "--init", "--recursive"], cwd);
    Ok(())
}

fn origin_default_branch(cwd: &Path) -> Result<String> {
    if let Ok(default) = run(
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        cwd,
    ) {
        if let Some(branch) = default.trim().strip_prefix("origin/") {
            if !branch.is_empty() {
                return Ok(branch.to_string());
            }
        }
    }

    for branch in ["main", "master"] {
        if run(&["rev-parse", "--verify", &format!("origin/{branch}")], cwd).is_ok() {
            return Ok(branch.to_string());
        }
    }

    Err(HimitsuError::Git(
        "could not determine origin default branch".to_string(),
    ))
}

/// Get the git status (short form).
pub fn status(cwd: &Path) -> Result<String> {
    run(&["status", "--short"], cwd)
}

/// Initialize a new git repository.
pub fn init(cwd: &Path) -> Result<String> {
    run(&["init"], cwd)
}

/// Returns true when the repo at `cwd` has at least one named remote configured.
///
/// Used to detect the "commits go nowhere" failure mode where a local store
/// has accumulated commits but never had `origin` (or any remote) set up, so
/// `git push` silently fails on every mutation.
pub fn has_any_remote(cwd: &Path) -> bool {
    match run(&["remote"], cwd) {
        Ok(out) => out.lines().any(|l| !l.trim().is_empty()),
        Err(_) => false,
    }
}

/// Add a named remote pointing at `url`. Errors if the remote already exists.
pub fn add_remote(cwd: &Path, name: &str, url: &str) -> Result<String> {
    run(&["remote", "add", name, url], cwd)
}

/// List absolute paths of initialized submodules for the repo at `cwd`.
///
/// Reads `.gitmodules` directly so callers don't depend on a POSIX shell.
/// Submodules that appear in `.gitmodules` but aren't checked out (no `.git`
/// file/dir inside) are filtered so callers can safely run git in each path.
pub fn list_submodules(cwd: &Path) -> Vec<std::path::PathBuf> {
    let Ok(out) = run(
        &[
            "config",
            "-f",
            ".gitmodules",
            "--get-regexp",
            r"submodule\..*\.path",
        ],
        cwd,
    ) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| l.split_once(' ').map(|(_, p)| p.to_string()))
        .map(|rel| cwd.join(rel))
        .filter(|p| p.join(".git").exists())
        .collect()
}

/// Returns true when the repo at `cwd` has commits ahead of its upstream.
/// Returns false when no upstream is configured — treat as nothing to push.
pub fn has_unpushed_commits(cwd: &Path) -> bool {
    match run(&["rev-list", "--count", "@{u}..HEAD"], cwd) {
        Ok(out) => out
            .trim()
            .parse::<u32>()
            .map(|n| n > 0)
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Move the repo at `cwd` from detached HEAD onto its default branch, but
/// only when the detached commit already matches the tip of that branch.
///
/// `git clone --recurse-submodules` leaves submodules detached at the pinned
/// commit, and a detached HEAD can't be pushed. Auto-checking out is safe
/// when HEAD == tip of origin/<default> (the pointer and branch tip agree).
/// If they diverge the submodule is genuinely pinned — don't silently unpin;
/// return an error with context so the caller can surface it.
///
/// No-op when HEAD is already symbolic.
pub fn ensure_on_branch(cwd: &Path) -> Result<()> {
    if run(&["symbolic-ref", "-q", "HEAD"], cwd).is_ok() {
        return Ok(());
    }
    let head = run(&["rev-parse", "HEAD"], cwd)?.trim().to_string();
    let default = run(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], cwd)?;
    let branch = default
        .trim()
        .strip_prefix("origin/")
        .unwrap_or_else(|| default.trim())
        .to_string();
    let tip = run(&["rev-parse", &format!("origin/{branch}")], cwd)?
        .trim()
        .to_string();
    if head != tip {
        return Err(HimitsuError::Git(format!(
            "detached HEAD at {} is not the tip of origin/{branch}; \
             checkout the branch manually before writing",
            &head[..head.len().min(7)]
        )));
    }
    run(
        &["checkout", "-B", &branch, &format!("origin/{branch}")],
        cwd,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_captures_output() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path()).unwrap();
        let output = run(&["status"], tmp.path()).unwrap();
        // A fresh repo should have some status output
        assert!(output.is_empty() || output.contains("branch"));
    }

    #[test]
    fn run_returns_error_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        // Running git log in a non-git directory should fail
        let result = run(&["log"], tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn has_any_remote_false_on_fresh_repo() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path()).unwrap();
        assert!(
            !has_any_remote(tmp.path()),
            "fresh repo should have no remotes"
        );
    }

    #[test]
    fn has_any_remote_true_after_add() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path()).unwrap();
        add_remote(tmp.path(), "origin", "git@github.com:foo/bar.git").unwrap();
        assert!(has_any_remote(tmp.path()));
    }

    #[test]
    fn has_any_remote_false_outside_repo() {
        let tmp = tempfile::tempdir().unwrap();
        // Not a git repo at all → must not panic, must report no remote.
        assert!(!has_any_remote(tmp.path()));
    }

    #[test]
    fn list_submodules_empty_without_gitmodules() {
        let tmp = tempfile::tempdir().unwrap();
        init(tmp.path()).unwrap();
        assert!(list_submodules(tmp.path()).is_empty());
    }

    #[test]
    fn list_submodules_outside_repo_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_submodules(tmp.path()).is_empty());
    }
}
