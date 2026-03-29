use clap::Args;

use super::Context;
use crate::error::{HimitsuError, Result};

/// Decrypt secrets (not supported - secrets are never stored in plaintext).
#[derive(Debug, Args)]
pub struct DecryptArgs {
    /// Target environment.
    pub env: Option<String>,
}

pub fn run(_args: DecryptArgs, _ctx: &Context) -> Result<()> {
    Err(HimitsuError::NotSupported(
        "bulk decrypt is not supported; secrets are never stored in plaintext. Use `himitsu get <path>` to read individual values."
            .into(),
    ))
}
