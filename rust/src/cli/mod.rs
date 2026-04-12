pub mod check;
pub mod codegen;
pub mod context;
pub mod decrypt;
pub mod encrypt;
pub mod generate;
pub mod get;
pub mod git;
pub mod group;
pub mod import;
pub mod inbox;
pub mod init;
pub mod ls;
pub mod recipient;
pub mod rekey;
pub mod remote;
pub mod schema;
pub mod search;
pub mod set;
pub mod share;
pub mod sync;

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use tracing::debug;

use crate::error::{HimitsuError, Result};

/// Resolved paths for the current invocation.
pub struct Context {
    /// XDG data directory: `~/.local/share/himitsu/` (keys, config).
    pub data_dir: PathBuf,
    /// XDG state directory: `~/.local/state/himitsu/` (db, stores).
    pub state_dir: PathBuf,
    /// Resolved store checkout path (may be empty if no store needed).
    pub store: PathBuf,
    /// Optional override for the recipients directory within `store`.
    ///
    /// Loaded from the store-internal `.himitsu/config.yaml` first, then
    /// from the project-level `himitsu.yaml` `store.recipients_path` field.
    /// When `None`, the default `.himitsu/recipients/` layout is used.
    pub recipients_path: Option<String>,
}

impl Context {
    /// Path to the age private key file.
    pub fn key_path(&self) -> PathBuf {
        self.data_dir.join("key")
    }

    /// Path to the age public key file.
    #[allow(dead_code)]
    pub fn pubkey_path(&self) -> PathBuf {
        self.data_dir.join("key.pub")
    }

    /// Directory containing managed store checkouts.
    pub fn stores_dir(&self) -> PathBuf {
        self.state_dir.join("stores")
    }

    /// Find the git root: in the new model the store itself is the git root.
    pub fn git_root(&self) -> Option<PathBuf> {
        if self.store.as_os_str().is_empty() {
            return None;
        }
        if self.store.join(".git").exists() {
            return Some(self.store.clone());
        }
        crate::config::find_git_root(&self.store)
    }

    /// Commit `.himitsu/` changes inside the store and push to origin.
    /// Best-effort: does not fail if no git repo or no remote configured.
    pub fn commit_and_push(&self, message: &str) {
        let Some(git_root) = self.git_root() else {
            return;
        };
        let _ = crate::git::run(&["add", ".himitsu"], &git_root);

        if crate::git::run(&["diff", "--cached", "--quiet"], &git_root).is_err() {
            let _ = crate::git::run(&["commit", "-m", message], &git_root);
            debug!("committed: {message}");
        }

        match crate::git::push(&git_root) {
            Ok(_) => debug!("pushed to remote"),
            Err(e) => debug!("push skipped: {e}"),
        }
    }
}

/// Himitsu - age-based secrets management with transport-agnostic sharing.
#[derive(Debug, Parser)]
#[command(
    name = "himitsu",
    version = crate::build_info::VERSION,
    about,
    long_about = None
)]
pub struct Cli {
    /// Override the store path directly (for testing or advanced use).
    #[arg(short = 's', long, global = true)]
    pub store: Option<String>,

    /// Select a remote store by org/repo slug (resolves via stores_dir).
    /// Mutually exclusive with --store.
    #[arg(short = 'r', long, global = true, conflicts_with = "store")]
    pub remote: Option<String>,

    /// Increase log verbosity (-v for debug, -vv for trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize himitsu (create keys, config, and optionally a store).
    Init(init::InitArgs),

    /// Set a secret value.
    Set(set::SetArgs),

    /// Get a secret value.
    Get(get::GetArgs),

    /// List secrets in the store (or all stores if none resolved).
    Ls(ls::LsArgs),

    /// Re-encrypt secrets for current recipients.
    Rekey(rekey::RekeyArgs),

    /// (Deprecated) Re-encrypt secrets. Use `rekey` instead.
    #[command(hide = true)]
    Encrypt(encrypt::EncryptArgs),

    /// Not supported — secrets are never stored in plaintext. Use 'get <path>' to read individual values.
    #[command(hide = true)]
    Decrypt(decrypt::DecryptArgs),

