use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{HimitsuError, Result};
use crate::git::adapter::GitAdapter;

/// Mutable state recorded by [`InMemoryGitAdapter`].
#[derive(Default)]
struct InMemoryState {
    /// Paths configured to report at least one named remote.
    remotes: HashSet<PathBuf>,
    /// Paths configured to report unpushed commits.
    unpushed: HashSet<PathBuf>,
    /// Map of repo path -> initialized submodule paths.
    submodules: HashMap<PathBuf, Vec<PathBuf>>,
    /// Log of `(cwd, message)` for each `commit` call.
    commits: Vec<(PathBuf, String)>,
    /// Log of `(cwd, args)` for each `run` call.
    runs: Vec<(PathBuf, Vec<String>)>,
    /// Paths considered to be on a branch (not detached HEAD).
    on_branch: HashSet<PathBuf>,
}

/// In-memory [`GitAdapter`] for tests.
///
/// Every operation succeeds by default and is recorded so tests can assert on
/// the interactions. No `git` binary or filesystem repo is required.
#[derive(Default)]
pub struct InMemoryGitAdapter {
    state: Mutex<InMemoryState>,
}

impl InMemoryGitAdapter {
    /// Create an adapter with empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `cwd` as having at least one configured remote.
    pub fn set_remote(&self, cwd: &Path) {
        self.state.lock().unwrap().remotes.insert(cwd.to_path_buf());
    }

    /// Mark `cwd` as having unpushed commits.
    pub fn set_unpushed(&self, cwd: &Path) {
        self.state
            .lock()
            .unwrap()
            .unpushed
            .insert(cwd.to_path_buf());
    }

    /// Register `submodules` as the initialized submodules of `cwd`.
    pub fn set_submodules(&self, cwd: &Path, submodules: Vec<PathBuf>) {
        self.state
            .lock()
            .unwrap()
            .submodules
            .insert(cwd.to_path_buf(), submodules);
    }

    /// Mark `cwd` as being on a branch (so `ensure_on_branch` succeeds).
    pub fn set_on_branch(&self, cwd: &Path) {
        self.state
            .lock()
            .unwrap()
            .on_branch
            .insert(cwd.to_path_buf());
    }

    /// Snapshot the recorded `(cwd, message)` commit log.
    pub fn commits(&self) -> Vec<(PathBuf, String)> {
        self.state.lock().unwrap().commits.clone()
    }

    /// Snapshot the recorded `(cwd, args)` run log.
    pub fn runs(&self) -> Vec<(PathBuf, Vec<String>)> {
        self.state.lock().unwrap().runs.clone()
    }
}

impl GitAdapter for InMemoryGitAdapter {
    fn run(&self, args: &[&str], cwd: &Path) -> Result<String> {
        self.state.lock().unwrap().runs.push((
            cwd.to_path_buf(),
            args.iter().map(|s| s.to_string()).collect(),
        ));
        Ok(String::new())
    }

    fn commit(&self, cwd: &Path, message: &str) -> Result<String> {
        self.state
            .lock()
            .unwrap()
            .commits
            .push((cwd.to_path_buf(), message.to_string()));
        Ok(String::new())
    }

    fn push(&self, _cwd: &Path) -> Result<String> {
        Ok(String::new())
    }

    fn has_any_remote(&self, cwd: &Path) -> bool {
        self.state.lock().unwrap().remotes.contains(cwd)
    }

    fn has_unpushed_commits(&self, cwd: &Path) -> bool {
        self.state.lock().unwrap().unpushed.contains(cwd)
    }

    fn list_submodules(&self, cwd: &Path) -> Vec<PathBuf> {
        self.state
            .lock()
            .unwrap()
            .submodules
            .get(cwd)
            .cloned()
            .unwrap_or_default()
    }

    fn ensure_on_branch(&self, cwd: &Path) -> Result<()> {
        if self.state.lock().unwrap().on_branch.contains(cwd) {
            Ok(())
        } else {
            Err(HimitsuError::Git(format!(
                "in-memory: {} is not on a branch",
                cwd.display()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_is_recorded_and_succeeds() {
        let adapter = InMemoryGitAdapter::new();
        let cwd = PathBuf::from("/repo");
        adapter.run(&["status", "--porcelain"], &cwd).unwrap();
        let runs = adapter.runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, cwd);
        assert_eq!(runs[0].1, vec!["status", "--porcelain"]);
    }

    #[test]
    fn commit_is_recorded() {
        let adapter = InMemoryGitAdapter::new();
        let cwd = PathBuf::from("/repo");
        adapter.commit(&cwd, "himitsu: set foo").unwrap();
        let commits = adapter.commits();
        assert_eq!(commits, vec![(cwd, "himitsu: set foo".to_string())]);
    }

    #[test]
    fn has_any_remote_reflects_configuration() {
        let adapter = InMemoryGitAdapter::new();
        let cwd = PathBuf::from("/repo");
        assert!(!adapter.has_any_remote(&cwd));
        adapter.set_remote(&cwd);
        assert!(adapter.has_any_remote(&cwd));
    }

    #[test]
    fn ensure_on_branch_errors_until_configured() {
        let adapter = InMemoryGitAdapter::new();
        let cwd = PathBuf::from("/repo");
        assert!(adapter.ensure_on_branch(&cwd).is_err());
        adapter.set_on_branch(&cwd);
        assert!(adapter.ensure_on_branch(&cwd).is_ok());
    }
}
