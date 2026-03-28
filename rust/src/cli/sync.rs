use clap::Args;

use super::Context;
use crate::config;
use crate::crypto::age;
use crate::error::Result;
use crate::remote::store;

/// Sync: re-encrypt all secrets for the updated recipient set.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Target environment. If omitted, syncs all environments.
    pub env: Option<String>,
}

pub fn run(args: SyncArgs, ctx: &Context) -> Result<()> {
    let identity = age::read_identity(&config::key_path(&ctx.user_home))?;
    let recipients = age::collect_all_recipients(&ctx.store)?;
    if recipients.is_empty() {
        return Err(crate::error::HimitsuError::Recipient(
            "no recipients found".into(),
        ));
    }

    let envs = match args.env {
        Some(env) => vec![env],
        None => store::list_envs(&ctx.store)?,
    };

    let mut count = 0;
    for env in &envs {
        let keys = store::list_secrets(&ctx.store, env)?;
        for key in &keys {
            let ciphertext = store::read_secret(&ctx.store, env, key)?;
            let plaintext = age::decrypt(&ciphertext, &identity)?;
            let new_ciphertext = age::encrypt(&plaintext, &recipients)?;
            store::write_secret(&ctx.store, env, key, &new_ciphertext)?;
            count += 1;
        }
    }

    ctx.commit_and_push(&format!("himitsu: sync {count} secret(s)"));

    println!(
        "Synced {count} secret(s) for {} recipient(s)",
        recipients.len()
    );
    Ok(())
}
