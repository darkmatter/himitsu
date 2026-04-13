use std::io::{self, Read};

use clap::Args;

use super::Context;
use crate::error::Result;

/// Write a secret's plaintext from an argument or stdin without any decoration.
#[derive(Debug, Args)]
pub struct WriteArgs {
    /// Secret path. Accepts a bare path (`prod/API_KEY`) or a provider-prefixed
    /// qualified reference (`github:org/repo/prod/API_KEY`).
    pub path: String,
    /// Plaintext value. When omitted, stdin is read instead.
    pub value: Option<String>,
    /// Force reading the value from stdin even if a positional value is given.
    #[arg(long)]
    pub stdin: bool,
    /// Skip git commit and push.
    #[arg(long)]
    pub no_push: bool,
}

pub fn run(args: WriteArgs, ctx: &Context) -> Result<()> {
    let plaintext: Vec<u8> = match (args.stdin, args.value) {
        (false, Some(value)) => value.into_bytes(),
        _ => {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf)?;
            buf
        }
    };

    super::set::set_plaintext(ctx, &args.path, &plaintext, args.no_push)?;
    Ok(())
}
