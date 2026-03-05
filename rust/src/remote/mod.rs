pub mod store;

use std::path::{Path, PathBuf};

use crate::error::{HimitsuError, Result};

/// List all known remotes (org/repo) by scanning `~/.himitsu/data/`.
pub fn list_remotes(himitsu_home: &Path) -> Result<Vec<String>> {
    let data_dir = himitsu_home.join("data");
    let mut remotes = vec![];
    if !data_dir.exists() {
        return Ok(remotes);
    }
    for org_entry in std::fs::read_dir(&data_dir)? {
        let org_entry = org_entry?;
        if !org_entry.file_type()?.is_dir() {
            continue;
        }
        let org_name = org_entry.file_name().to_string_lossy().to_string();
        for repo_entry in std::fs::read_dir(org_entry.path())? {
            let repo_entry = repo_entry?;
            if !repo_entry.file_type()?.is_dir() {
                continue;
            }
            let repo_name = repo_entry.file_name().to_string_lossy().to_string();
            remotes.push(format!("{org_name}/{repo_name}"));
        }
    }
    remotes.sort();
    Ok(remotes)
}

/// Validate that a remote exists locally.
pub fn ensure_remote_exists(remote_path: &Path) -> Result<()> {
    if !remote_path.exists() {
        return Err(HimitsuError::RemoteNotFound(
            remote_path.to_string_lossy().to_string(),
        ));
    }
    Ok(())
}

/// Get the path to the remote's vars directory.
pub fn vars_dir(remote_path: &Path) -> PathBuf {
    remote_path.join("vars")
}

/// Get the path to the remote's recipients directory.
pub fn recipients_dir(remote_path: &Path) -> PathBuf {
    remote_path.join("recipients")
}
