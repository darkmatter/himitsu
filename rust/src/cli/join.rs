use clap::Args;

use super::Context;
use crate::crypto::age;
use crate::error::{HimitsuError, Result};
use crate::remote::store as rstore;

/// Join a store by adding your own age public key to its recipient list.
///
/// Idempotent: if your key is already in the recipient list, this is a no-op.
/// After adding, commits and pushes so other members can rekey for you.
#[derive(Debug, Args)]
pub struct JoinArgs {
    /// Recipient name (default: username, with a key fingerprint appended if taken).
    #[arg(long)]
    pub name: Option<String>,

    /// Skip the automatic git push after adding.
    #[arg(long)]
    pub no_push: bool,
}

pub fn run(args: JoinArgs, ctx: &Context) -> Result<()> {
    // Presentation wrapper: the silent core does the work (the dispatcher
    // runs the commit/push/cache chain post-dispatch); this layer owns the
    // human-facing output.
    match join_core(ctx, args.name)? {
        JoinOutcome::AlreadyRecipient => {
            println!("Already a recipient of this store — nothing to do.");
        }
        JoinOutcome::Joined(name) => {
            println!("Joined as recipient '{name}'");
        }
    }
    Ok(())
}

/// Outcome of [`join_core`], for presentation layers to render.
#[derive(Debug)]
pub enum JoinOutcome {
    /// The caller's key was added under this recipient name.
    Joined(String),
    /// The caller was already a recipient — nothing changed.
    AlreadyRecipient,
}

/// Silent core of `join`: add the caller's own age public key to the
/// store's recipient list. Idempotent. No stdin, no stdout — safe to call
/// from the TUI (use `store_ops::join`, which adds the mutation chain).
pub(super) fn join_core(ctx: &Context, name: Option<String>) -> Result<JoinOutcome> {
    let self_pubkey = read_own_pubkey(ctx)?;
    let recipients = age::collect_recipients(&ctx.store, ctx.recipients_path.as_deref())?;
    if recipients.iter().any(|r| r.to_string() == self_pubkey) {
        return Ok(JoinOutcome::AlreadyRecipient);
    }

    let recipients_dir =
        rstore::recipients_dir_with_override(&ctx.store, ctx.recipients_path.as_deref());
    std::fs::create_dir_all(&recipients_dir)?;
    let name = name.unwrap_or_else(|| {
        let base = default_recipient_name();
        if recipients_dir.join(format!("{base}.pub")).exists() {
            format!("{base}-{}", crate::keyring::fingerprint(&self_pubkey))
        } else {
            base
        }
    });

    let pub_file = recipients_dir.join(format!("{name}.pub"));
    if let Some(parent) = pub_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if pub_file.exists() {
        let existing = std::fs::read_to_string(&pub_file)?.trim().to_string();
        if existing == self_pubkey {
            return Ok(JoinOutcome::AlreadyRecipient);
        }
        return Err(HimitsuError::Recipient(format!(
            "recipient name '{name}' is taken by a different key — use --name to pick another"
        )));
    }

    use std::io::Write;
    writeln!(std::fs::File::create_new(&pub_file)?, "{self_pubkey}")?;
    Ok(JoinOutcome::Joined(name))
}

fn read_own_pubkey(ctx: &Context) -> Result<String> {
    let key_path = crate::crypto::keystore::pubkey_path(&ctx.data_dir);
    let contents = std::fs::read_to_string(&key_path).map_err(|e| {
        HimitsuError::Recipient(format!(
            "could not read public key at {}: {e}",
            key_path.display()
        ))
    })?;
    Ok(age::parse_recipient(contents.trim())?.to_string())
}

fn default_recipient_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "self".to_string())
}

