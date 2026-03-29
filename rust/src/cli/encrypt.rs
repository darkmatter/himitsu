use clap::Args;

use super::Context;
use crate::error::Result;

/// (Deprecated) Re-encrypt secrets. Use `rekey` instead.
#[derive(Debug, Args)]
pub struct EncryptArgs {
    /// Path prefix (deprecated: use `himitsu rekey [path]` instead).
    pub env: Option<String>,
}

pub fn run(args: EncryptArgs, ctx: &Context) -> Result<()> {
    eprintln!("warning: `encrypt` is deprecated, use `rekey` instead");
    super::rekey::run(
        super::rekey::RekeyArgs {
            path: args.env,
            force: false,
        },
        ctx,
    )
}
