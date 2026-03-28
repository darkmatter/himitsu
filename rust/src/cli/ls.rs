use clap::Args;

use super::Context;
use crate::error::Result;
use crate::remote::store;

/// List environments or secrets within an environment.
#[derive(Debug, Args)]
pub struct LsArgs {
    /// Environment to list secrets for. If omitted, lists all environments.
    pub env: Option<String>,
}

pub fn run(args: LsArgs, ctx: &Context) -> Result<()> {
    match args.env {
        Some(env) => {
            let keys = store::list_secrets(&ctx.store, &env)?;
            if keys.is_empty() {
                eprintln!("No secrets in environment '{env}'");
            } else {
                for key in &keys {
                    println!("{key}");
                }
            }
        }
        None => {
            let envs = store::list_envs(&ctx.store)?;
            if envs.is_empty() {
                eprintln!("No environments found");
            } else {
                for env in &envs {
                    println!("{env}");
                }
            }
        }
    }
    Ok(())
}
