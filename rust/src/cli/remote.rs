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
    /// Push local changes to the store's git remote.
    Push,

    /// Pull latest changes from the store's git remote.
    Pull,

    /// Show the git status of the store.
    Status,

    /// Clone a remote repository into stores_dir/<org>/<repo> and register it.
    Add {
        /// Remote slug in the form <org>/<repo>.
        slug: String,

        /// Git URL to clone from (default: git@github.com:<org>/<repo>.git).
        #[arg(long)]
        url: Option<String>,
    },
}

/// Resolve the git working directory from the store context.
/// In the new model, the store IS the git root.
fn resolve_git_dir(ctx: &Context) -> Result<std::path::PathBuf> {
    if ctx.store.as_os_str().is_empty() {
        return Err(HimitsuError::Git(
            "no store configured; use --store or --remote".into(),
        ));
    }
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

            let dest = config::stores_dir().join(org).join(repo);

            if dest.exists() {
                return Err(HimitsuError::Remote(format!(
                    "remote '{slug}' already exists at {}",
                    dest.display()
                )));
            }

            let clone_url = url.unwrap_or_else(|| format!("git@github.com:{org}/{repo}.git"));

            println!("Cloning {clone_url} → {}", dest.display());
            git::clone(&clone_url, &dest)?;
            println!("Added remote '{slug}'");
        }
    }

    Ok(())
}
