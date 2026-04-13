use std::io::{self, Write};

use clap::Args;

use super::Context;
use crate::error::Result;

/// Print a secret's plaintext bytes to stdout with no decoration.
#[derive(Debug, Args)]
pub struct ReadArgs {
    /// Secret path. Accepts a bare path (`prod/API_KEY`) or a provider-prefixed
    /// qualified reference (`github:org/repo/prod/API_KEY`).
    pub path: String,
}

pub fn run(args: ReadArgs, ctx: &Context) -> Result<()> {
    let plaintext = super::get::get_plaintext(ctx, &args.path)?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&plaintext)?;
    stdout.flush()?;
    Ok(())
}
