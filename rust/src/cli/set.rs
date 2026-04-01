use clap::Args;

use super::Context;
use crate::crypto::age;
use crate::error::{HimitsuError, Result};
use crate::index::SecretIndex;
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
    let secret_ref = SecretRef::parse(&args.path)?;
    // Capture slug before partial moves below.
    let qualified_slug = secret_ref.store_slug.clone();

    let (effective_store, secret_path, recipients_path_override) = if secret_ref.is_qualified() {
        let resolved = secret_ref.resolve_store()?;
        let path = secret_ref.path.ok_or_else(|| {
            HimitsuError::InvalidReference(
                "qualified reference must include a secret path after org/repo".into(),
            )
        })?;
        // For cross-store writes, use the target store's default recipients layout.
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

    let ciphertext = age::encrypt(args.value.as_bytes(), &recipients)?;
    store::write_secret(&effective_store, &secret_path, &ciphertext)?;

    // Update search index
    if let Ok(idx) = SecretIndex::open(&ctx.index_path()) {
        // For qualified refs, the store_id is the slug; for bare paths use ctx.
        let store_id = if qualified_slug.is_some() {
            qualified_slug
        } else {
            ctx.store_id()
        };
        if let Some(sid) = store_id {
            let _ = idx.register_remote(&sid, None);
            let _ = idx.upsert(&sid, &secret_path);
        }
    }

    if !args.no_push {
        ctx.commit_and_push(&format!("himitsu: set {secret_path}"));
    }

    println!("Set {secret_path}");
    Ok(())
}
