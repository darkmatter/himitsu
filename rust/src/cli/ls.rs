use std::path::Path;

use clap::Args;

use super::Context;
use crate::error::Result;
use crate::reference::SecretRef;
use crate::remote::store;

/// List secrets in the store (optionally filtered by path prefix).
#[derive(Debug, Args)]
pub struct LsArgs {
    /// Optional path prefix or qualified store reference.
    ///
    /// - Bare prefix: `prod` — lists secrets under `prod/` in the current store.
    /// - Qualified store: `github:org/repo` — lists all secrets in that store.
    /// - Qualified prefix: `github:org/repo/prod` — lists secrets under `prod/`
    ///   in that store.
    pub path: Option<String>,
}

pub fn run(args: LsArgs, ctx: &Context) -> Result<()> {
    // If the path argument is a qualified ref (provider:org/repo[/prefix]),
    // resolve to the named store and use any trailing path as a prefix filter.
    if let Some(ref path_str) = args.path {
        let secret_ref = SecretRef::parse(path_str)?;
        if secret_ref.is_qualified() {
            let resolved_store = secret_ref.resolve_store()?;
            return list_secrets_in(&resolved_store, secret_ref.path.as_deref());
        }
    }

    if ctx.store.as_os_str().is_empty() {
        // No store resolved — list all known stores
        let remotes = crate::remote::list_remotes()?;
        if remotes.is_empty() {
            eprintln!("No stores configured. Use `himitsu remote add <org/repo>` to add one.");
        } else {
            for r in &remotes {
                println!("{r}");
            }
        }
        return Ok(());
    }

    list_secrets_in(&ctx.store, args.path.as_deref())
}

fn list_secrets_in(store_path: &Path, prefix: Option<&str>) -> Result<()> {
    let paths = store::list_secrets(store_path, prefix)?;
    if paths.is_empty() {
        let msg = match prefix {
            Some(p) => format!("No secrets under '{p}'"),
            None => "No secrets found".to_string(),
        };
        eprintln!("{msg}");
    } else {
        for p in &paths {
            println!("{p}");
        }
    }
    Ok(())
}
