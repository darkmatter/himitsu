use std::collections::BTreeSet;
use std::path::Path;

use clap::Args;
use tracing::debug;

use super::Context;
use crate::config;
use crate::error::{HimitsuError, Result};

/// Verify store checkouts are up to date with their remotes.
///
/// Exits 0 when all checked stores are up to date; exits 1 when any store is
/// behind its remote tracking branch.  Information about ahead commits and
/// uncommitted changes is printed but does not affect the exit code.
///
/// Examples:
///   himitsu check
///   himitsu check myorg/secrets --offline
#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Store slug to check (e.g. org/repo). If omitted, checks all referenced stores.
    pub store: Option<String>,

    /// Skip `git fetch` (check against already-fetched remote refs).
    #[arg(long)]
    pub offline: bool,
}

/// Status of a single store checkout relative to its remote.
struct StoreStatus {
    behind: u32,
    ahead: u32,
    dirty: bool,
    branch: String,
    /// Non-fatal warnings (e.g. no remote tracking branch configured).
    warnings: Vec<String>,
}

pub fn run(args: CheckArgs, ctx: &Context) -> Result<()> {
    let slugs = discover_stores(&args, ctx)?;

    if slugs.is_empty() {
        println!("no stores found; use `himitsu remote add <org/repo>` to add one");
        return Ok(());
    }

    let mut any_behind = false;

    for slug in &slugs {
        let (org, repo) = config::validate_remote_slug(slug)?;
        let store_path = config::store_checkout(org, repo);

        if !store_path.exists() {
            eprintln!(
                "  warning: store '{slug}' not found locally — \
                 run `himitsu remote add {slug}`"
            );
            continue;
        }

        match check_store(slug, &store_path, args.offline) {
            Ok(status) => {
                // Print any non-fatal warnings
                for w in &status.warnings {
                    println!("  ⚠ {slug}: {w}");
                }

                if status.behind > 0 {
                    any_behind = true;
                    println!(
                        "⚠ {slug}: {} commit(s) behind origin/{} — \
                         run `himitsu git pull` then `himitsu generate`",
                        status.behind, status.branch
                    );
                } else {
                    println!("✓ {slug}: up to date");
                }

                if status.ahead > 0 {
                    println!(
                        "  info: {slug} is {} commit(s) ahead of origin/{}",
                        status.ahead, status.branch
                    );
                }
                if status.dirty {
                    println!("  info: {slug} has uncommitted changes");
                }
            }
            Err(e) => {
                eprintln!("  warning: could not check '{slug}': {e}");
            }
        }
    }

    if any_behind {
        std::process::exit(1);
    }

    Ok(())
}

/// Determine which store slugs to check.
///
/// Priority:
/// 1. Explicit `args.store` slug.
/// 2. Slugs referenced in a project config found in the CWD ancestry.
/// 3. All known stores (`list_remotes()`).
fn discover_stores(args: &CheckArgs, _ctx: &Context) -> Result<Vec<String>> {
    // 1. Explicit store argument
    if let Some(ref slug) = args.store {
        config::validate_remote_slug(slug)?;
        return Ok(vec![slug.clone()]);
    }

    // 2. Project config
    if let Some((cfg, _path)) = config::load_project_config() {
        let slugs = collect_stores_from_project_config(&cfg);
        if !slugs.is_empty() {
            return Ok(slugs.into_iter().collect());
        }
    }

    // 3. All known stores
    crate::remote::list_remotes()
}

/// Extract unique store slugs referenced in a project config.
///
/// Sources:
/// - `default_store` field
/// - Paths inside `envs` entries that contain an `org/repo` prefix (e.g.
///   `"myorg/secrets/prod/DB_PASS"` → slug `"myorg/secrets"`).
fn collect_stores_from_project_config(cfg: &config::ProjectConfig) -> BTreeSet<String> {
    let mut slugs = BTreeSet::new();

    if let Some(ref s) = cfg.default_store {
        if config::validate_remote_slug(s).is_ok() {
            slugs.insert(s.clone());
        }
    }

    for entries in cfg.envs.values() {
        for entry in entries {
            // Tag selectors don't carry a path — they expand at resolve time
            // against whatever store the caller already chose. They cannot
            // contribute a slug to the auto-discovery list.
            let path = match entry {
                config::EnvEntry::Single(p) | config::EnvEntry::Glob(p) => p.as_str(),
                config::EnvEntry::Alias { path, .. } => path.as_str(),
                config::EnvEntry::Tag(_) | config::EnvEntry::AliasTag { .. } => continue,
            };
            // A qualified path looks like "org/repo/env/KEY" — at least 3 segments.
            // Extract the first two segments as the slug.
            let parts: Vec<&str> = path.splitn(3, '/').collect();
            if parts.len() >= 3 {
                let candidate = format!("{}/{}", parts[0], parts[1]);
                if config::validate_remote_slug(&candidate).is_ok() {
                    slugs.insert(candidate);
                }
            }
        }
    }

    slugs
}

/// Run git ahead/behind checks for a single store.
fn check_store(slug: &str, store_path: &Path, offline: bool) -> Result<StoreStatus> {
    debug!("checking store '{slug}' at {}", store_path.display());

    let mut warnings = Vec::new();

    // Fetch from remote (unless --offline)
    if !offline {
        match crate::git::run(&["fetch", "--quiet"], store_path) {
            Ok(_) => debug!("fetched '{slug}'"),
            Err(e) => {
                warnings.push(format!("fetch failed: {e}"));
            }
        }
    }

    // Current branch name
    let branch = match crate::git::run(&["rev-parse", "--abbrev-ref", "HEAD"], store_path) {
        Ok(b) => b.trim().to_string(),
        Err(e) => {
            return Err(HimitsuError::Git(format!(
                "could not determine branch for '{slug}': {e}"
            )));
        }
    };

    // Verify that the remote tracking branch exists
    let remote_ref = format!("origin/{branch}");
    let tracking_exists =
        crate::git::run(&["rev-parse", "--verify", &remote_ref], store_path).is_ok();

    if !tracking_exists {
        return Ok(StoreStatus {
            behind: 0,
            ahead: 0,
            dirty: false,
            branch,
            warnings: vec!["no remote tracking branch — skipping ahead/behind check".to_string()],
        });
    }

    // Behind count: commits in origin/<branch> not yet in HEAD
    let behind_str = crate::git::run(
        &["rev-list", "--count", &format!("HEAD..{remote_ref}")],
        store_path,
    )
    .unwrap_or_default();
    let behind: u32 = behind_str.trim().parse().unwrap_or(0);

    // Ahead count: commits in HEAD not yet in origin/<branch>
    let ahead_str = crate::git::run(
        &["rev-list", "--count", &format!("{remote_ref}..HEAD")],
        store_path,
    )
    .unwrap_or_default();
    let ahead: u32 = ahead_str.trim().parse().unwrap_or(0);

    // Dirty working tree
    let dirty = crate::git::run(&["status", "--short"], store_path)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    Ok(StoreStatus {
        behind,
        ahead,
        dirty,
        branch,
        warnings,
    })
}
