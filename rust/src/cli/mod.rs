pub mod codegen;
pub mod decrypt;
pub mod encrypt;
pub mod get;
pub mod git;
pub mod group;
pub mod import;
pub mod inbox;
pub mod init;
pub mod ls;
pub mod recipient;
pub mod remote;
pub mod schema;
pub mod search;
pub mod set;
pub mod share;
pub mod sync;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing::debug;

use crate::error::Result;

/// Resolved paths for the current invocation.
pub struct Context {
    /// User-level home: `~/.himitsu/` (keys, config, search index).
    pub user_home: PathBuf,
    /// Project store: `$GIT_ROOT/.himitsu/` or `~/.himitsu/` for personal use.
    pub store: PathBuf,
}

impl Context {
    /// Find the git root for the current store.
    /// Store is typically `$GIT_ROOT/.himitsu/`, so parent is the git root.
    pub fn git_root(&self) -> Option<PathBuf> {
        let parent = self.store.parent()?;
        if parent.join(".git").exists() {
            return Some(parent.to_path_buf());
        }
        crate::config::find_git_root(&self.store)
    }

    /// Commit changed files in `.himitsu/` and push to origin.
    /// Best-effort: does not fail if no git repo or no remote configured.
    pub fn commit_and_push(&self, message: &str) {
        let Some(git_root) = self.git_root() else {
            return;
        };
        // Stage all .himitsu/ changes
        if let Ok(rel) = self.store.strip_prefix(&git_root) {
            let rel_str = rel.to_string_lossy();
            let _ = crate::git::run(&["add", &rel_str], &git_root);
        } else {
            let _ = crate::git::run(&["add", ".himitsu"], &git_root);
        }

        if crate::git::run(&["diff", "--cached", "--quiet"], &git_root).is_err() {
            // There are staged changes
            let _ = crate::git::run(&["commit", "-m", message], &git_root);
            debug!("committed: {message}");
        }

        // Push -- best effort
        match crate::git::push(&git_root) {
            Ok(_) => debug!("pushed to remote"),
            Err(e) => debug!("push skipped: {e}"),
        }
    }
}

/// Himitsu - age-based secrets management with transport-agnostic sharing.
#[derive(Debug, Parser)]
#[command(name = "himitsu", version, about, long_about = None)]
pub struct Cli {
    /// Override the store path (default: $GIT_ROOT/.himitsu/ or ~/.himitsu/).
    #[arg(short = 's', long, global = true)]
    pub store: Option<String>,

    /// Select a remote store by org/repo slug (resolves to ~/.himitsu/data/<org>/<repo>).
    /// Mutually exclusive with --store.
    #[arg(short = 'r', long, global = true, conflicts_with = "store")]
    pub remote: Option<String>,

    /// Increase log verbosity (-v for debug, -vv for trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize himitsu in the current project (or globally).
    Init(init::InitArgs),

    /// Set a secret value.
    Set(set::SetArgs),

    /// Get a secret value.
    Get(get::GetArgs),

    /// List environments or secrets.
    Ls(ls::LsArgs),

    /// Re-encrypt secrets for current recipients.
    Encrypt(encrypt::EncryptArgs),

    /// Decrypt secrets (not supported - secrets are never stored in plaintext).
    Decrypt(decrypt::DecryptArgs),

    /// Sync secrets with a remote store.
    Sync(sync::SyncArgs),

    /// Search secrets across all known projects.
    Search(search::SearchArgs),

    /// Manage recipients.
    Recipient(recipient::RecipientArgs),

    /// Manage recipient groups.
    Group(group::GroupArgs),

    /// Manage remote sync targets.
    Remote(remote::RemoteArgs),

    /// Generate and manage JSON schemas.
    Schema(schema::SchemaArgs),

    /// Generate typed config code from secrets.
    Codegen(codegen::CodegenArgs),

    /// Run git commands inside the himitsu directory (~/.himitsu).
    Git(git::GitArgs),

    // ── Hidden commands (not yet implemented) ─────────────────────
    // These parse and dispatch normally but are omitted from `--help`
    // because they are stubs.  They will be promoted to visible once
    // the backing implementation lands.
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
        let user_home = crate::config::user_home();

        // Resolve --remote slug into a concrete store override path.
        // --remote and --store are mutually exclusive (enforced by clap).
        let store_override: Option<String> = match &self.remote {
            Some(slug) => Some(
                crate::config::remote_store_path(&user_home, slug)?
                    .to_string_lossy()
                    .to_string(),
            ),
            None => self.store.clone(),
        };

        let needs_store = !matches!(self.command, Command::Init(_) | Command::Git(_));

        let store = if matches!(self.command, Command::Init(_)) {
            crate::config::store_path_or_default(&store_override)
        } else if needs_store {
            match crate::config::store_path(&store_override) {
                Ok(s) => s,
                Err(crate::error::HimitsuError::NotInitialized) => {
                    // Smart init: prompt the user instead of hard-erroring.
                    eprintln!("You have not initialized your secrets directory (~/.himitsu).");
                    eprint!("Would you like to do so now? [y/N] ");
                    std::io::Write::flush(&mut std::io::stderr())?;
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;
                    let yes = {
                        let t = input.trim();
                        t.eq_ignore_ascii_case("y") || t.eq_ignore_ascii_case("yes")
                    };
                    if yes {
                        eprintln!();
                        let ctx = Context {
                            user_home: user_home.clone(),
                            store: crate::config::store_path_or_default(&store_override),
                        };
                        init::run(init::InitArgs { json: false }, &ctx)?;
                        eprintln!();
                        crate::config::store_path(&store_override)?
                    } else {
                        return Ok(());
                    }
                }
                Err(e) => return Err(e),
            }
        } else {
            crate::config::store_path_or_default(&store_override)
        };

        let ctx = Context { user_home, store };

        match self.command {
            Command::Init(args) => init::run(args, &ctx),
            Command::Set(args) => set::run(args, &ctx),
            Command::Get(args) => get::run(args, &ctx),
            Command::Ls(args) => ls::run(args, &ctx),
            Command::Encrypt(args) => encrypt::run(args, &ctx),
            Command::Decrypt(args) => decrypt::run(args, &ctx),
            Command::Sync(args) => sync::run(args, &ctx),
            Command::Search(args) => search::run(args, &ctx),
            Command::Recipient(args) => recipient::run(args, &ctx),
            Command::Group(args) => group::run(args, &ctx),
            Command::Remote(args) => remote::run(args, &ctx),
            Command::Schema(args) => schema::run(args, &ctx),
            Command::Codegen(args) => codegen::run(args, &ctx),
            Command::Git(args) => git::run(args, &ctx),
            Command::Share(args) => share::run(args, &ctx),
            Command::Inbox(args) => inbox::run(args, &ctx),
            Command::Import(args) => import::run(args, &ctx),
        }
    }
}
