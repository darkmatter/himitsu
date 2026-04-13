use clap::Args;

use super::Context;
use crate::crypto::age;
use crate::error::{HimitsuError, Result};

use crate::reference::SecretRef;
use crate::remote::store;

/// Set a secret value.
#[derive(Debug, Args)]
pub struct SetArgs {
    /// Secret path. Accepts a bare path (`prod/API_KEY`) or a provider-prefixed
    /// qualified reference (`github:org/repo/prod/API_KEY`) that overrides the
    /// default store.
    pub path: String,
    /// Secret value.
    pub value: String,
    /// Skip git commit and push.
    #[arg(long)]
    pub no_push: bool,
}

pub fn run(args: SetArgs, ctx: &Context) -> Result<()> {
    let secret_path = set_plaintext(ctx, &args.path, args.value.as_bytes(), args.no_push)?;
    println!("Set {secret_path}");
    Ok(())
}

/// Encrypt `plaintext` and persist it at `path`, returning the resolved secret path.
pub fn set_plaintext(
    ctx: &Context,
    path: &str,
    plaintext: &[u8],
    no_push: bool,
) -> Result<String> {
    let secret_ref = SecretRef::parse(path)?;
    let (effective_store, secret_path, recipients_path_override) = if secret_ref.is_qualified() {
        let resolved = secret_ref.resolve_store()?;
        let path = secret_ref.path.ok_or_else(|| {
            HimitsuError::InvalidReference(
                "qualified reference must include a secret path after org/repo".into(),
            )
        })?;
        (resolved, path, None)
    } else {
        let path = secret_ref.path.expect("bare SecretRef always has a path");
        (ctx.store.clone(), path, ctx.recipients_path.as_deref())
    };

    let recipients = age::collect_recipients(&effective_store, recipients_path_override)?;
    if recipients.is_empty() {
        return Err(HimitsuError::Recipient(
            "no recipients found; run `himitsu init` or add recipients first".into(),
        ));
    }

    let ciphertext = age::encrypt(plaintext, &recipients)?;
    store::write_secret(&effective_store, &secret_path, &ciphertext)?;

    if !no_push {
        ctx.commit_and_push(&format!("himitsu: set {secret_path}"));
    }

    Ok(secret_path)
}
