use clap::Args;

use super::Context;
use crate::crypto::age;
use crate::error::Result;
use crate::remote::store;

/// Get a secret value.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// Secret path (e.g. "prod/API_KEY").
    pub path: String,
}

pub fn run(args: GetArgs, ctx: &Context) -> Result<()> {
    let ciphertext = store::read_secret(&ctx.store, &args.path)?;
    let identity = age::read_identity(&ctx.key_path())?;
    let plaintext = age::decrypt(&ciphertext, &identity)?;
    print!("{}", String::from_utf8_lossy(&plaintext));
    Ok(())
}
