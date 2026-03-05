use clap::Args;

use super::Context;
use crate::config;
use crate::crypto::age;
use crate::error::Result;
use crate::remote::store;

/// Sync encrypted secrets to configured project destinations.
///
/// In Phase 2, sync re-encrypts all secrets for the updated recipient set.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Target environment. If omitted, syncs all environments.
    pub env: Option<String>,
}

pub fn run(args: SyncArgs, ctx: &Context) -> Result<()> {
    let mode = config::detect_mode(&std::env::current_dir()?);
    let remote_ref = config::resolve_remote(&ctx.remote_override, &mode, &ctx.himitsu_home)?;
    let remote_path = config::remote_path(&ctx.himitsu_home, &remote_ref);
    crate::remote::ensure_remote_exists(&remote_path)?;

    // Read identity for decryption
    let key_path = ctx.himitsu_home.join("keys/age.txt");
    let identity = age::read_identity(&key_path)?;

    // Collect current recipients
    let recipients = age::collect_all_recipients(&remote_path)?;
    if recipients.is_empty() {
        return Err(crate::error::HimitsuError::Recipient(
            "no recipients found".into(),
        ));
    }

    // Determine which envs to re-encrypt
    let envs = match args.env {
        Some(env) => vec![env],
        None => store::list_envs(&remote_path)?,
    };

    let mut count = 0;
    for env in &envs {
        let keys = store::list_secrets(&remote_path, env)?;
        for key in &keys {
            let ciphertext = store::read_secret(&remote_path, env, key)?;
            let plaintext = age::decrypt(&ciphertext, &identity)?;
            let new_ciphertext = age::encrypt(&plaintext, &recipients)?;
            store::write_secret(&remote_path, env, key, &new_ciphertext)?;
            count += 1;
        }
    }

    println!(
        "Synced {count} secret(s) for {} recipient(s) in {remote_ref}",
        recipients.len()
    );
    Ok(())
}
