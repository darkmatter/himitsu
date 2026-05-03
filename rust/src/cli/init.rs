use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use clap::Args;

use super::Context;
use crate::config::{self, KeyProvider};
use crate::crypto::age;
use crate::error::Result;

/// Initialize himitsu (create keys, config, and optionally a store).
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Output result as JSON (for TUI consumption).
    #[arg(long, hide = true)]
    pub json: bool,

    /// Register an initial store by slug (e.g. `org/repo`) and set it as the
    /// default. The store is created at `stores_dir/<org>/<repo>` and gets an
    /// `origin` remote of `git@github.com:<org>/<repo>.git` if it does not
    /// already have a git remote.
    #[arg(long)]
    pub name: Option<String>,

    /// Git URL to restore the named store from (default: git@github.com:<org>/<repo>.git).
    #[arg(long, requires = "name")]
    pub url: Option<String>,

    /// Override the himitsu data directory (persisted to ~/.config/himitsu/home).
    #[arg(long, hide = true)]
    pub home: Option<String>,

    /// Select the key storage backend (e.g. `disk`, `macos-keychain`).
    #[arg(long, hide = true)]
    pub key_provider: Option<String>,

    /// Skip the TUI wizard and run in headless CLI mode.
    #[arg(long)]
    pub no_tui: bool,

    /// Project-scoped store slug. When set and the cwd (or a parent) is a
    /// git repo, write `default_store: <slug>` to `<git_root>/himitsu.yaml`
    /// and restore-or-create the store the same way `--name` does for the
    /// global default. Hidden from CLI help — the wizard is the primary
    /// entry point for project-scoped configuration.
    #[arg(long, hide = true)]
    pub project: Option<String>,
}

pub fn run(args: InitArgs, ctx: &Context) -> Result<()> {
    // ── TUI wizard mode ───────────────────────────────────────────────────
    // Launch the interactive ratatui wizard when stdout is a terminal and the
    // caller hasn't opted out via --json or --no-tui.
    if !args.json && !args.no_tui && std::io::stdout().is_terminal() {
        return crate::tui::run_init_flow();
    }

    // ── Handle --home override ────────────────────────────────────────────
    // Must happen before any path-dependent work so subsequent invocations
    // (including the one the TUI is about to make) pick up the new data_dir.
    if let Some(ref home) = args.home {
        // Persist the custom data_dir into ~/.config/himitsu/config.yaml.
        let config_path = config::config_path();
        let mut cfg = config::Config::load(&config_path)?;
        cfg.data_dir = Some(home.trim().to_string());
        cfg.save(&config_path)?;
        // Re-derive paths now that the config has been updated.
        let patched_ctx = Context {
            data_dir: config::data_dir(),
            state_dir: config::state_dir(),
            store: ctx.store.clone(),
            recipients_path: ctx.recipients_path.clone(),
        };
        return run_init(args, &patched_ctx);
    }

    run_init(args, ctx)
}

