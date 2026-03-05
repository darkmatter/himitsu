use clap::{Args, Subcommand};

use super::Context;
use crate::error::{HimitsuError, Result};

/// Share secrets with external recipients.
#[derive(Debug, Args)]
pub struct ShareArgs {
    #[command(subcommand)]
    pub command: ShareCommand,
}

#[derive(Debug, Subcommand)]
pub enum ShareCommand {
    /// Send secrets to a recipient.
    Send {
        /// Recipient reference (e.g. github:org/repo, nostr:npub1..., email:user@domain).
        #[arg(long)]
        to: String,

        /// Secret path to share.
        #[arg(long)]
        path: String,

        /// Secret value to share.
        #[arg(long)]
        value: Option<String>,
    },
}

pub fn run(_args: ShareArgs, _ctx: &Context) -> Result<()> {
    Err(HimitsuError::NotSupported(
        "share is not yet implemented (planned for Phase 4)".into(),
    ))
}
