use clap::Args;

use super::Context;
use crate::error::{HimitsuError, Result};

/// Import secrets from external stores (SOPS, 1Password).
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Path to a SOPS-encrypted file to import.
    #[arg(long)]
    pub sops: Option<String>,

    /// 1Password reference to import (e.g. op://vault/item or op://vault/item/field).
    #[arg(long)]
    pub op: Option<String>,

    /// Target environment.
    #[arg(long)]
    pub env: String,

    /// Secret key name (required for single-field 1Password import).
    #[arg(long)]
    pub key: Option<String>,

    /// Overwrite existing secrets.
    #[arg(long)]
    pub overwrite: bool,
}

pub fn run(_args: ImportArgs, _ctx: &Context) -> Result<()> {
    Err(HimitsuError::NotSupported(
        "import is not yet implemented (planned for Phase 8)".into(),
    ))
}