    /// Sync stores: pull from git remote and optionally rekey drifted secrets.
    Sync(sync::SyncArgs),

    /// Search secrets across all known projects.
    Search(search::SearchArgs),

    /// Manage recipients.
    Recipient(recipient::RecipientArgs),

    /// Manage recipient groups.
    Group(group::GroupArgs),

    /// Manage remote stores (add, remove, list, set default).
    Remote(remote::RemoteArgs),

    /// Manage the active store context used for disambiguation.
    Context(context::ContextArgs),

    /// (Internal) Generate and manage JSON schemas for himitsu config files.
    #[command(hide = true)]
    Schema(schema::SchemaArgs),

    /// Generate SOPS-encrypted output files from env definitions in project config.
    Generate(generate::GenerateArgs),

    /// (Legacy) Generate typed config code from secrets. See 'generate' for canonical output.
    #[command(hide = true)]
    Codegen(codegen::CodegenArgs),

    /// Run git commands inside a store checkout (or all stores with --all).
    Git(git::GitArgs),

    /// Verify store checkouts are up to date with their remotes.
    Check(check::CheckArgs),

    /// Print version information.
    Version,

    // ── Hidden commands (not yet implemented) ─────────────────────
    /// Share secrets with external recipients.
    #[command(hide = true)]
    Share(share::ShareArgs),

    /// Manage the incoming secret inbox.
    #[command(hide = true)]
    Inbox(inbox::InboxArgs),

    /// Import secrets from external stores.
    #[command(hide = true)]
    Import(import::ImportArgs),
}

impl Cli {
    pub fn run(self) -> Result<()> {
        let command = match self.command {
            Some(cmd) => cmd,
            None => return Self::launch_tui(),
        };

        let data_dir = crate::config::data_dir();
        let state_dir = crate::config::state_dir();

        // ── First-use auto-initialization ────────────────────────────────
        // For all non-init, non-git commands: if himitsu is not initialized,
        // automatically run init (no prompt needed — the user clearly wants
        // to use himitsu).
        let is_init = matches!(&command, Command::Init(_));
        let is_git = matches!(&command, Command::Git(_));
        let is_version = matches!(&command, Command::Version);

        if !is_init && !is_git && !is_version && !data_dir.join("key").exists() {
            eprintln!("First run — initializing himitsu...");
            let ctx = Context {
                data_dir: data_dir.clone(),
                state_dir: state_dir.clone(),
                store: PathBuf::new(),
                recipients_path: None,
            };
            init::run(
                init::InitArgs {
                    json: false,
                    name: None,
                    home: None,
                    key_provider: None,
                    no_tui: true,
                },
                &ctx,
            )?;
            eprintln!();
        }

        // ── Resolve store ─────────────────────────────────────────────────────
        let store_override: Option<PathBuf> = if let Some(slug) = &self.remote {
            // ensure_store validates the slug and lazy-clones if the checkout
            // doesn't exist locally yet.
            Some(crate::config::ensure_store(slug)?)
        } else {
            self.store.as_ref().map(PathBuf::from)
        };

        // Commands that require a resolved store
        let needs_store = matches!(
            &command,
            Command::Set(_)
                | Command::Get(_)
                | Command::Rekey(_)
                | Command::Recipient(_)
                | Command::Group(_)
                | Command::Schema(_)
                | Command::Generate(_)
                | Command::Codegen(_)
                | Command::Share(_)
                | Command::Import(_)
        );

        let store = if let Some(ref p) = store_override {
            p.clone()
        } else if needs_store {
            crate::config::resolve_store(None)?
        } else {
            // Init, Ls, Search, Remote, Git, Version: store is optional
            PathBuf::new()
        };

        if self.store.is_some()
            && command_uses_explicit_path_store(&command)
            && !init::store_exists(&store)
        {
            prompt_to_create_store(&store, &data_dir, &state_dir)?;
        }

        let recipients_path = load_recipients_path_override(&store);
        let ctx = Context {
            data_dir,
            state_dir,
            store,
            recipients_path,
        };

        match command {
            Command::Init(args) => init::run(args, &ctx),
            Command::Set(args) => set::run(args, &ctx),
            Command::Get(args) => get::run(args, &ctx),
            Command::Ls(args) => ls::run(args, &ctx),
            Command::Rekey(args) => rekey::run(args, &ctx),
            Command::Encrypt(args) => encrypt::run(args, &ctx),
            Command::Decrypt(args) => decrypt::run(args, &ctx),
            Command::Sync(args) => sync::run(args, &ctx),
            Command::Search(args) => search::run(args, &ctx),
            Command::Recipient(args) => recipient::run(args, &ctx),
            Command::Group(args) => group::run(args, &ctx),
            Command::Remote(args) => remote::run(args, &ctx),
            Command::Context(args) => context::run(args, &ctx),

            Command::Schema(args) => schema::run(args, &ctx),
            Command::Generate(args) => generate::run(args, &ctx),
            Command::Codegen(args) => codegen::run(args, &ctx),
            Command::Git(args) => git::run(args, &ctx),
            Command::Check(args) => check::run(args, &ctx),
            Command::Version => {
                println!("{}", crate::build_info::VERSION_LINE);
                Ok(())
            }
            Command::Share(args) => share::run(args, &ctx),
            Command::Inbox(args) => inbox::run(args, &ctx),
            Command::Import(args) => import::run(args, &ctx),
        }
    }