/// Core init logic, separated so the `--home` override path can call it with
/// a patched [`Context`].
pub(crate) fn run_init(args: InitArgs, ctx: &Context) -> Result<()> {
    let data_dir = &ctx.data_dir;
    let state_dir = &ctx.state_dir;

    // ── 1. Ensure data_dir exists (keys, config) ──────────────────────────
    let key_existed = data_dir.join("key").exists();

    std::fs::create_dir_all(data_dir)?;

    let key_path = data_dir.join("key");
    let pubkey_path = data_dir.join("key.pub");

    let pubkey = if !key_path.exists() {
        let (secret, public) = age::keygen();
        std::fs::write(
            &key_path,
            format!(
                "# created: {}\n# public key: {public}\n{secret}\n",
                timestamp()
            ),
        )?;
        std::fs::write(&pubkey_path, format!("{public}\n"))?;
        public
    } else {
        read_public_key(data_dir)?
    };

    let config_path = config::config_path();
    if !config_path.exists() {
        config::Config::write_default(&config_path)?;
    }

    // ── 2. Handle --key-provider ──────────────────────────────────────────
    if let Some(ref provider_str) = args.key_provider {
        let provider: KeyProvider = provider_str.parse()?;
        let mut cfg = config::Config::load(&config_path)?;
        cfg.key_provider = provider;
        cfg.save(&config_path)?;
    }

    // ── 3. Ensure state_dir exists (stores subdir) ────────────────────────
    std::fs::create_dir_all(state_dir.join("stores"))?;

    // ── 4. Optionally initialize a path-based store (--store flag) ────────
    let store = &ctx.store;
    let store_existed = if store.as_os_str().is_empty() {
        true // no store requested
    } else {
        store_exists(store)
    };

    if !store.as_os_str().is_empty() {
        ensure_store_layout(store, &pubkey)?;
    }

    // ── 5. Handle --name: register a named remote store and set as default ─
    let (name_registered, name_restored) = if let Some(ref slug) = args.name {
        let restored =
            restore_or_create_named_store(slug, args.url.as_deref(), &pubkey, state_dir)?;

        // Set (or update) default_store in global config
        let mut cfg = config::Config::load(&config_path)?;
        cfg.default_store = Some(slug.clone());
        cfg.save(&config_path)?;
        (true, restored)
    } else {
        (false, false)
    };

    // ── 6. Detect git context for suggestions ─────────────────────────────
    let in_git_repo = config::find_git_root(&std::env::current_dir()?).is_some();
    let git_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| config::find_git_root(&cwd));
    let suggested_remote = git_root.as_ref().and_then(detect_origin_remote);

    // ── 5b. Handle --project: write project-scoped default_store ──────────
    let (project_registered, project_restored, project_config_path) =
        if let Some(ref slug) = args.project {
            let root = git_root.clone().ok_or_else(|| {
                crate::error::HimitsuError::InvalidConfig(
                    "--project requires a git repository (none found from cwd)".into(),
                )
            })?;
            let restored =
                restore_or_create_named_store(slug, args.url.as_deref(), &pubkey, state_dir)?;
            let pc_path = root.join("himitsu.yaml");
            let mut pc = config::ProjectConfig::load_or_default(&pc_path)?;
            pc.default_store = Some(slug.clone());
            pc.save(&pc_path)?;
            (true, restored, Some(pc_path))
        } else {
            (false, false, None)
        };

    // ── 7. Read back the current key_provider ─────────────────────────────
    let cfg = config::Config::load(&config_path)?;
    let key_provider = cfg.key_provider.to_string();

    // ── 8. Output ─────────────────────────────────────────────────────────
    let anything_created = !key_existed
        || (!store_existed && !store.as_os_str().is_empty())
        || name_registered
        || project_registered;

    if args.json {
        let json = serde_json::json!({
            "data_dir": data_dir.to_string_lossy(),
            "state_dir": state_dir.to_string_lossy(),
            "store": store.to_string_lossy(),
            "pubkey": pubkey,
            "key_existed": key_existed,
            "store_existed": store_existed,
            "in_git_repo": in_git_repo,
            "suggested_remote": suggested_remote,
            "key_provider": key_provider,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !anything_created {
        // Already fully initialized — show summary.
        println!("Already initialized.");
        println!("  Public key: {pubkey}");
        println!("  Key provider: {key_provider}");
        // Show registered remote stores (if any).
        let remotes = crate::remote::list_remotes().unwrap_or_default();
        if !remotes.is_empty() {
            let default_slug = cfg.default_store.as_deref().unwrap_or("");
            for r in &remotes {
                if r == default_slug {
                    println!("  Stores: {r} (default)");
                } else {
                    println!("  Stores: {r}");
                }
            }
        }
    } else {
        // Wizard summary: show what was created.
        if !key_existed {
            println!("✓ Created age keypair");
            println!("  Public key: {pubkey}");
        }
        if !store_existed && !store.as_os_str().is_empty() {
            println!("✓ Initialized store at {}", store.display());
            if let Some(ref suggested) = suggested_remote {
                println!("  Detected git origin: {suggested}");
            }
        }
        if name_registered {
            let slug = args.name.as_deref().unwrap_or("");
            if name_restored {
                println!("✓ Restored store {slug} (default)");
            } else {
                println!("✓ Registered store {slug} (default)");
            }
        }
        if project_registered {
            let slug = args.project.as_deref().unwrap_or("");
            let path = project_config_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            if project_restored {
                println!("✓ Restored project store {slug} → {path}");
            } else {
                println!("✓ Registered project store {slug} → {path}");
            }
        }
        if args.key_provider.is_some() {
            println!("✓ Key provider: {key_provider}");
        }
        println!("✓ Created state directory");

        // Prompt to create a primary store if none was set up.
        if store.as_os_str().is_empty() && !name_registered {
            println!();
            let suggested_primary = suggested_remote_slug();
            if suggested_primary.is_empty() {
                println!(
                    "Run `himitsu init --name <your-github-username>/secrets` to create your primary personal GitHub store."
                );
            } else {
                println!(
                    "Run `himitsu init --name {suggested_primary}` to create your primary personal GitHub store."
                );
            }
        }
    }

    Ok(())
}

// ── Interactive wizard helpers ─────────────────────────────────────────────

/// Build a default primary-store slug suggestion for the init wizard.
///
/// A himitsu primary store should usually live under the user's personal
/// GitHub account rather than under whichever project/org the current repo
/// belongs to. Resolution order therefore prefers explicit personal-account
/// hints, falling back to the current repo's origin only when nothing better is
/// available.
pub(crate) fn suggested_remote_slug() -> String {
    detect_personal_github_username()
        .or_else(detect_origin_github_org)
        .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()))
        .map(|u| format!("{u}/secrets"))
        .unwrap_or_default()
}

