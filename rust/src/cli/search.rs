use std::path::PathBuf;

use clap::Args;

use super::Context;
use crate::error::Result;
use crate::index::SecretIndex;

/// Search secrets across all known projects.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Search query to match against secret paths.
    pub query: String,

    /// Refresh the search index before querying.
    #[arg(long)]
    pub refresh: bool,
}

pub fn run(args: SearchArgs, ctx: &Context) -> Result<()> {
    let index_path = ctx.index_path();
    let idx = SecretIndex::open(&index_path)?;

    if args.refresh {
        refresh_index(&idx, ctx)?;
    }

    let results = idx.search(&args.query, None)?;

    for result in &results {
        println!("{}\t{}", result.remote_id, result.secret_path);
    }

    Ok(())
}

/// Refresh the search index by rescanning all registered remotes plus stores_dir.
fn refresh_index(idx: &SecretIndex, ctx: &Context) -> Result<()> {
    // Re-index all remotes already registered in the SQLite index
    let remote_ids = idx.list_remotes()?;
    for remote_id in &remote_ids {
        let store_path = PathBuf::from(remote_id);
        if !store_path.exists() {
            continue;
        }
        idx.clear_remote(remote_id)?;
        let paths = crate::remote::store::list_secrets(&store_path, None)?;
        for path in &paths {
            idx.upsert(remote_id, path)?;
        }
    }

    // Also scan stores_dir for any new stores not yet in the index
    let stores_dir = ctx.stores_dir();
    if stores_dir.exists() {
        for org_entry in std::fs::read_dir(&stores_dir)? {
            let org_entry = org_entry?;
            if !org_entry.file_type()?.is_dir() {
                continue;
            }
            let org = org_entry.file_name().to_string_lossy().to_string();
            for repo_entry in std::fs::read_dir(org_entry.path())? {
                let repo_entry = repo_entry?;
                if !repo_entry.file_type()?.is_dir() {
                    continue;
                }
                let repo = repo_entry.file_name().to_string_lossy().to_string();
                let slug = format!("{org}/{repo}");
                let store_path = repo_entry.path();

                if !remote_ids.contains(&slug) {
                    idx.register_remote(&slug, None)?;
                    let paths = crate::remote::store::list_secrets(&store_path, None)?;
                    for path in &paths {
                        idx.upsert(&slug, path)?;
                    }
                }
            }
        }
    }

    Ok(())
}
