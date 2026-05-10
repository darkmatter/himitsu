use std::path::Path;

use clap::Args;

use super::{rekey, Context};
use crate::config;
use crate::error::Result;

/// Sync stores: pull from git remote and optionally re-encrypt drifted secrets.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Store slug to sync (e.g. org/repo). If omitted, syncs all stores.
    pub store: Option<String>,
    /// Skip the rekey step after pulling.
    #[arg(long)]
    pub no_rekey: bool,
}

pub fn run(args: SyncArgs, ctx: &Context) -> Result<()> {
    // Identify stores to sync
    let slugs: Vec<String> = if let Some(ref slug) = args.store {
        config::validate_remote_slug(slug)?;
        vec![slug.clone()]
    } else {
        let all = crate::remote::list_remotes()?;
        if all.is_empty() {
            println!("no stores found; use `himitsu remote add <org/repo>` to add one");
            return Ok(());
        }
        all
    };

    for slug in &slugs {
        let (org, repo) = config::validate_remote_slug(slug)?;
        let store_path = config::store_checkout(org, repo);

        if !store_path.exists() {
            eprintln!("warning: store '{slug}' not found locally, skipping");
            continue;
        }

        let store_ctx = Context {
            data_dir: ctx.data_dir.clone(),
            state_dir: ctx.state_dir.clone(),
            store: store_path.clone(),
            recipients_path: None,
            key_provider: ctx.key_provider.clone(),
        };

        // Commit any pre-existing pending changes (e.g. from a prior sync that
        // rekeyed but didn't push) so `git pull --rebase` doesn't refuse on a
        // dirty tree.
        let pre_committed = store_ctx.commit(&format!("himitsu sync: pending changes in {slug}"));

        // Pull latest from remote (best-effort). When the upstream branch
        // doesn't exist yet (fresh empty remote), skip silently — there's
        // nothing to pull and the rekey/push below will seed the first commit.
        let pulled = match pull_with_remote_branch(&store_path) {
            PullOutcome::Pulled => {
                tracing::debug!("pulled '{slug}'");
                true
            }
            PullOutcome::SkippedEmptyRemote => {
                tracing::debug!("'{slug}' upstream has no default branch yet, skipping pull");
                false
            }
            PullOutcome::Failed(e) => {
                eprintln!("warning: pull failed for '{slug}': {e}");
                false
            }
        };

        // Optionally rekey secrets for the current recipient set
        let rekey_count = if !args.no_rekey {
            match rekey::rekey_store(&store_ctx, None) {
                Ok(n) => n,
                Err(e) => {
                    eprintln!("warning: rekey failed for '{slug}': {e}");
                    0
                }
            }
        } else {
            0
        };

        // Commit any rekeyed files and push everything (including pre-existing
        // pending changes from above) so the working tree ends clean.
        let rekey_committed = store_ctx.commit(&format!("himitsu sync: rekey in {slug}"));
        if pre_committed || rekey_committed {
            store_ctx.push();
        }

        let pull_label = if pulled { "pulled" } else { "skipped pull" };
        if args.no_rekey {
            println!("{slug}: {pull_label}");
        } else {
            println!("{slug}: {pull_label}, {rekey_count} secret(s) rekeyed");
        }
    }

    Ok(())
}

enum PullOutcome {
    Pulled,
    SkippedEmptyRemote,
    Failed(String),
}

/// Pull `origin/<current-branch>` into the working tree, fast-forward only.
///
/// Tolerates two edge cases the plain `git pull` shipped in `git::pull` does
/// not: a local branch with no configured upstream (use the matching `origin/`
/// ref directly), and a freshly-created empty remote with no default branch
/// (skip the pull entirely so the next push can seed the upstream).
fn pull_with_remote_branch(cwd: &Path) -> PullOutcome {
    if let Err(e) = crate::git::run(&["fetch", "--quiet", "origin"], cwd) {
        return PullOutcome::Failed(e.to_string());
    }

    let branch = match crate::git::run(&["symbolic-ref", "--short", "HEAD"], cwd) {
        Ok(out) => out.trim().to_string(),
        Err(e) => return PullOutcome::Failed(e.to_string()),
    };
    if branch.is_empty() {
        return PullOutcome::Failed("detached HEAD".to_string());
    }

    if crate::git::run(&["rev-parse", "--verify", &format!("origin/{branch}")], cwd).is_err() {
        return PullOutcome::SkippedEmptyRemote;
    }

    match crate::git::run(
        &[
            "pull",
            "--ff-only",
            "--recurse-submodules",
            "origin",
            &branch,
        ],
        cwd,
    ) {
        Ok(_) => PullOutcome::Pulled,
        Err(e) => PullOutcome::Failed(e.to_string()),
    }
}
