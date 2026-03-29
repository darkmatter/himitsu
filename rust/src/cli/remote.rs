use std::path::PathBuf;

use clap::{Args, Subcommand};

use super::Context;
use crate::config;
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

    /// Clone a remote repository into ~/.himitsu/data/<org>/<repo> and register it.
    Add {
        /// Remote slug in the form <org>/<repo>.
        slug: String,

        /// Git URL to clone from (default: git@github.com:<org>/<repo>.git).
        #[arg(long)]
        url: Option<String>,
    },
}

/// Resolve the git working directory from the store context.
///
/// The project store is `$GIT_ROOT/.himitsu/`, so git operations run
/// against the parent directory (the actual git repo root).
fn resolve_git_dir(ctx: &Context) -> Result<PathBuf> {
    if ctx.store.join(".git").exists() {
        Ok(ctx.store.clone())
    } else {
        ctx.store
            .parent()
            .ok_or_else(|| HimitsuError::Git("cannot determine git root from store".into()))
            .map(|p| p.to_path_buf())
    }
}

pub fn run(args: RemoteArgs, ctx: &Context) -> Result<()> {
    match args.command {
        RemoteCommand::Push => {
            let git_dir = resolve_git_dir(ctx)?;
            git::commit(&git_dir, "himitsu: update secrets")?;
            git::push(&git_dir)?;
            println!("Pushed");
        }

        RemoteCommand::Pull => {
            let git_dir = resolve_git_dir(ctx)?;
            git::pull(&git_dir)?;
            println!("Pulled");
        }

        RemoteCommand::Status => {
            let git_dir = resolve_git_dir(ctx)?;
            let status = git::status(&git_dir)?;
            if status.is_empty() {
                println!("clean");
            } else {
                print!("{status}");
            }
        }

        RemoteCommand::Add { slug, url } => {
            let (org, repo) = config::validate_remote_slug(&slug)?;

            let dest = ctx.user_home.join("data").join(org).join(repo);

            if dest.exists() {
                return Err(HimitsuError::Remote(format!(
                    "remote '{slug}' already exists at {}",
                    dest.display()
                )));
            }

            let clone_url = url.unwrap_or_else(|| format!("git@github.com:{org}/{repo}.git"));

            println!("Cloning {clone_url} → {}", dest.display());
            git::clone(&clone_url, &dest)?;
            config::register_store(&ctx.user_home, &dest)?;
            println!("Added remote '{slug}'");
        }
    }

    Ok(())
}
