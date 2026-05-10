use clap::Args;

use super::Context;
use crate::crypto::age;
use crate::error::{HimitsuError, Result};
use crate::remote::store;

/// Re-encrypt secrets for the current recipient set.
#[derive(Debug, Args)]
pub struct RekeyArgs {
    /// Path prefix to filter which secrets to re-encrypt. If omitted, re-encrypts all.
    pub path: Option<String>,
    /// Always re-encrypt, even if nothing appears to have changed.
    #[arg(long)]
    pub force: bool,
}

/// Re-encrypt secrets in `ctx.store` for the current recipient set.
///
/// Returns the count of re-encrypted secrets.
/// `path_prefix`, if given, limits re-encryption to secrets under that prefix.
///
/// Note: `args.force` is accepted for forward-compat (future no-op detection)
/// but currently all matched secrets are always re-encrypted.
pub fn rekey_store(ctx: &Context, path_prefix: Option<&str>) -> Result<usize> {
    let identity = ctx.load_identity()?;
    let recipients = age::collect_recipients(&ctx.store, ctx.recipients_path.as_deref())?;
    if recipients.is_empty() {
        return Err(HimitsuError::Recipient("no recipients found".into()));
    }

    let all_paths = store::list_secrets(&ctx.store, None)?;
    let paths_to_process: Vec<String> = match path_prefix {
        Some(prefix) => {
            let pfx_slash = format!("{prefix}/");
            all_paths
                .into_iter()
                .filter(|p| p == prefix || p.starts_with(&pfx_slash))
                .collect()
        }
        None => all_paths,
    };

    let mut count = 0;
    for path in &paths_to_process {
        let ciphertext = store::read_secret(&ctx.store, path)?;
        let plaintext = age::decrypt(&ciphertext, &identity)?;
        let new_ciphertext = age::encrypt(&plaintext, &recipients)?;
        store::write_secret(&ctx.store, path, &new_ciphertext)?;
        count += 1;
    }
    Ok(count)
}

pub fn run(args: RekeyArgs, ctx: &Context) -> Result<()> {
    let count = rekey_store(ctx, args.path.as_deref())?;
    let recipients = age::collect_recipients(&ctx.store, ctx.recipients_path.as_deref())?;
    println!(
        "Re-encrypted {count} secret(s) for {} recipient(s)",
        recipients.len()
    );
    Ok(())
}
