use clap::Args;

use super::{rekey, Context};
use crate::config;
use crate::error::Result;

/// Sync stores: pull from git remote and optionally re-encrypt drifted secrets.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Store slug to sync (e.g. org/repo). If omitted, syncs all stores.
    pub store: Option<String>,
    /// Skip the rekey step after pulling.
    #[arg(long)]
    pub no_rekey: bool,
}

pub fn run(args: SyncArgs, ctx: &Context) -> Result<()> {
    // Identify stores to sync
    let slugs: Vec<String> = if let Some(ref slug) = args.store {
        config::validate_remote_slug(slug)?;
        vec![slug.clone()]
    } else {
        let all = crate::remote::list_remotes()?;
        if all.is_empty() {
            println!("no stores found; use `himitsu remote add <org/repo>` to add one");
            return Ok(());
        }
        all
    };

    for slug in &slugs {
        let (org, repo) = config::validate_remote_slug(slug)?;
        let store_path = config::store_checkout(org, repo);

        if !store_path.exists() {
            eprintln!("warning: store '{slug}' not found locally, skipping");
            continue;
        }

        // Pull latest from remote (best-effort)
        match crate::git::pull(&store_path) {
            Ok(_) => tracing::debug!("pulled '{slug}'"),
            Err(e) => eprintln!("warning: pull failed for '{slug}': {e}"),
        }

        // Optionally rekey secrets for the current recipient set
        let rekey_count = if !args.no_rekey {
            let store_ctx = Context {
                data_dir: ctx.data_dir.clone(),
                state_dir: ctx.state_dir.clone(),
                store: store_path.clone(),
            };
            match rekey::rekey_store(&store_ctx, None) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("warning: rekey failed for '{slug}': {e}");
                    0
                }
            }
        } else {
            0
        };

        if args.no_rekey {
            println!("{slug}: pulled");
        } else {
            println!("{slug}: pulled, {rekey_count} secret(s) rekeyed");
        }
    }

    Ok(())
}
