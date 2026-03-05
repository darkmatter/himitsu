use clap::Args;

use super::Context;
use crate::config;
use crate::crypto::age;
use crate::error::Result;
use crate::index::SecretIndex;
use crate::remote::store;

/// Set a secret value.
#[derive(Debug, Args)]
pub struct SetArgs {
    /// Target environment (e.g. prod, dev).
    pub env: String,

    /// Secret key name.
    pub key: String,

    /// Secret value.
    pub value: String,
}

pub fn run(args: SetArgs, ctx: &Context) -> Result<()> {
    let mode = config::detect_mode(&std::env::current_dir()?);
    let remote_ref = config::resolve_remote(&ctx.remote_override, &mode, &ctx.himitsu_home)?;
    let remote_path = config::remote_path(&ctx.himitsu_home, &remote_ref);
    crate::remote::ensure_remote_exists(&remote_path)?;

    // Collect all recipients for encryption
    let recipients = age::collect_all_recipients(&remote_path)?;
    if recipients.is_empty() {
        return Err(crate::error::HimitsuError::Recipient(
            "no recipients found; add recipients first with `himitsu recipient add`".into(),
        ));
    }

    // Encrypt the value
    let ciphertext = age::encrypt(args.value.as_bytes(), &recipients)?;

    // Write to store
    store::write_secret(&remote_path, &args.env, &args.key, &ciphertext)?;

    // Update search index
    let index_path = ctx.himitsu_home.join("state/index.db");
    if let Ok(idx) = SecretIndex::open(&index_path) {
        let _ = idx.register_remote(&remote_ref, None);
        let path = format!("vars/{}/{}.age", args.env, args.key);
        let _ = idx.upsert(&remote_ref, &args.env, &path, &args.key);
    }

    println!("Set {}/{} in {}", args.env, args.key, remote_ref);
    Ok(())
}
