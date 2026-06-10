//! StoreOps — the mutation seam between presentation and the Store.
//!
//! One module owns the side-effect chain every Store mutation must run:
//! the append-only commit (`himitsu: {msg}` on success, `himitsu: FAILED:
//! {msg}: {err}` on failure, so `git status` is never left dirty), the
//! push (on success, unless opted out), and the completions-cache refresh
//! (on success, so the next tab-press sees the new path list).
//!
//! Two adapters sit in front of it:
//! - the **CLI dispatcher** calls [`finalize`] once per command (so batch
//!   commands like `import` stay one commit), with the message from
//!   `mutation_message`;
//! - **TUI views** call the silent mutation cores below ([`set_secret`],
//!   [`delete_secret`], [`rekey`], [`join`], [`recipient_add`],
//!   [`recipient_rm`]), which run the same chain per action. Cores never
//!   touch stdin or stdout — errors come back as values, and ratatui owns
//!   the screen. Generalizes the `add_core`/`rm_core` precedent (hm-by7).
//!
//! Global-config mutations (e.g. `remote add`) are not Store mutations —
//! they have no commit/cache chain and stay outside this module.

use super::Context;
use crate::error::Result;
use crate::proto::SecretValue;

/// Run the post-mutation side-effect chain for `result`.
///
/// Commits on success **and** failure (the append-only invariant: a failed
/// mutation that left partial writes must still leave a clean tree, with
/// the `FAILED:` prefix recording what happened). Pushes and refreshes the
/// completions cache only on success.
pub fn finalize<T>(ctx: &Context, msg: &str, no_push: bool, result: &Result<T>) {
    let final_msg = match result {
        Ok(_) => format!("himitsu: {msg}"),
        Err(e) => format!("himitsu: FAILED: {msg}: {e}"),
    };
    let committed = ctx.commit(&final_msg);
    if result.is_ok() && committed && !no_push {
        ctx.push();
    }
    // Keep the completions cache in sync after every successful mutation
    // so the next tab-press sees the updated path list immediately.
    if result.is_ok() && !ctx.store.as_os_str().is_empty() {
        let _ = crate::completions_cache::refresh_store(&ctx.state_dir, &ctx.store);
    }
}

/// Run `mutation` under the chain: the mutation body executes, then
/// [`finalize`] runs regardless of outcome.
pub fn run_mutation<T>(
    ctx: &Context,
    msg: &str,
    no_push: bool,
    mutation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let result = mutation();
    finalize(ctx, msg, no_push, &result);
    result
}

/// Create or overwrite a Secret: validate env-key metadata, encrypt to the
/// effective store's recipients, write, then run the chain. Returns the
/// normalized secret path.
///
/// A qualified ref (`github:org/repo/...`) writes to that store, not the
/// ambient one — the chain (commit, push, completions refresh) is scoped
/// to the same effective store so the write is never stranded dirty.
pub fn set_secret(ctx: &Context, path: &str, sv: &SecretValue) -> Result<String> {
    let mut chain_ctx = ctx.clone();
    if let Ok(secret_ref) = crate::reference::SecretRef::parse(path) {
        if secret_ref.is_qualified() {
            if let Ok(store) = secret_ref.resolve_store() {
                chain_ctx.store = store;
            }
            // Resolution errors fall through with the ambient store: the
            // mutation body re-parses and surfaces the real error before
            // anything is written.
        }
    }
    run_mutation(&chain_ctx, &format!("set {path}"), false, || {
        if !sv.env_key.is_empty() {
            super::set::validate_env_key(&sv.env_key)?;
        }
        super::set::encrypt_and_write(ctx, path, sv)
    })
}

/// Delete a Secret from the active store, then run the chain.
pub fn delete_secret(ctx: &Context, path: &str) -> Result<()> {
    run_mutation(ctx, &format!("delete {path}"), false, || {
        crate::remote::store::delete_secret(&ctx.store, path)
    })
}

/// Re-encrypt secrets for the store's current recipient list (optionally
/// narrowed to a path prefix), then run the chain. Returns the number of
/// secrets rekeyed.
pub fn rekey(ctx: &Context, path_prefix: Option<&str>) -> Result<usize> {
    let msg = match path_prefix {
        Some(p) => format!("rekey {p}"),
        None => "rekey".to_string(),
    };
    run_mutation(ctx, &msg, false, || {
        super::rekey::rekey_store(ctx, path_prefix)
    })
}

/// Join the active store as a recipient (idempotent), then run the chain.
pub fn join(ctx: &Context) -> Result<super::join::JoinOutcome> {
    run_mutation(ctx, "join", false, || super::join::join_core(ctx, None))
}

/// Add a recipient by explicit age public key, then run the chain. An
/// empty/whitespace description is normalized to `None` so the core never
/// writes an empty sidecar.
pub fn recipient_add(
    ctx: &Context,
    name: &str,
    age_key: &str,
    description: Option<String>,
) -> Result<()> {
    run_mutation(ctx, &format!("recipient add {name}"), false, || {
        let description = description.and_then(|d| {
            let t = d.trim().to_string();
            (!t.is_empty()).then_some(t)
        });
        super::recipient::add_core(ctx, name, Some(age_key), description).map(|_missing| ())
    })
}

