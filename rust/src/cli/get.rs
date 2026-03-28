use clap::Args;

use super::Context;
use crate::config;
use crate::crypto::age;
use crate::error::Result;
use crate::remote::store;

/// Get a secret value.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// Target environment (e.g. prod, dev).
    pub env: String,
    /// Secret key name.
    pub key: String,
}

pub fn run(args: GetArgs, ctx: &Context) -> Result<()> {
    let ciphertext = store::read_secret(&ctx.store, &args.env, &args.key)?;
    let identity = age::read_identity(&config::key_path(&ctx.user_home))?;
    let plaintext = age::decrypt(&ciphertext, &identity)?;
    print!("{}", String::from_utf8_lossy(&plaintext));
    Ok(())
}
