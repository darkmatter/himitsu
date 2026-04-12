use clap::{Args, Subcommand};

use crate::config;
use crate::error::Result;
use crate::reference::SecretRef;

/// Manage the active store context used for disambiguation.
///
/// When multiple stores are configured, himitsu needs to know which one to
/// use for bare secret paths (`env/KEY`) that don't include an explicit
/// store reference.  The context is that default.
///
/// # Examples
///
/// ```text
/// himitsu alias                          # show current alias
/// himitsu alias <name> <reference>            # set alias
/// himitsu alias clear                    # remove context (back to auto)
/// ```
#[derive(Debug, Args)]
pub struct AliasArgs {
    #[command(subcommand)]
    pub command: Option<AliasCommand>,
}

#[derive(Debug, Subcommand)]
pub enum AliasCommand {
    /// Set the active store context.
    ///
    /// Accepts either a bare slug (`org/repo`) or a provider-qualified
    /// reference (`github:org/repo`).  The slug portion is extracted and
    /// stored; the provider prefix is validated but not persisted (store
    /// checkouts are keyed by slug only).
    Remote {
        /// Store reference: `org/repo` or `provider:org/repo`.
        reference: String,
    },
    /// Clear the active context, falling back to automatic disambiguation.
    Clear,
}

pub fn run(args: AliasArgs, _ctx: &super::Context) -> Result<()> {
    let cfg_path = config::config_path();
    let mut cfg = config::Config::load(&cfg_path)?;

    match args.command {
        // ── show ─────────────────────────────────────────────────────────
        None => match &cfg.alias {
            Some(alias) => {
                println!("Alias: {alias}");
                println!();
                println!("Run `himitsu alias clear` to remove it.");
            }
            None => {
                println!("No alias set — store is resolved automatically.");
                println!();
                println!("Run `himitsu alias <name> <reference>` to set one.");
            }
        },

        // ── alias remote <ref> ─────────────────────────────────────────
        Some(AliasCommand::Remote { reference }) => {
            // Parse and validate the reference, then extract the store slug.
            let secret_ref = SecretRef::parse_store_ref(&reference)?;
            let slug = secret_ref.store_slug.as_deref().unwrap_or(reference.trim());

            cfg.context = Some(slug.to_string());
            cfg.save(&cfg_path)?;
            println!("Context set to: {slug}");
        }

        // ── alias clear ────────────────────────────────────────────────
        Some(AliasCommand::Clear) => {
            if cfg.context.is_none() {
                println!("No context was set.");
            } else {
                cfg.context = None;
                cfg.save(&cfg_path)?;
                println!("Context cleared.");
            }
        }
    }

    Ok(())
}