    fn launch_tui() -> Result<()> {
        use std::process::Command as Cmd;

        match Cmd::new("himitsu-tui").status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(HimitsuError::NotSupported(
                "TUI not found. Install himitsu-tui or run a subcommand (try `himitsu --help`).".into(),
            )),
            Err(e) => Err(e.into()),
        }
    }
}

fn command_uses_explicit_path_store(command: &Command) -> bool {
    matches!(
        command,
        Command::Set(_)
            | Command::Get(_)
            | Command::Ls(_)
            | Command::Rekey(_)
            | Command::Encrypt(_)
            | Command::Decrypt(_)
            | Command::Recipient(_)
            | Command::Group(_)
            | Command::Schema(_)
            | Command::Generate(_)
            | Command::Codegen(_)
            | Command::Share(_)
            | Command::Import(_)
    )
}

/// When no store exists, himitsu will prompt the user to create one.
fn prompt_to_create_store(store: &Path, data_dir: &Path, state_dir: &Path) -> Result<()> {
    eprint!("No store exists. Create one at {}? Y/n ", store.display());
    io::stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;

    let answer = response.trim();
    if !answer.is_empty()
        && !answer.eq_ignore_ascii_case("y")
        && !answer.eq_ignore_ascii_case("yes")
    {
        return Err(HimitsuError::StoreNotFound(format!(
            "store creation declined for {}",
            store.display()
        )));
    }

    std::fs::create_dir_all(state_dir.join("stores"))?;
    let pubkey = init::read_public_key(data_dir)?;
    init::ensure_store_layout(store, &pubkey)?;
    Ok(())
}

// ── Context helpers ──────────────────────────────────────────────────────────

/// Determine the recipients directory override for a resolved store.
///
/// Resolution order (first `Some` wins):
/// 1. Store-internal `.himitsu/config.yaml` → `StoreConfig.recipients_path`
/// 2. Project config (walked up from CWD) → `store.recipients_path`
/// 3. `None` → use default `.himitsu/recipients/` layout
fn load_recipients_path_override(store: &std::path::Path) -> Option<String> {
    if store.as_os_str().is_empty() {
        return None;
    }

    // 1. Check store-internal config
    let store_cfg_path = crate::remote::store::store_config_path(store);
    if store_cfg_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&store_cfg_path) {
            if let Ok(cfg) = serde_yaml::from_str::<crate::config::StoreConfig>(&contents) {
                if cfg.recipients_path.is_some() {
                    return cfg.recipients_path;
                }
            }
        }
    }

    // 2. Check project config
    if let Some((project_cfg, _)) = crate::config::load_project_config() {
        if let Some(ref store_cfg) = project_cfg.store {
            if store_cfg.recipients_path.is_some() {
                return store_cfg.recipients_path.clone();
            }
        }
    }

    None
}
