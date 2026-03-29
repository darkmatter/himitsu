use clap::Args;

use super::Context;
use crate::error::Result;
use crate::remote::store;

/// List secrets in the store (optionally filtered by path prefix).
#[derive(Debug, Args)]
pub struct LsArgs {
    /// Optional path prefix to filter secrets (e.g. "prod" shows all under prod/).
    pub path: Option<String>,
}

pub fn run(args: LsArgs, ctx: &Context) -> Result<()> {
    if ctx.store.as_os_str().is_empty() {
        // No store resolved — list all known stores
        let stores_dir = ctx.stores_dir();
        if !stores_dir.exists() {
            eprintln!("No stores configured. Use `himitsu remote add <org/repo>` to add one.");
            return Ok(());
        }
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

    let prefix = args.path.as_deref();
    let paths = store::list_secrets(&ctx.store, prefix)?;

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
