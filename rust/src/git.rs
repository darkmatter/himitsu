use std::path::Path;
use std::process::Command;

use crate::error::{HimitsuError, Result};

/// Run a git command with the given arguments in the specified directory.
/// Returns the stdout on success, or an error with stderr on failure.
pub fn run(args: &[&str], cwd: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
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

/// Get the git status (short form).
pub fn status(cwd: &Path) -> Result<String> {
    run(&["status", "--short"], cwd)
}

/// Initialize a new git repository.
pub fn init(cwd: &Path) -> Result<String> {
    run(&["init"], cwd)
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
}