/// Remove a recipient by name, then run the chain.
pub fn recipient_rm(ctx: &Context, name: &str) -> Result<()> {
    run_mutation(ctx, &format!("recipient rm {name}"), false, || {
        super::recipient::rm_core(ctx, name)
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::crypto::age;
    use crate::error::HimitsuError;
    use crate::remote::store as rstore;

    /// A Context over a tempdir store that is a real git repo, with one age
    /// identity on disk and the store's recipients configured.
    fn test_ctx() -> (tempfile::TempDir, Context) {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let state_dir = tmp.path().join("state");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(rstore::recipients_dir(&store)).unwrap();
        std::fs::create_dir_all(rstore::secrets_dir(&store)).unwrap();

        let (secret, public) = age::keygen();
        std::fs::write(
            data_dir.join("key"),
            format!("# public key: {public}\n{secret}\n"),
        )
        .unwrap();
        std::fs::write(
            rstore::recipients_dir(&store).join("me.pub"),
            format!("{public}\n"),
        )
        .unwrap();

        crate::git::run(&["init", "-q"], &store).unwrap();
        crate::git::run(&["config", "user.email", "t@t"], &store).unwrap();
        crate::git::run(&["config", "user.name", "t"], &store).unwrap();

        let ctx = Context {
            data_dir,
            state_dir,
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
            project_root: None,
            git: Arc::new(crate::git::CliGitAdapter),
            project_config_cell: Default::default(),
        };
        (tmp, ctx)
    }

    fn last_commit_subject(store: &Path) -> String {
        crate::git::run(&["log", "--format=%s", "-1"], store)
            .unwrap()
            .trim()
            .to_string()
    }

    #[test]
    fn set_secret_runs_the_full_chain() {
        let (_tmp, ctx) = test_ctx();

        let sv = SecretValue {
            data: b"v1".to_vec(),
            ..Default::default()
        };
        let path = set_secret(&ctx, "prod/api-key", &sv).unwrap();
        assert_eq!(path, "prod/api-key");

        // Mutation landed and is readable back through the resolver.
        let decoded = crate::cli::resolver::SecretResolver::resolve(&ctx, "prod/api-key").unwrap();
        assert_eq!(decoded.data, b"v1");

        // Append-only commit with the canonical message; tree left clean.
        assert_eq!(last_commit_subject(&ctx.store), "himitsu: set prod/api-key");
        let status = crate::git::run(&["status", "--porcelain"], &ctx.store).unwrap();
        assert!(status.trim().is_empty(), "tree dirty: {status}");

        // Completions cache refreshed — the new path is immediately visible.
        let hits = crate::completions_cache::lookup(
            &ctx.state_dir,
            std::slice::from_ref(&ctx.store),
            "prod",
        )
        .unwrap();
        assert!(hits.iter().any(|h| h == "prod/api-key"), "{hits:?}");
    }

    #[test]
    fn set_secret_validates_env_key_metadata() {
        let (_tmp, ctx) = test_ctx();
        let sv = SecretValue {
            data: b"v".to_vec(),
            env_key: "1NOT VALID".to_string(),
            ..Default::default()
        };
        assert!(set_secret(&ctx, "prod/x", &sv).is_err());
    }

    #[test]
    fn delete_secret_commits_and_refreshes() {
        let (_tmp, ctx) = test_ctx();
        let sv = SecretValue {
            data: b"v".to_vec(),
            ..Default::default()
        };
        set_secret(&ctx, "prod/gone", &sv).unwrap();

        delete_secret(&ctx, "prod/gone").unwrap();

        assert_eq!(last_commit_subject(&ctx.store), "himitsu: delete prod/gone");
        let status = crate::git::run(&["status", "--porcelain"], &ctx.store).unwrap();
        assert!(status.trim().is_empty(), "tree dirty: {status}");
        let hits = crate::completions_cache::lookup(
            &ctx.state_dir,
            std::slice::from_ref(&ctx.store),
            "prod",
        )
        .unwrap();
        assert!(!hits.iter().any(|h| h == "prod/gone"), "{hits:?}");
    }

    #[test]
    fn failed_mutation_commits_the_failed_marker() {
        let (_tmp, ctx) = test_ctx();

        // A mutation that writes something, then fails: the chain must
        // still commit, with the FAILED prefix recording the partial state.
        let result: Result<()> = run_mutation(&ctx, "test-op", true, || {
            std::fs::write(rstore::secrets_dir(&ctx.store).join("partial.age"), b"junk").unwrap();
            Err(HimitsuError::NotSupported("boom".into()))
        });
        assert!(result.is_err());

        let subject = last_commit_subject(&ctx.store);
        assert!(
            subject.starts_with("himitsu: FAILED: test-op"),
            "got: {subject}"
        );
        let status = crate::git::run(&["status", "--porcelain"], &ctx.store).unwrap();
        assert!(status.trim().is_empty(), "tree dirty: {status}");
    }

    #[test]
    fn join_core_outcomes_are_silent_values() {
        let (_tmp, ctx) = test_ctx();
        // The fixture's own key is already a recipient ("me.pub" holds it).
        match join(&ctx).unwrap() {
            super::super::join::JoinOutcome::AlreadyRecipient => {}
            other => panic!("expected AlreadyRecipient, got {other:?}"),
        }
    }
}
