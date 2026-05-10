pub mod check;
pub mod codegen;
pub mod completions;
pub mod context;
pub mod decrypt;
pub mod docs;
pub mod duration;
pub mod encrypt;
pub mod exec;
pub mod export;
pub mod generate;
pub mod get;
pub mod git;
pub mod import;
pub mod inbox;
pub mod init;
pub mod join;
pub mod ls;
pub mod read;
pub mod recipient;
pub mod rekey;
pub mod remote;
pub mod schema;
pub mod search;
pub mod set;
pub mod share;
pub mod sync;
pub mod tag;
pub mod write;

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use tracing::debug;

use crate::error::{HimitsuError, Result};

/// Resolved paths for the current invocation.
#[derive(Clone)]
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
    /// Where the age private key lives. Resolved from `Config.key_provider`
    /// at dispatcher boot so callers don't each re-read the config.
    pub key_provider: crate::config::KeyProvider,
}

impl Context {
    /// Path to the age private key file. Only valid for the
    /// [`Disk`](crate::config::KeyProvider::Disk) provider — with the
    /// keychain provider this path doesn't exist, so callers should
    /// reach the secret through [`Self::load_identity`] instead of
    /// reading the path directly.
    pub fn key_path(&self) -> PathBuf {
        crate::crypto::keystore::disk_secret_path(&self.data_dir)
    }

    /// Path to the age public key file. Always written (provider-agnostic).
    #[allow(dead_code)]
    pub fn pubkey_path(&self) -> PathBuf {
        crate::crypto::keystore::pubkey_path(&self.data_dir)
    }