/// Try to discover a personal GitHub username for the default primary-store
/// suggestion.
///
/// Resolution order:
/// 1. `$GITHUB_USER` / `$GITHUB_USERNAME`.
/// 2. `git config github.user`.
fn detect_personal_github_username() -> Option<String> {
    for var in ["GITHUB_USER", "GITHUB_USERNAME"] {
        if let Ok(user) = std::env::var(var) {
            let user = user.trim().to_string();
            if !user.is_empty() {
                return Some(user);
            }
        }
    }

    let output = std::process::Command::new("git")
        .args(["config", "github.user"])
        .output()
        .ok()?;
    if output.status.success() {
        let user = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !user.is_empty() {
            return Some(user);
        }
    }

    None
}

/// Build a default project-store slug suggestion: `<repo-org>/secrets`,
/// where `<repo-org>` is the GitHub org of the current repo's `origin`
/// remote. Returns an empty string when not in a git repo or when the origin
/// can't be parsed as a GitHub slug.
pub(crate) fn suggested_project_slug() -> String {
    detect_origin_github_org()
        .map(|org| format!("{org}/secrets"))
        .unwrap_or_default()
}

fn detect_origin_github_org() -> Option<String> {
    let slug = std::env::current_dir()
        .ok()
        .and_then(|cwd| config::find_git_root(&cwd))
        .as_ref()
        .and_then(detect_origin_remote)?;
    let (org, _) = slug.split_once('/')?;
    Some(org.to_string())
}

// ── Helpers ────────────────────────────────────────────────────────────────

pub(crate) fn read_public_key(data_dir: &Path) -> Result<String> {
    let pubkey_path = data_dir.join("key.pub");
    if pubkey_path.exists() {
        return Ok(std::fs::read_to_string(&pubkey_path)?.trim().to_string());
    }

    let key_path = data_dir.join("key");
    let contents = std::fs::read_to_string(&key_path)?;
    Ok(extract_public_key(&contents).unwrap_or_default())
}

pub(crate) fn store_exists(store: &Path) -> bool {
    crate::remote::store::secrets_dir(store).exists()
}

