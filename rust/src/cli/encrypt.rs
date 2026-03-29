use clap::Args;

use super::Context;
use crate::crypto::age;
use crate::error::Result;
use crate::remote::store;

/// Re-encrypt all secrets for current recipients.
#[derive(Debug, Args)]
pub struct EncryptArgs {
    /// Path prefix to filter which secrets to re-encrypt. If omitted, re-encrypts all.
    pub env: Option<String>,
}

pub fn run(args: EncryptArgs, ctx: &Context) -> Result<()> {
    let identity = age::read_identity(&ctx.key_path())?;
    let recipients = age::collect_all_recipients(&ctx.store)?;
    if recipients.is_empty() {
        return Err(crate::error::HimitsuError::Recipient(
            "no recipients found".into(),
        ));
    }

    let all_paths = store::list_secrets(&ctx.store, None)?;
    let paths_to_process: Vec<String> = match &args.env {
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

    ctx.commit_and_push(&format!("himitsu: re-encrypt {count} secret(s)"));

    println!(
        "Re-encrypted {count} secret(s) for {} recipient(s)",
        recipients.len()
    );
    Ok(())
}
