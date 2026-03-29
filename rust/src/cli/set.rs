use clap::Args;

use super::Context;
use crate::crypto::age;
use crate::error::Result;
use crate::index::SecretIndex;
use crate::remote::store;

/// Set a secret value.
#[derive(Debug, Args)]
pub struct SetArgs {
    /// Secret path (e.g. "prod/API_KEY" or "db/password").
    pub path: String,
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
    store::write_secret(&ctx.store, &args.path, &ciphertext)?;

    // Update search index
    if let Ok(idx) = SecretIndex::open(&ctx.index_path()) {
        if let Some(store_id) = ctx.store_id() {
            let _ = idx.register_remote(&store_id, None);
            let _ = idx.upsert(&store_id, &args.path);
        }
    }

    if !args.no_push {
        ctx.commit_and_push(&format!("himitsu: set {}", args.path));
    }

    println!("Set {}", args.path);
    Ok(())
}
