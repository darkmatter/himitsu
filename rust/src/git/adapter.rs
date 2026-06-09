use std::path::{Path, PathBuf};

use crate::error::Result;

/// Abstraction over git operations used by Context's auto-commit/push/pull cycle.
///
/// Production uses [`CliGitAdapter`](crate::git::CliGitAdapter) which shells out
/// to the `git` binary. Tests can substitute
/// [`InMemoryGitAdapter`](crate::git::InMemoryGitAdapter) to avoid filesystem
/// git deps.
pub trait GitAdapter: Send + Sync {
    /// Run an arbitrary git command, returning stdout on success.
    fn run(&self, args: &[&str], cwd: &Path) -> Result<String>;

    /// Stage all changes and commit with the given message.
    fn commit(&self, cwd: &Path, message: &str) -> Result<String>;

    /// Push to the remote origin.
    fn push(&self, cwd: &Path) -> Result<String>;

    /// Returns true when the repo has at least one named remote configured.
    fn has_any_remote(&self, cwd: &Path) -> bool;

    /// Returns true when the repo has commits ahead of its upstream.
    fn has_unpushed_commits(&self, cwd: &Path) -> bool;

    /// List absolute paths of initialized submodules for the repo at `cwd`.
    fn list_submodules(&self, cwd: &Path) -> Vec<PathBuf>;

    /// Move detached HEAD onto default branch when HEAD matches branch tip.
    fn ensure_on_branch(&self, cwd: &Path) -> Result<()>;
}
