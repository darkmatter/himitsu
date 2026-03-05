use clap::{Args, Subcommand};

use super::Context;
use crate::error::{HimitsuError, Result};
use crate::git;

/// Manage git-backed remotes.
#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommand,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Add a remote repository (clone existing or create new via --github).
    Add {
        /// Repository reference (e.g. org/repo). Optional when using --github.
        repo: Option<String>,

        /// Create a new GitHub repository.
        #[arg(long)]
        github: bool,

        /// GitHub org or user for repo creation.
        #[arg(long)]
        org: Option<String>,

        /// Repository name for repo creation.
        #[arg(long)]
        name: Option<String>,
    },

    /// Push local changes to the remote.
    Push,

    /// Pull latest changes from the remote.
    Pull,

    /// Show the status of the remote.
    Status,
}

pub fn run(args: RemoteArgs, ctx: &Context) -> Result<()> {
    match args.command {
        RemoteCommand::Add {
            repo,
            github,
            org,
            name,
        } => {
            if github {
                // Create GitHub repo then clone
                let org = org.ok_or_else(|| {
                    HimitsuError::Remote("--org is required with --github".into())
                })?;
                let name = name.ok_or_else(|| {
                    HimitsuError::Remote("--name is required with --github".into())
                })?;
                let remote_ref = format!("{org}/{name}");
                let dest = crate::config::remote_path(&ctx.himitsu_home, &remote_ref);

                if dest.exists() {
                    return Err(HimitsuError::Remote(format!(
                        "remote '{remote_ref}' already exists at {}",
                        dest.display()
                    )));
                }

                // Create the repo via gh CLI
                let output = std::process::Command::new("gh")
                    .args(["repo", "create", &remote_ref, "--private", "--clone"])
                    .current_dir(dest.parent().unwrap_or(&ctx.himitsu_home))
                    .output()
                    .map_err(|e| HimitsuError::Remote(format!("failed to run gh: {e}")))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    // If create failed, try cloning instead
                    let url = format!("git@github.com:{remote_ref}.git");
                    std::fs::create_dir_all(dest.parent().unwrap())?;
                    git::clone(&url, &dest).map_err(|_| {
                        HimitsuError::Remote(format!(
                            "failed to create/clone repo: {}",
                            stderr.trim()
                        ))
                    })?;
                }

                println!("Added remote '{remote_ref}'");
            } else {
                // Clone existing repo
                let repo = repo.ok_or_else(|| {
                    HimitsuError::Remote("repository reference required (e.g. org/repo)".into())
                })?;
                let dest = crate::config::remote_path(&ctx.himitsu_home, &repo);

                if dest.exists() {
                    return Err(HimitsuError::Remote(format!(
                        "remote '{repo}' already exists at {}",
                        dest.display()
                    )));
                }

                let url = format!("git@github.com:{repo}.git");
                git::clone(&url, &dest)?;
                println!("Added remote '{repo}'");
            }

            Ok(())
        }

        RemoteCommand::Push => {
            let mode = crate::config::detect_mode(&std::env::current_dir()?);
            let remote_ref =
                crate::config::resolve_remote(&ctx.remote_override, &mode, &ctx.himitsu_home)?;
            let remote_path = crate::config::remote_path(&ctx.himitsu_home, &remote_ref);
            crate::remote::ensure_remote_exists(&remote_path)?;

            git::commit(&remote_path, "himitsu: update secrets")?;
            git::push(&remote_path)?;
            println!("Pushed {remote_ref}");
            Ok(())
        }

        RemoteCommand::Pull => {
            let mode = crate::config::detect_mode(&std::env::current_dir()?);
            let remote_ref =
                crate::config::resolve_remote(&ctx.remote_override, &mode, &ctx.himitsu_home)?;
            let remote_path = crate::config::remote_path(&ctx.himitsu_home, &remote_ref);
            crate::remote::ensure_remote_exists(&remote_path)?;

            git::pull(&remote_path)?;
            println!("Pulled {remote_ref}");
            Ok(())
        }

        RemoteCommand::Status => {
            let mode = crate::config::detect_mode(&std::env::current_dir()?);
            let remote_ref =
                crate::config::resolve_remote(&ctx.remote_override, &mode, &ctx.himitsu_home)?;
            let remote_path = crate::config::remote_path(&ctx.himitsu_home, &remote_ref);
            crate::remote::ensure_remote_exists(&remote_path)?;

            let status = git::status(&remote_path)?;
            if status.is_empty() {
                println!("{remote_ref}: clean");
            } else {
                println!("{remote_ref}:");
                print!("{status}");
            }
            Ok(())
        }
    }
}
