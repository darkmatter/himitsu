use clap::Args;

use super::Context;
use crate::crypto::age;
use crate::error::{HimitsuError, Result};
use crate::reference::SecretRef;
use crate::remote::store;

/// Get a secret value.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// Secret path. Accepts a bare path (`prod/API_KEY`) or a provider-prefixed
    /// qualified reference (`github:org/repo/prod/API_KEY`) that overrides the
    /// default store.
    pub path: String,
}

pub fn run(args: GetArgs, ctx: &Context) -> Result<()> {
    let secret_ref = SecretRef::parse(&args.path)?;

    let (effective_store, secret_path) = if secret_ref.is_qualified() {
        let resolved = secret_ref.resolve_store()?;
        let path = secret_ref.path.ok_or_else(|| {
            HimitsuError::InvalidReference(
                "qualified reference must include a secret path after org/repo".into(),
            )
        })?;
        (resolved, path)
    } else {
        let path = secret_ref.path.expect("bare SecretRef always has a path");
        (ctx.store.clone(), path)
    };

    let ciphertext = store::read_secret(&effective_store, &secret_path)?;
    let identity = age::read_identity(&ctx.key_path())?;
    let plaintext = age::decrypt(&ciphertext, &identity)?;
    print!("{}", String::from_utf8_lossy(&plaintext));
    Ok(())
}