/// Restore a slug-managed store from git when it already exists remotely,
/// otherwise create a fresh local checkout with the standard GitHub origin.
///
/// Returns `true` when existing remote contents were restored or updated.
fn restore_or_create_named_store(
    slug: &str,
    url: Option<&str>,
    pubkey: &str,
    state_dir: &Path,
) -> Result<bool> {
    let (org, repo) = config::validate_remote_slug(slug)?;
    let dest = config::store_checkout(org, repo);
    let clone_url = url
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("git@github.com:{org}/{repo}.git"));

    let restored = if dest.exists() {
        pull_existing_store(&dest)
    } else {
        match crate::git::clone_noninteractive(&clone_url, &dest) {
            Ok(_) => true,
            Err(e) => {
                tracing::debug!(
                    "could not clone existing store {slug} from {clone_url}; creating local store: {e}"
                );
                false
            }
        }
    };

    ensure_store_layout(&dest, pubkey)?;
    if crate::git::has_any_remote(&dest) {
        // A clone (or an existing checkout) already carries the user's remote.
    } else if let Some(url) = url {
        if let Err(e) = crate::git::add_remote(&dest, "origin", url) {
            tracing::debug!("failed to set origin for {}: {e}", dest.display());
        }
    } else {
        ensure_default_origin(&dest, &state_dir.join("stores"));
    }

    Ok(restored)
}

fn pull_existing_store(store: &Path) -> bool {
    if !store.join(".git").exists() || !crate::git::has_any_remote(store) {
        return false;
    }

    match crate::git::pull_or_checkout_origin(store) {
        Ok(_) => true,
        Err(e) => {
            tracing::debug!("failed to restore {} from origin: {e}", store.display());
            false
        }
    }
}

pub(crate) fn ensure_store_layout(store: &Path, pubkey: &str) -> Result<bool> {
    let existed = store_exists(store);

    if !existed {
        std::fs::create_dir_all(crate::remote::store::secrets_dir(store))?;
    }

    let recipients_dir = crate::remote::store::recipients_dir(store);
    std::fs::create_dir_all(&recipients_dir)?;
    let self_pub = recipients_dir.join("self.pub");
    if !self_pub.exists() && !pubkey.is_empty() {
        std::fs::write(&self_pub, format!("{pubkey}\n"))?;
    }

    // Ensure the store is a git repo. Idempotent: skips if .git already exists
    // (e.g. from `remote add` which clones). Creates an initial commit so HEAD
    // is valid for commands like `git status` and `check`.
    ensure_git_repo(store);

    Ok(!existed)
}

/// Idempotent: ensure a store directory is a git repository with at least one
/// commit. Safe to call on stores that were cloned via `remote add` (no-ops
/// when `.git` already exists).
pub(crate) fn ensure_git_repo(store: &Path) {
    use crate::git;

    if store.as_os_str().is_empty() || store.join(".git").exists() {
        return;
    }

    if let Err(e) = git::init(store) {
        tracing::debug!("git init failed for {}: {e}", store.display());
        return;
    }

    // Stage everything and create an initial commit so HEAD exists.
    let _ = git::run(&["add", "."], store);
    let _ = git::run(&["commit", "-m", "chore: initialize himitsu store"], store);
}

/// Idempotent: ensure a slug-managed store has at least one git remote
/// configured. Without this, every auto-commit lands in a local-only repo
/// and never pushes — commits accumulate silently.
///
/// When the store sits at `<stores_dir>/<org>/<repo>` and has no remotes,
/// adds `origin = git@github.com:<org>/<repo>.git` (the same default URL
/// `remote add` uses). No-op when:
///   * the store path is not under `stores_dir` (caller doesn't manage it),
///   * the store is not a git repo,
///   * any remote is already configured (caller already chose a remote).
pub(crate) fn ensure_default_origin(store: &Path, stores_dir: &Path) {
    use crate::git;

    if store.as_os_str().is_empty() || !store.join(".git").exists() {
        return;
    }
    if git::has_any_remote(store) {
        return;
    }

    let Ok(rel) = store.strip_prefix(stores_dir) else {
        return;
    };
    let parts: Vec<&str> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    if parts.len() != 2 {
        return;
    }
    let (org, repo) = (parts[0], parts[1]);

    // Sanity: the slug must round-trip through the validator. Guards against
    // legacy directories like `git@github.com:foo/` left over from earlier
    // bugs in `remote add` slug parsing.
    if config::validate_remote_slug(&format!("{org}/{repo}")).is_err() {
        return;
    }

    let url = format!("git@github.com:{org}/{repo}.git");
    if let Err(e) = git::add_remote(store, "origin", &url) {
        tracing::debug!("failed to set origin for {}: {e}", store.display());
    } else {
        tracing::debug!("set default origin {url} for {}", store.display());
    }
}