    /// Load the user's age identity through the active provider. This is
    /// the chokepoint: every command that decrypts goes through it.
    pub fn load_identity(&self) -> Result<::age::x25519::Identity> {
        crate::crypto::keystore::load_identity(&self.key_provider, &self.data_dir)
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

    /// Stage `.himitsu/` and commit if there are staged changes.
    ///
    /// Returns `true` when a parent commit was created. Best-effort: missing
    /// git repo or clean tree yields `false` without erroring.
    ///
    /// Submodules are committed first so the parent's pointer-bump references
    /// a real commit on the submodule side. Without this, a write to a path
    /// inside a submodule would strand the file — submodule dirty, parent
    /// clean, nothing to push.
    pub fn commit(&self, message: &str) -> bool {
        let Some(git_root) = self.git_root() else {
            return false;
        };

        for sm in crate::git::list_submodules(&git_root) {
            let status = crate::git::run(&["status", "--porcelain"], &sm).unwrap_or_default();
            if status.trim().is_empty() {
                continue;
            }
            if let Err(e) = crate::git::ensure_on_branch(&sm) {
                eprintln!(
                    "warning: submodule at {} can't commit: {e}\n  \
                     working tree left dirty — resolve manually.",
                    sm.display()
                );
                continue;
            }
            let _ = crate::git::run(&["add", "-A"], &sm);
            if crate::git::run(&["diff", "--cached", "--quiet"], &sm).is_err() {
                let _ = crate::git::run(&["commit", "-m", message], &sm);
                debug!("committed in submodule {}: {message}", sm.display());
            }
        }

        let _ = crate::git::run(&["add", ".himitsu"], &git_root);

        if crate::git::run(&["diff", "--cached", "--quiet"], &git_root).is_err() {
            let _ = crate::git::run(&["commit", "-m", message], &git_root);
            debug!("committed: {message}");
            true
        } else {
            false
        }
    }

    /// Fetch from `origin` and fast-forward `pull` the store's working tree.
    /// Used by the `auto_pull` config to give a `git fetch && himitsu <cmd>`
    /// workflow with no extra commands.
    ///
    /// Best-effort: a missing git repo, no remote, no upstream branch, or a
    /// network failure all degrade to a stderr warning so the underlying
    /// command can still run offline.
    pub fn pull_if_remote(&self) {
        let Some(git_root) = self.git_root() else {
            return;
        };
        if !crate::git::has_any_remote(&git_root) {
            return;
        }
        if let Err(e) = crate::git::run(&["fetch", "--recurse-submodules", "origin"], &git_root) {
            eprintln!("warning: auto-pull fetch failed: {e}");
            return;
        }
        // --ff-only is intentional: the dispatcher's invariant is a clean
        // working tree, so a non-fast-forward state means someone modified
        // history out-of-band. Surface that loudly instead of silently
        // creating a merge commit. --recurse-submodules updates submodule
        // working trees to match the fetched pointers.
        if let Err(e) = crate::git::run(&["pull", "--ff-only", "--recurse-submodules"], &git_root) {
            eprintln!("warning: auto-pull skipped (not fast-forward or no upstream): {e}");
        }
    }

    /// Push the store's git repo to its remote. Best-effort: failures are
    /// logged at debug and discarded (offline, auth issues, etc.).
    ///
    /// Special case: when the store has *no* remote configured at all, emit a
    /// one-shot stderr warning instead of a silent debug log. Otherwise
    /// every mutation appears to succeed while commits accumulate locally
    /// forever — the exact failure mode this dispatcher is meant to prevent.
    pub fn push(&self) {
        let Some(git_root) = self.git_root() else {
            return;
        };

        // Push submodules first. If any fail, hold back the parent push —
        // otherwise the parent's pointer would reference a commit that
        // doesn't exist on the submodule's remote, leaving a fresh clone
        // with a dangling ref. Next invocation will retry.
        let mut submodule_push_failed = false;
        for sm in crate::git::list_submodules(&git_root) {
            if !crate::git::has_any_remote(&sm) {
                eprintln!(
                    "warning: submodule at {} has no git remote — commit landed locally only.",
                    sm.display()
                );
                submodule_push_failed = true;
                continue;
            }
            if !crate::git::has_unpushed_commits(&sm) {
                continue;
            }
            match crate::git::push(&sm) {
                Ok(_) => debug!("pushed submodule {}", sm.display()),
                Err(e) => {
                    eprintln!(
                        "warning: submodule push failed at {}: {e}\n  \
                         parent push held back — retry after resolving.",
                        sm.display()
                    );
                    submodule_push_failed = true;
                }
            }
        }
        if submodule_push_failed {
            return;
        }

        if !crate::git::has_any_remote(&git_root) {
            eprintln!(
                "warning: store at {} has no git remote — commit landed locally only.\n  \
                 Add one with: himitsu git remote add origin <url>",
                git_root.display()
            );
            return;
        }
        match crate::git::push(&git_root) {
            Ok(_) => debug!("pushed to remote"),
            Err(e) => debug!("push skipped: {e}"),
        }
    }

    /// Back-compat shim: commit and push in one step on the success path.
    /// Prefer `commit` + `push` directly so failure paths can still commit.
    pub fn commit_and_push(&self, message: &str) {
        self.commit(message);
        self.push();
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
    #[command(alias = "add")]
    Set(set::SetArgs),

    /// Get a secret value.
    Get(get::GetArgs),

    /// Read a secret's plaintext to stdout with no decoration (scripting).
    Read(read::ReadArgs),

    /// Write a secret's plaintext from argument or stdin with no decoration (scripting).
    Write(write::WriteArgs),

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

    /// Join a store by adding your own age key to its recipient list.
    Join(join::JoinArgs),

    /// Sync stores: pull from git remote and optionally rekey drifted secrets.
    Sync(sync::SyncArgs),

    /// Search secrets across all known projects.
    Search(search::SearchArgs),

    /// Manage tags on a secret (add, rm, list).
    Tag(tag::TagArgs),

    /// Manage recipients.
    Recipient(recipient::RecipientArgs),

    /// Manage remote stores (add, remove, list, set default).
    Remote(remote::RemoteArgs),

    /// Manage the active store context used for disambiguation.
    Context(context::ContextArgs),

    /// (Internal) Generate and manage JSON schemas for himitsu config files.
    #[command(hide = true)]
    Schema(schema::SchemaArgs),

    /// Generate SOPS-encrypted output files from env definitions in project config.
    Generate(generate::GenerateArgs),

    /// Export secrets matching a glob pattern as a SOPS-encrypted file.
    Export(export::ExportArgs),

    /// (Legacy) Generate typed config code from secrets. See 'generate' for canonical output.
    #[command(hide = true)]
    Codegen(codegen::CodegenArgs),

    /// Run a command with secrets injected as environment variables.
    Exec(exec::ExecArgs),

    /// Run git commands inside a store checkout (or all stores with --all).
    Git(git::GitArgs),

    /// Verify store checkouts are up to date with their remotes.
    Check(check::CheckArgs),

    /// Show the himitsu documentation (renders README).
    Docs,

    /// Print version information.
    Version,

    /// Generate shell completion scripts.
    Completions(completions::CompletionsArgs),

    /// (Internal) Print secret paths for shell completion. Used by the
    /// generated completion scripts — not intended for direct use.
    #[command(name = "__complete-paths", hide = true)]
    CompletePaths(completions::CompletePathsArgs),

    // ── Hidden commands (not yet implemented) ─────────────────────
    /// Share secrets with external recipients.
    #[command(hide = true)]
    Share(share::ShareArgs),

    /// Manage the incoming secret inbox.
    #[command(hide = true)]
    Inbox(inbox::InboxArgs),

    /// Import secrets from external stores (1Password or SOPS).
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
        let is_docs = matches!(&command, Command::Docs);
        let is_completions = matches!(&command, Command::Completions(_));
        let is_complete_paths = matches!(&command, Command::CompletePaths(_));

        if !is_init
            && !is_git
            && !is_version
            && !is_docs
            && !is_completions
            && !is_complete_paths
            && !crate::crypto::keystore::is_initialized(&data_dir)
        {
            eprintln!("First run — initializing himitsu...");
            let ctx = Context {
                data_dir: data_dir.clone(),
                state_dir: state_dir.clone(),
                store: PathBuf::new(),
                recipients_path: None,
                key_provider: crate::config::KeyProvider::default(),
            };
            init::run(
                init::InitArgs {
                    json: false,
                    name: None,
                    url: None,
                    home: None,
                    key_provider: None,
                    no_tui: true,
                    project: None,
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
                | Command::Read(_)
                | Command::Write(_)
                | Command::Rekey(_)
                | Command::Recipient(_)
                | Command::Join(_)
                | Command::Schema(_)
                | Command::Generate(_)
                | Command::Export(_)
                | Command::Codegen(_)
                | Command::Exec(_)
                | Command::Import(_)
                | Command::Tag(_)
        );

        let store = if let Some(ref p) = store_override {
            p.clone()
        } else if needs_store {
            crate::config::resolve_store(None)?
        } else if is_complete_paths {
            // Completion helper: best-effort store resolution, never errors.
            // If nothing resolves we fall back to enumerating stores_dir in
            // `completions::run_complete_paths`.
            crate::config::resolve_store(None).unwrap_or_default()
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

        // Idempotent: ensure the resolved store is a git repo. Handles stores
        // created by `init --name` which sets up the directory layout but
        // didn't previously run `git init`.
        if !store.as_os_str().is_empty() && init::store_exists(&store) {
            init::ensure_git_repo(&store);
            // Also ensure the slug-managed store has a default `origin` so
            // auto-commits actually push somewhere. Catches stores created
            // before the dispatcher started auto-committing.
            init::ensure_default_origin(&store, &state_dir.join("stores"));
        }

        let recipients_path = load_recipients_path_override(&store);
        let key_provider = crate::config::Config::load(&crate::config::config_path())
            .map(|c| c.key_provider)
            .unwrap_or_default();
        let ctx = Context {
            data_dir,
            state_dir,
            store,
            recipients_path,
            key_provider,
        };

        // Pre-dispatch: when `auto_pull` is on, fetch + fast-forward the
        // resolved store so reads see latest state and writes can't fast-fail
        // a push because of a remote-side commit. Only fires for store-
        // touching commands; init/version/completions etc. skip.
        if !ctx.store.as_os_str().is_empty()
            && init::store_exists(&ctx.store)
            && crate::config::auto_pull_enabled()
        {
            ctx.pull_if_remote();
        }

        // Snapshot the mutation message and `--no-push` opt-out *before*
        // dispatching, since `command` is moved into the match below.
        //
        // The append-only invariant — every mutating command must leave the
        // store with a clean working tree — is enforced post-dispatch by
        // committing on both success and failure paths. See `mutation_message`
        // for the set of commands considered mutations.
        let mutation_msg = mutation_message(&command);
        let no_push = match &command {
            Command::Set(a) => a.no_push,
            Command::Write(a) => a.no_push,
            Command::Import(a) => a.no_push,
            Command::Join(a) => a.no_push,
            _ => false,
        };

        let result = match command {
            Command::Init(args) => init::run(args, &ctx),
            Command::Set(args) => set::run(args, &ctx),
            Command::Get(args) => get::run(args, &ctx),
            Command::Read(args) => read::run(args, &ctx),
            Command::Write(args) => write::run(args, &ctx),
            Command::Ls(args) => ls::run(args, &ctx),
            Command::Rekey(args) => rekey::run(args, &ctx),
            Command::Encrypt(args) => encrypt::run(args, &ctx),
            Command::Decrypt(args) => decrypt::run(args, &ctx),
            Command::Join(args) => join::run(args, &ctx),
            Command::Sync(args) => sync::run(args, &ctx),
            Command::Search(args) => search::run(args, &ctx),
            Command::Recipient(args) => recipient::run(args, &ctx),
            Command::Remote(args) => remote::run(args, &ctx),
            Command::Context(args) => context::run(args, &ctx),

            Command::Schema(args) => schema::run(args, &ctx),
            Command::Generate(args) => generate::run(args, &ctx),
            Command::Export(args) => export::run(args, &ctx),
            Command::Codegen(args) => codegen::run(args, &ctx),
            Command::Exec(args) => exec::run(args, &ctx),
            Command::Git(args) => git::run(args, &ctx),
            Command::Check(args) => check::run(args, &ctx),
            Command::Docs => docs::run(),
            Command::Version => {
                println!("{}", crate::build_info::VERSION_LINE);
                Ok(())
            }
            Command::Completions(args) => completions::run(args),
            Command::CompletePaths(args) => completions::run_complete_paths(args, &ctx),
            Command::Share(args) => share::run(args, &ctx),
            Command::Inbox(args) => inbox::run(args, &ctx),
            Command::Import(args) => import::run(args, &ctx),
            Command::Tag(args) => tag::run(args, &ctx),
        };

        // Post-dispatch: enforce the append-only invariant for mutating
        // commands. Always commit (success OR failure) so `git status` is
        // never left dirty; on failure prefix the message with `FAILED:` and
        // append the error so the history records the partial state. Push
        // only on success, and only when the user did not opt out.
        if let Some(msg) = mutation_msg {
            let final_msg = match &result {
                Ok(_) => format!("himitsu: {msg}"),
                Err(e) => format!("himitsu: FAILED: {msg}: {e}"),
            };
            let committed = ctx.commit(&final_msg);
            if result.is_ok() && committed && !no_push {
                ctx.push();
            }
        }

        result
    }

    fn launch_tui() -> Result<()> {
        if !io::stdout().is_terminal() {
            return Err(HimitsuError::NotSupported(
                "stdout is not a terminal — run a subcommand (try `himitsu --help`).".into(),
            ));
        }

        let data_dir = crate::config::data_dir();
        let state_dir = crate::config::state_dir();

        // The dashboard is read-only: if no store resolves (none configured,
        // ambiguous, etc.) we still open and render an empty state rather
        // than erroring out.
        let store = crate::config::resolve_store(None).unwrap_or_default();
        let recipients_path = load_recipients_path_override(&store);
        let key_provider = crate::config::Config::load(&crate::config::config_path())
            .map(|c| c.key_provider)
            .unwrap_or_default();

        let ctx = Context {
            data_dir,
            state_dir,
            store,
            recipients_path,
            key_provider,
        };
        crate::tui::run(&ctx)
    }
}

/// Returns the human-readable mutation message for commands that change the
/// store on disk, or `None` for read-only commands and commands that touch
/// state outside the store (project/global config, generated output files).
///
/// Commands listed here participate in the append-only commit dispatcher:
/// the store is committed after every invocation, so `git status` is never
/// left dirty. The message becomes the commit subject (with a `FAILED:`
/// prefix on the error path).
///
/// Intentionally excluded:
///   * `Sync` — already a git operation; user-driven pull/rekey.
///   * `Init` — has its own bootstrap commit logic.
///   * `Generate`, `Export`, `Codegen` — write outside the store.
///   * `Remote`, `Context` — mutate global config, not the store repo.
fn mutation_message(cmd: &Command) -> Option<String> {
    match cmd {
        Command::Set(a) => Some(format!("set {}", a.path)),
        Command::Write(a) => Some(format!("write {}", a.path)),
        Command::Rekey(a) => Some(match &a.path {
            Some(p) => format!("rekey {p}"),
            None => "rekey".to_string(),
        }),
        Command::Encrypt(_) => Some("rekey (encrypt)".to_string()),
        Command::Import(_) => Some("import".to_string()),
        Command::Join(_) => Some("join".to_string()),
        Command::Recipient(a) => {
            recipient_subcommand_label(&a.command).map(|label| format!("recipient {label}"))
        }
        Command::Schema(a) => match &a.command {
            schema::SchemaCommand::Refresh => Some("schema refresh".to_string()),
            _ => None,
        },
        Command::Tag(a) => match &a.action {
            tag::TagAction::Add { .. } => Some(format!("tag add {}", a.path)),
            tag::TagAction::Rm { .. } => Some(format!("tag rm {}", a.path)),
            tag::TagAction::List => None,
        },
        _ => None,
    }
}

/// Short label for mutating recipient subcommands. `Show` and `Ls` are
/// read-only and return `None` so the dispatcher skips the commit.
fn recipient_subcommand_label(cmd: &recipient::RecipientCommand) -> Option<String> {
    match cmd {
        recipient::RecipientCommand::Add { name, .. } => Some(format!("add {name}")),
        recipient::RecipientCommand::Rm { name } => Some(format!("rm {name}")),
        recipient::RecipientCommand::Show { .. } | recipient::RecipientCommand::Ls => None,
    }
}

fn command_uses_explicit_path_store(command: &Command) -> bool {
    matches!(
        command,
        Command::Set(_)
            | Command::Get(_)
            | Command::Read(_)
            | Command::Write(_)
            | Command::Ls(_)
            | Command::Rekey(_)
            | Command::Encrypt(_)
            | Command::Decrypt(_)
            | Command::Recipient(_)
            | Command::Join(_)
            | Command::Schema(_)
            | Command::Generate(_)
            | Command::Export(_)
            | Command::Codegen(_)
            | Command::Exec(_)
            | Command::Import(_)
            | Command::Tag(_)
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
/// 1. Store-internal `.himitsu/config.yaml` → `recipients_path`
/// 2. Project config (walked up from CWD) → `store.recipients_path`
/// 3. `None` → use default `.himitsu/recipients/` layout
fn load_recipients_path_override(store: &std::path::Path) -> Option<String> {
    if store.as_os_str().is_empty() {
        return None;
    }

    // 1. Check store-internal config
    if let Ok(cfg) = crate::remote::store::load_store_config(store) {
        if cfg.recipients_path.is_some() {
            return cfg.recipients_path;
        }
    }

    // 2. Check project config
    if let Some((project_cfg, _)) = crate::config::load_project_config() {
        if project_cfg.recipients_path.is_some() {
            return project_cfg.recipients_path;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(argv: &[&str]) -> Command {
        let cli = Cli::try_parse_from(argv).expect("argv parses");
        cli.command.expect("subcommand present")
    }

    #[test]
    fn mutation_message_set_includes_path() {
        let cmd = parse(&["himitsu", "set", "prod/API_KEY", "value"]);
        assert_eq!(mutation_message(&cmd).as_deref(), Some("set prod/API_KEY"));
    }

    #[test]
    fn mutation_message_write_includes_path() {
        let cmd = parse(&["himitsu", "write", "prod/TOKEN", "v"]);
        assert_eq!(mutation_message(&cmd).as_deref(), Some("write prod/TOKEN"));
    }

    #[test]
    fn mutation_message_rekey_with_and_without_path() {
        let cmd = parse(&["himitsu", "rekey"]);
        assert_eq!(mutation_message(&cmd).as_deref(), Some("rekey"));

        let cmd = parse(&["himitsu", "rekey", "prod"]);
        assert_eq!(mutation_message(&cmd).as_deref(), Some("rekey prod"));
    }

    #[test]
    fn mutation_message_recipient_add_and_rm() {
        let cmd = parse(&["himitsu", "recipient", "add", "ops/alice", "--self"]);
        assert_eq!(
            mutation_message(&cmd).as_deref(),
            Some("recipient add ops/alice")
        );

        let cmd = parse(&["himitsu", "recipient", "rm", "ops/alice"]);
        assert_eq!(
            mutation_message(&cmd).as_deref(),
            Some("recipient rm ops/alice")
        );
    }

    #[test]
    fn mutation_message_recipient_show_and_ls_are_readonly() {
        let cmd = parse(&["himitsu", "recipient", "show", "ops/alice"]);
        assert_eq!(mutation_message(&cmd), None);

        let cmd = parse(&["himitsu", "recipient", "ls"]);
        assert_eq!(mutation_message(&cmd), None);
    }

    #[test]
    fn mutation_message_schema_refresh_only() {
        let cmd = parse(&["himitsu", "schema", "refresh"]);
        assert_eq!(mutation_message(&cmd).as_deref(), Some("schema refresh"));

        let cmd = parse(&["himitsu", "schema", "list"]);
        assert_eq!(mutation_message(&cmd), None);
    }

    #[test]
    fn mutation_message_readonly_commands_return_none() {
        for argv in [
            vec!["himitsu", "get", "prod/API_KEY"],
            vec!["himitsu", "read", "prod/API_KEY"],
            vec!["himitsu", "ls"],
            vec!["himitsu", "search", "api"],
            vec!["himitsu", "version"],
        ] {
            let cmd = parse(&argv);
            assert_eq!(
                mutation_message(&cmd),
                None,
                "expected {argv:?} to be read-only"
            );
        }
    }

    #[test]
    fn mutation_message_outside_store_commands_return_none() {
        // Generate/Export/Codegen write outside the store; Remote/Context
        // mutate global config. None should trigger a store commit.
        let cmd = parse(&["himitsu", "remote", "list"]);
        assert_eq!(mutation_message(&cmd), None);

        let cmd = parse(&["himitsu", "context", "clear"]);
        assert_eq!(mutation_message(&cmd), None);
    }

    // ── pull_if_remote ──────────────────────────────────────────────────

    /// Helper: build two linked git repos so we can test fetch+ff-only pull
    /// against a real remote without touching the network.
    ///
    /// Returns `(remote_path, local_path)`. The remote has one commit; the
    /// local was cloned from it and tracks `origin/main`.
    fn make_linked_repos(root: &Path) -> (PathBuf, PathBuf) {
        let remote = root.join("remote.git");
        let local = root.join("local");

        // Bare remote.
        crate::git::run(
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
            root,
        )
        .unwrap();

        // Working repo + initial commit.
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        crate::git::run(&["init", "-b", "main"], &work).unwrap();
        crate::git::run(&["config", "user.email", "t@t"], &work).unwrap();
        crate::git::run(&["config", "user.name", "t"], &work).unwrap();
        std::fs::write(work.join("seed.txt"), "seed").unwrap();
        crate::git::run(&["add", "."], &work).unwrap();
        crate::git::run(&["commit", "-m", "seed"], &work).unwrap();
        crate::git::run(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            &work,
        )
        .unwrap();
        crate::git::run(&["push", "-u", "origin", "main"], &work).unwrap();

        // The "local" store is a fresh clone — has tracking branch.
        crate::git::run(
            &["clone", remote.to_str().unwrap(), local.to_str().unwrap()],
            root,
        )
        .unwrap();
        crate::git::run(&["config", "user.email", "t@t"], &local).unwrap();
        crate::git::run(&["config", "user.name", "t"], &local).unwrap();

        (remote, local)
    }

    fn ctx_for(store: &Path) -> Context {
        let tmp = tempfile::tempdir().unwrap().keep();
        Context {
            data_dir: tmp.clone(),
            state_dir: tmp,
            store: store.to_path_buf(),
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        }
    }

    #[test]
    fn pull_if_remote_fast_forwards_when_remote_advances() {
        let root = tempfile::tempdir().unwrap();
        let (_remote, local) = make_linked_repos(root.path());

        // Advance the remote by pushing a new commit from the work clone.
        let work = root.path().join("work");
        std::fs::write(work.join("new.txt"), "x").unwrap();
        crate::git::run(&["add", "."], &work).unwrap();
        crate::git::run(&["commit", "-m", "remote-side"], &work).unwrap();
        crate::git::run(&["push"], &work).unwrap();

        // Local doesn't have the new commit yet.
        assert!(!local.join("new.txt").exists());

        ctx_for(&local).pull_if_remote();

        // After auto-pull, local has fast-forwarded.
        assert!(
            local.join("new.txt").exists(),
            "auto-pull should fast-forward and bring in remote commit"
        );
    }

    #[test]
    fn pull_if_remote_noop_without_remote() {
        // Reproduces the user's broken store state: a local repo with no
        // origin must not panic, must not produce any side effect.
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("orphan");
        std::fs::create_dir_all(&store).unwrap();
        crate::git::init(&store).unwrap();

        ctx_for(&store).pull_if_remote();
        // No assertion to make beyond "did not panic" — the absence of a
        // remote means there's nothing to do.
    }

    #[test]
    fn pull_if_remote_noop_outside_git_repo() {
        let tmp = tempfile::tempdir().unwrap();
        ctx_for(tmp.path()).pull_if_remote();
    }

    // ── submodule commit + push ─────────────────────────────────────────

    /// Build parent+submodule both tracking bare remotes. Returns
    /// `(local_store, submodule_path, sub_bare_remote)` where `local_store`
    /// is a fresh clone of the parent with the submodule initialized.
    fn make_linked_repos_with_submodule(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let allow = "-c";
        let allow_val = "protocol.file.allow=always";

        let parent_remote = root.join("parent.git");
        let sub_remote = root.join("sub.git");
        crate::git::run(
            &[
                "init",
                "--bare",
                "-b",
                "main",
                parent_remote.to_str().unwrap(),
            ],
            root,
        )
        .unwrap();
        crate::git::run(
            &["init", "--bare", "-b", "main", sub_remote.to_str().unwrap()],
            root,
        )
        .unwrap();

        // Seed the submodule remote with one commit.
        let sub_work = root.join("sub-work");
        std::fs::create_dir_all(&sub_work).unwrap();
        crate::git::run(&["init", "-b", "main"], &sub_work).unwrap();
        crate::git::run(&["config", "user.email", "t@t"], &sub_work).unwrap();
        crate::git::run(&["config", "user.name", "t"], &sub_work).unwrap();
        std::fs::write(sub_work.join("seed.txt"), "s").unwrap();
        crate::git::run(&["add", "."], &sub_work).unwrap();
        crate::git::run(&["commit", "-m", "seed-sub"], &sub_work).unwrap();
        crate::git::run(
            &["remote", "add", "origin", sub_remote.to_str().unwrap()],
            &sub_work,
        )
        .unwrap();
        crate::git::run(&["push", "-u", "origin", "main"], &sub_work).unwrap();

        // Seed the parent remote and wire the submodule under `.himitsu/sub`.
        let parent_work = root.join("parent-work");
        std::fs::create_dir_all(&parent_work).unwrap();
        crate::git::run(&["init", "-b", "main"], &parent_work).unwrap();
        crate::git::run(&["config", "user.email", "t@t"], &parent_work).unwrap();
        crate::git::run(&["config", "user.name", "t"], &parent_work).unwrap();
        std::fs::create_dir_all(parent_work.join(".himitsu")).unwrap();
        std::fs::write(parent_work.join(".himitsu/.keep"), "").unwrap();
        crate::git::run(&["add", "."], &parent_work).unwrap();
        crate::git::run(&["commit", "-m", "seed-parent"], &parent_work).unwrap();
        crate::git::run(
            &[
                allow,
                allow_val,
                "submodule",
                "add",
                sub_remote.to_str().unwrap(),
                ".himitsu/sub",
            ],
            &parent_work,
        )
        .unwrap();
        crate::git::run(&["commit", "-m", "add-sub"], &parent_work).unwrap();
        crate::git::run(
            &["remote", "add", "origin", parent_remote.to_str().unwrap()],
            &parent_work,
        )
        .unwrap();
        crate::git::run(&["push", "-u", "origin", "main"], &parent_work).unwrap();

        // Fresh clone with recursed submodules — this is the "local store".
        let local = root.join("local");
        crate::git::run(
            &[
                allow,
                allow_val,
                "clone",
                "--recurse-submodules",
                parent_remote.to_str().unwrap(),
                local.to_str().unwrap(),
            ],
            root,
        )
        .unwrap();
        crate::git::run(&["config", "user.email", "t@t"], &local).unwrap();
        crate::git::run(&["config", "user.name", "t"], &local).unwrap();
        let sub = local.join(".himitsu/sub");
        crate::git::run(&["config", "user.email", "t@t"], &sub).unwrap();
        crate::git::run(&["config", "user.name", "t"], &sub).unwrap();

        (local, sub, sub_remote)
    }

    #[test]
    fn commit_stages_and_commits_inside_dirty_submodule() {
        let root = tempfile::tempdir().unwrap();
        let (local, sub_path, _sub_remote) = make_linked_repos_with_submodule(root.path());

        // Simulate a mutation: a new encrypted secret inside the submodule.
        std::fs::create_dir_all(sub_path.join("env")).unwrap();
        std::fs::write(sub_path.join("env/FOO.age"), "ciphertext").unwrap();

        let parent_head_before = crate::git::run(&["rev-parse", "HEAD"], &local).unwrap();
        let sub_head_before = crate::git::run(&["rev-parse", "HEAD"], &sub_path).unwrap();

        let committed = ctx_for(&local).commit("himitsu: set env/FOO");
        assert!(committed, "parent should commit the pointer bump");

        let sub_head_after = crate::git::run(&["rev-parse", "HEAD"], &sub_path).unwrap();
        let parent_head_after = crate::git::run(&["rev-parse", "HEAD"], &local).unwrap();

        assert_ne!(
            sub_head_before.trim(),
            sub_head_after.trim(),
            "submodule HEAD should advance so the pointer bump is real"
        );
        assert_ne!(
            parent_head_before.trim(),
            parent_head_after.trim(),
            "parent HEAD should advance with the pointer-bump commit"
        );
    }

    #[test]
    fn push_propagates_submodule_commits_to_sub_remote() {
        let root = tempfile::tempdir().unwrap();
        let (local, sub_path, sub_remote) = make_linked_repos_with_submodule(root.path());

        std::fs::create_dir_all(sub_path.join("env")).unwrap();
        std::fs::write(sub_path.join("env/FOO.age"), "ciphertext").unwrap();

        let ctx = ctx_for(&local);
        ctx.commit("himitsu: set env/FOO");
        ctx.push();

        // Verify by cloning the sub remote fresh and checking the file lands.
        let verify = root.path().join("verify-sub");
        crate::git::run(
            &[
                "-c",
                "protocol.file.allow=always",
                "clone",
                sub_remote.to_str().unwrap(),
                verify.to_str().unwrap(),
            ],
            root.path(),
        )
        .unwrap();
        assert!(
            verify.join("env/FOO.age").exists(),
            "sub remote should have the secret after push"
        );
    }
}
