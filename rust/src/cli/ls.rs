use clap::Args;

use super::Context;
use crate::config;
use crate::error::Result;
use crate::remote::store;

/// List environments or secrets within an environment.
#[derive(Debug, Args)]
pub struct LsArgs {
    /// Environment to list secrets for. If omitted, lists all environments.
    pub env: Option<String>,
}

pub fn run(args: LsArgs, ctx: &Context) -> Result<()> {
    let mode = config::detect_mode(&std::env::current_dir()?);
    let remote_ref = config::resolve_remote(&ctx.remote_override, &mode, &ctx.himitsu_home)?;
    let remote_path = config::remote_path(&ctx.himitsu_home, &remote_ref);
    crate::remote::ensure_remote_exists(&remote_path)?;

    match args.env {
        Some(env) => {
            let keys = store::list_secrets(&remote_path, &env)?;
            if keys.is_empty() {
                eprintln!("No secrets in environment '{env}'");
            } else {
                for key in &keys {
                    println!("{key}");
                }
            }
        }
        None => {
            let envs = store::list_envs(&remote_path)?;
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
