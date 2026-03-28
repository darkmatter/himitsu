use clap::{Args, Subcommand};

use super::Context;
use crate::error::{HimitsuError, Result};
use crate::git;

/// Manage remote sync targets.
#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommand,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Push local changes to the project's git remote.
    Push,

    /// Pull latest changes from the project's git remote.
    Pull,

    /// Show the git status of the store.
    Status,
}

pub fn run(args: RemoteArgs, ctx: &Context) -> Result<()> {
    // The store is $GIT_ROOT/.himitsu/ -- git operations on the parent repo
    let git_dir = if ctx.store.join(".git").exists() {
        ctx.store.clone()
    } else {
        ctx.store
            .parent()
            .ok_or_else(|| HimitsuError::Git("cannot determine git root from store".into()))?
            .to_path_buf()
    };

    match args.command {
        RemoteCommand::Push => {
            git::commit(&git_dir, "himitsu: update secrets")?;
            git::push(&git_dir)?;
            println!("Pushed");
        }

        RemoteCommand::Pull => {
            git::pull(&git_dir)?;
            println!("Pulled");
        }

        RemoteCommand::Status => {
            let status = git::status(&git_dir)?;
            if status.is_empty() {
                println!("clean");
            } else {
                print!("{status}");
            }
        }
    }

    Ok(())
}
