use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git::adapter::GitAdapter;

/// [`GitAdapter`] implementation that shells out to the `git` binary by
/// delegating to the free functions in [`crate::git`].
pub struct CliGitAdapter;

impl GitAdapter for CliGitAdapter {
    fn run(&self, args: &[&str], cwd: &Path) -> Result<String> {
        super::run(args, cwd)
    }

    fn commit(&self, cwd: &Path, message: &str) -> Result<String> {
        super::commit(cwd, message)
    }

    fn push(&self, cwd: &Path) -> Result<String> {
        super::push(cwd)
    }

    fn has_any_remote(&self, cwd: &Path) -> bool {
        super::has_any_remote(cwd)
    }

    fn has_unpushed_commits(&self, cwd: &Path) -> bool {
        super::has_unpushed_commits(cwd)
    }

    fn list_submodules(&self, cwd: &Path) -> Vec<PathBuf> {
        super::list_submodules(cwd)
    }

    fn ensure_on_branch(&self, cwd: &Path) -> Result<()> {
        super::ensure_on_branch(cwd)
    }
}
