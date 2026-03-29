pub mod store;

use std::path::Path;

use crate::error::{HimitsuError, Result};

/// List all known remotes (org/repo) by scanning `stores_dir()`.
pub fn list_remotes() -> Result<Vec<String>> {
    let stores = crate::config::stores_dir();
    let mut remotes = vec![];
    if !stores.exists() {
        return Ok(remotes);
    }
    for org_entry in std::fs::read_dir(&stores)? {
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
