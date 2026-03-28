use clap::Args;

use super::Context;
use crate::config;
use crate::error::Result;
use crate::index::SecretIndex;

/// Search secrets across all known projects.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Search query to match against key names.
    pub query: String,

    /// Refresh the search index before querying.
    #[arg(long)]
    pub refresh: bool,
}

pub fn run(args: SearchArgs, ctx: &Context) -> Result<()> {
    let index_path = config::index_path(&ctx.user_home);
    let idx = SecretIndex::open(&index_path)?;

    if args.refresh {
        refresh_index(&idx, &ctx.user_home)?;
    }

    let results = idx.search(&args.query, None)?;

    if results.is_empty() {
        return Ok(());
    }

    for result in &results {
        println!("{}\t{}\t{}", result.remote_id, result.env, result.key_name);
    }

    Ok(())
}

/// Refresh the search index by scanning all known stores.
fn refresh_index(idx: &SecretIndex, user_home: &std::path::Path) -> Result<()> {
    let stores = config::load_known_stores(user_home);
    for store_str in &stores {
        let store_path = std::path::PathBuf::from(store_str);
        if !store_path.exists() {
            continue;
        }

        idx.register_remote(store_str, None)?;
        idx.clear_remote(store_str)?;

        let envs = crate::remote::store::list_envs(&store_path)?;
        for env in &envs {
            let keys = crate::remote::store::list_secrets(&store_path, env)?;
            for key in &keys {
                let path = format!("vars/{env}/{key}.age");
                idx.upsert(store_str, env, &path, key)?;
            }
        }
    }
    Ok(())
}
