use clap::Args;

use super::Context;
use crate::error::Result;
use crate::index::SecretIndex;

/// Search secrets across all remotes.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Search query to match against key names.
    pub query: String,

    /// Refresh the search index before querying.
    #[arg(long)]
    pub refresh: bool,
}

pub fn run(args: SearchArgs, ctx: &Context) -> Result<()> {
    let index_path = ctx.himitsu_home.join("state/index.db");
    let idx = SecretIndex::open(&index_path)?;

    // Optionally refresh the index
    if args.refresh {
        refresh_index(&idx, ctx)?;
    }

    let results = idx.search(&args.query, ctx.remote_override.as_deref())?;

    if results.is_empty() {
        // Empty output, exit 0 per spec
        return Ok(());
    }

    for result in &results {
        println!("{}\t{}\t{}", result.remote_id, result.env, result.key_name);
    }

    Ok(())
}

/// Refresh the search index by scanning all known remotes.
fn refresh_index(idx: &SecretIndex, ctx: &Context) -> Result<()> {
    let remotes = crate::remote::list_remotes(&ctx.himitsu_home)?;
    for remote_ref in &remotes {
        let remote_path = crate::config::remote_path(&ctx.himitsu_home, remote_ref);
        idx.register_remote(remote_ref, None)?;
        idx.clear_remote(remote_ref)?;

        let envs = crate::remote::store::list_envs(&remote_path)?;
        for env in &envs {
            let keys = crate::remote::store::list_secrets(&remote_path, env)?;
            for key in &keys {
                let path = format!("vars/{env}/{key}.age");
                idx.upsert(remote_ref, env, &path, key)?;
            }
        }
    }
    Ok(())
}
