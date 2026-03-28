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
    /// Skip git commit and push.
    #[arg(long)]
    pub no_push: bool,
}

pub fn run(args: SetArgs, ctx: &Context) -> Result<()> {
    let recipients = age::collect_all_recipients(&ctx.store)?;
    if recipients.is_empty() {
        return Err(crate::error::HimitsuError::Recipient(
            "no recipients found; run `himitsu init` or add recipients first".into(),
        ));
    }

    let ciphertext = age::encrypt(args.value.as_bytes(), &recipients)?;
    store::write_secret(&ctx.store, &args.env, &args.key, &ciphertext)?;

    // Update search index
    if let Ok(idx) = SecretIndex::open(&config::index_path(&ctx.user_home)) {
        let store_id = ctx.store.to_string_lossy().to_string();
        let _ = idx.register_remote(&store_id, None);
        let path = format!("vars/{}/{}.age", args.env, args.key);
        let _ = idx.upsert(&store_id, &args.env, &path, &args.key);
    }

    // Commit + push to git remote
    if !args.no_push {
        ctx.commit_and_push(&format!("himitsu: set {}/{}", args.env, args.key));
    }

    println!("Set {}/{}", args.env, args.key);
    Ok(())
}