/// Check whether the user's own pubkey is in the store's recipient list.
/// Returns `true` if the user IS a recipient (or if the check can't run).
pub fn is_self_recipient(ctx: &Context) -> bool {
    let Ok(self_pubkey) = read_own_pubkey(ctx) else {
        return true;
    };
    let Ok(recipients) = age::collect_recipients(&ctx.store, ctx.recipients_path.as_deref()) else {
        return true;
    };
    if recipients.is_empty() {
        return true;
    }
    recipients.iter().any(|r| r.to_string() == self_pubkey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::store as rstore;
    use tempfile::TempDir;

    fn mk_ctx_with_key(tmp: &TempDir) -> Context {
        let data_dir = tmp.path().join("data");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(rstore::recipients_dir(&store)).unwrap();
        std::fs::create_dir_all(rstore::secrets_dir(&store)).unwrap();

        let (_, public) = age::keygen();
        std::fs::write(data_dir.join("key.pub"), format!("{public}\n")).unwrap();

        Context {
            data_dir,
            state_dir: tmp.path().join("state"),
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
            project_root: None,
            git: std::sync::Arc::new(crate::git::CliGitAdapter),
            project_config_cell: Default::default(),
        }
    }

    #[test]
    fn join_adds_self_as_recipient() {
        let tmp = TempDir::new().unwrap();
        let ctx = mk_ctx_with_key(&tmp);

        // Store has another recipient but not us — we should NOT be a recipient.
        let rdir = rstore::recipients_dir(&ctx.store);
        std::fs::write(
            rdir.join("other.pub"),
            "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p\n",
        )
        .unwrap();
        assert!(!is_self_recipient(&ctx));

        run(
            JoinArgs {
                name: None,
                no_push: true,
            },
            &ctx,
        )
        .unwrap();

        assert!(is_self_recipient(&ctx));
    }

    #[test]
    fn join_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let ctx = mk_ctx_with_key(&tmp);

        run(
            JoinArgs {
                name: Some("me".into()),
                no_push: true,
            },
            &ctx,
        )
        .unwrap();
        // Second call should succeed silently
        run(
            JoinArgs {
                name: Some("me".into()),
                no_push: true,
            },
            &ctx,
        )
        .unwrap();
    }

    #[test]
    fn join_detects_name_collision() {
        let tmp = TempDir::new().unwrap();
        let ctx = mk_ctx_with_key(&tmp);

        // Write a different key under the default name
        let rdir = rstore::recipients_dir(&ctx.store);
        let name = default_recipient_name();
        std::fs::write(
            rdir.join(format!("{name}.pub")),
            "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p\n",
        )
        .unwrap();

        assert!(join_core(&ctx, Some(name.clone())).is_err());
        let public = read_own_pubkey(&ctx).unwrap();
        let expected = format!("{name}-{}", crate::keyring::fingerprint(&public));
        assert!(matches!(join_core(&ctx, None).unwrap(), JoinOutcome::Joined(n) if n == expected));
        assert_eq!(
            std::fs::read_to_string(rdir.join(format!("{expected}.pub")))
                .unwrap()
                .trim(),
            public
        );
        assert!(matches!(
            join_core(&ctx, None).unwrap(),
            JoinOutcome::AlreadyRecipient
        ));
    }

    #[test]
    fn is_self_recipient_true_when_present() {
        let tmp = TempDir::new().unwrap();
        let ctx = mk_ctx_with_key(&tmp);
        let pubkey = read_own_pubkey(&ctx).unwrap();

        let rdir = rstore::recipients_dir(&ctx.store);
        std::fs::write(rdir.join("me.pub"), format!("{pubkey}\n")).unwrap();

        assert!(is_self_recipient(&ctx));
    }

    #[test]
    fn is_self_recipient_false_when_absent() {
        let tmp = TempDir::new().unwrap();
        let ctx = mk_ctx_with_key(&tmp);

        // Store has a recipient, but not us
        let rdir = rstore::recipients_dir(&ctx.store);
        std::fs::write(
            rdir.join("other.pub"),
            "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p\n",
        )
        .unwrap();

        assert!(!is_self_recipient(&ctx));
    }
}
