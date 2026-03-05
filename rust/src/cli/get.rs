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
    let mode = config::detect_mode(&std::env::current_dir()?);
    let remote_ref = config::resolve_remote(&ctx.remote_override, &mode, &ctx.himitsu_home)?;
    let remote_path = config::remote_path(&ctx.himitsu_home, &remote_ref);
    crate::remote::ensure_remote_exists(&remote_path)?;

    // Read encrypted secret
    let ciphertext = store::read_secret(&remote_path, &args.env, &args.key)?;

    // Read identity (private key)
    let key_path = ctx.himitsu_home.join("keys/age.txt");
    let identity = age::read_identity(&key_path)?;

    // Decrypt
    let plaintext = age::decrypt(&ciphertext, &identity)?;
    let value = String::from_utf8_lossy(&plaintext);
    print!("{value}");

    Ok(())
}
