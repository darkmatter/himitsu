use clap::{Args, Subcommand};

use crate::config;
use crate::error::{HimitsuError, Result};
use crate::git;

/// Manage remote stores (add, remove, list, set default).
#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommand,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Clone a remote repository into stores_dir/<org>/<repo> and register it.
    Add {
        /// Remote slug in the form <org>/<repo>.
        slug: String,

        /// Git URL to clone from (default: git@github.com:<org>/<repo>.git).
        #[arg(long)]
        url: Option<String>,
    },

    /// Get or set the default store.
    Default {
        /// Store slug to set as default (e.g. org/repo). If omitted, prints current default.
        slug: Option<String>,
    },

    /// List all known stores.
    List,

    /// Remove a store checkout.
    Remove {
        /// Store slug to remove (e.g. org/repo).
        slug: String,
    },
}

pub fn run(args: RemoteArgs, _ctx: &super::Context) -> Result<()> {
    match args.command {
        RemoteCommand::Add { slug, url } => {
            // If the user passed a full git URL as the slug, extract the
            // org/repo slug and use the original input as the clone URL.
            let (resolved_slug, clone_url) =
                if let Some(parsed) = super::init::parse_remote_slug(&slug) {
                    (parsed, url.unwrap_or_else(|| slug.clone()))
                } else {
                    let s = slug.clone();
                    let (org, repo) = config::validate_remote_slug(&s)?;
                    let u =
                        url.unwrap_or_else(|| format!("git@github.com:{org}/{repo}.git"));
                    (s, u)
                };

            let (org, repo) = config::validate_remote_slug(&resolved_slug)?;
            let dest = config::stores_dir().join(org).join(repo);

            if dest.exists() {
                if !dest.join(".git").exists() {
                    return Err(HimitsuError::Remote(format!(
                        "remote '{resolved_slug}' already exists at {} but is not a git checkout",
                        dest.display()
                    )));
                }
                if !git::has_any_remote(&dest) {
                    git::add_remote(&dest, "origin", &clone_url)?;
                }
                println!("Updating {resolved_slug} from origin");
                git::pull_or_checkout_origin(&dest)?;
                println!("Updated remote '{resolved_slug}'");
            } else {
                println!("Cloning {clone_url} → {}", dest.display());
                git::clone(&clone_url, &dest)?;
                println!("Added remote '{resolved_slug}'");
            }
        }

        RemoteCommand::Default { slug } => match slug {
            None => {
                let cfg = config::Config::load(&config::config_path())?;
                match cfg.default_store {
                    Some(s) => println!("{s}"),
                    None => println!("none set"),
                }
            }
            Some(new_slug) => {
                config::validate_remote_slug(&new_slug)?;
                let cfg_path = config::config_path();
                let mut cfg = config::Config::load(&cfg_path)?;
                cfg.default_store = Some(new_slug.clone());
                cfg.save(&cfg_path)?;
                println!("Default store set to '{new_slug}'");
            }
        },

        RemoteCommand::List => {
            let remotes = crate::remote::list_remotes()?;
            if remotes.is_empty() {
                println!("no remotes found");
            } else {
                for r in &remotes {
                    println!("{r}");
                }
            }
        }

        RemoteCommand::Remove { slug } => {
            let (org, repo) = config::validate_remote_slug(&slug)?;
            let stores = config::stores_dir();
            let path = stores.join(org).join(repo);

            // Safety: resolved path must be under stores_dir
            if !path.starts_with(&stores) {
                return Err(HimitsuError::Remote(format!(
                    "resolved path {} is not under stores directory",
                    path.display()
                )));
            }

            if !path.exists() {
                return Err(HimitsuError::RemoteNotFound(slug.clone()));
            }

            std::fs::remove_dir_all(&path)?;
            println!("Removed remote '{slug}'");

            // Clear default_store if it was the removed slug
            let cfg_path = config::config_path();
            let mut cfg = config::Config::load(&cfg_path)?;
            if cfg.default_store.as_deref() == Some(slug.as_str()) {
                cfg.default_store = None;
                cfg.save(&cfg_path)?;
                println!("Cleared default store (was '{slug}')");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    /// Verify that a full SSH URL is resolved to the correct slug and clone URL.
    #[test]
    fn slug_from_ssh_url() {
        let slug = "git@github.com:myorg/secrets.git";
        let parsed = super::super::init::parse_remote_slug(slug);
        assert_eq!(parsed, Some("myorg/secrets".to_string()));
    }

    /// Verify that a full HTTPS URL is resolved correctly.
    #[test]
    fn slug_from_https_url() {
        let slug = "https://github.com/myorg/secrets.git";
        let parsed = super::super::init::parse_remote_slug(slug);
        assert_eq!(parsed, Some("myorg/secrets".to_string()));
    }

    /// A plain slug should NOT be parsed as a URL.
    #[test]
    fn plain_slug_not_parsed_as_url() {
        let slug = "myorg/secrets";
        let parsed = super::super::init::parse_remote_slug(slug);
        assert_eq!(parsed, None);
    }
}
