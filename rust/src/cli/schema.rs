use clap::{Args, Subcommand};

use super::Context;
use crate::error::{HimitsuError, Result};

/// Generate and manage JSON schemas for himitsu config files.
#[derive(Debug, Args)]
pub struct SchemaArgs {
    #[command(subcommand)]
    pub command: SchemaCommand,
}

#[derive(Debug, Subcommand)]
pub enum SchemaCommand {
    /// Refresh dynamic schemas from current state.
    Refresh,
}

pub fn run(_args: SchemaArgs, _ctx: &Context) -> Result<()> {
    Err(HimitsuError::NotSupported(
        "schema is not yet implemented (planned for Phase 7)".into(),
    ))
}