fn timestamp() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

fn extract_public_key(contents: &str) -> Option<String> {
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("# public key: ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn detect_origin_remote(git_root: &PathBuf) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(git_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_remote_slug(&url)
}

pub(crate) fn parse_remote_slug(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return Some(rest.strip_suffix(".git").unwrap_or(rest).to_string());
    }
    if let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        return Some(rest.strip_suffix(".git").unwrap_or(rest).to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    // parsing
    #[test]
    fn parse_ssh_remote() {
        assert_eq!(
            parse_remote_slug("git@github.com:myorg/myrepo.git"),
            Some("myorg/myrepo".into())
        );
    }

    #[test]
    fn parse_https_remote() {
        assert_eq!(
            parse_remote_slug("https://github.com/myorg/myrepo.git"),
            Some("myorg/myrepo".into())
        );
    }

    #[test]
    fn parse_unknown_url_returns_none() {
        assert_eq!(parse_remote_slug("https://gitlab.com/foo/bar"), None);
    }

    // ── ensure_default_origin ────────────────────────────────────────────

    /// Helper: layout `<root>/stores/<org>/<repo>` with `git init` and return
    /// the `(stores_dir, store_path)` pair.
    fn make_store_under(root: &Path, org: &str, repo: &str) -> (PathBuf, PathBuf) {
        let stores = root.join("stores");
        let store = stores.join(org).join(repo);
        std::fs::create_dir_all(&store).unwrap();
        crate::git::init(&store).unwrap();
        (stores, store)
    }

    #[test]
    fn ensure_default_origin_sets_github_url_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let (stores, store) = make_store_under(tmp.path(), "myorg", "secrets");

        // Reproduce the bug: store has commits but no remote. Auto-commit
        // dispatcher would silently never push.
        assert!(!crate::git::has_any_remote(&store));

        ensure_default_origin(&store, &stores);

        assert!(crate::git::has_any_remote(&store));
        let url = crate::git::run(&["remote", "get-url", "origin"], &store).unwrap();
        assert_eq!(url.trim(), "git@github.com:myorg/secrets.git");
    }

    #[test]
    fn ensure_default_origin_noop_when_remote_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let (stores, store) = make_store_under(tmp.path(), "myorg", "secrets");
        crate::git::add_remote(&store, "origin", "https://example.com/custom.git").unwrap();

        ensure_default_origin(&store, &stores);

        // Existing remote must be preserved — the user picked it.
        let url = crate::git::run(&["remote", "get-url", "origin"], &store).unwrap();
        assert_eq!(url.trim(), "https://example.com/custom.git");
    }

    #[test]
    fn ensure_default_origin_noop_for_path_outside_stores_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("loose-store");
        std::fs::create_dir_all(&store).unwrap();
        crate::git::init(&store).unwrap();
        let stores = tmp.path().join("stores");
        std::fs::create_dir_all(&stores).unwrap();

        ensure_default_origin(&store, &stores);

        assert!(
            !crate::git::has_any_remote(&store),
            "stores outside stores_dir aren't slug-managed; we can't guess a URL"
        );
    }

    #[test]
    fn ensure_default_origin_noop_for_non_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join("stores");
        let store = stores.join("myorg").join("secrets");
        std::fs::create_dir_all(&store).unwrap();
        // No git init → must not panic, must not somehow set a remote.

        ensure_default_origin(&store, &stores);

        assert!(!store.join(".git").exists());
    }

    #[test]
    fn ensure_default_origin_rejects_garbage_slug_dirs() {
        // Reproduces a pre-existing bug where `remote add` accepted a full URL
        // as the slug and created `stores/git@github.com:foo/bar/`. We must
        // not configure an origin for those — the slug is not valid.
        let tmp = tempfile::tempdir().unwrap();
        let (stores, store) = make_store_under(tmp.path(), "git@github.com:foo", "secrets");

        ensure_default_origin(&store, &stores);

        assert!(!crate::git::has_any_remote(&store));
    }
}
