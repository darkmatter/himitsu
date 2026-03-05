pub mod codegen;
pub mod decrypt;
pub mod encrypt;
pub mod get;
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

use crate::error::Result;

/// Global options available to all subcommands.
pub struct Context {
    pub himitsu_home: PathBuf,
    pub remote_override: Option<String>,
}

/// Himitsu - age-based secrets management with transport-agnostic sharing.
#[derive(Debug, Parser)]
#[command(name = "himitsu", version, about, long_about = None)]
pub struct Cli {
    /// Target remote (org/repo). Overrides project binding and default remote.
    #[arg(short = 'r', long, global = true)]
    pub remote: Option<String>,

    /// Increase log verbosity (-v for debug, -vv for trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new himitsu store at ~/.himitsu.
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

    /// Sync encrypted secrets to project destinations.
    Sync(sync::SyncArgs),

    /// Search secrets across remotes.
    Search(search::SearchArgs),

    /// Manage recipients.
    Recipient(recipient::RecipientArgs),

    /// Manage recipient groups.
    Group(group::GroupArgs),

    /// Manage git-backed remotes.
    Remote(remote::RemoteArgs),

    /// Share secrets with external recipients.
    Share(share::ShareArgs),

    /// Manage the incoming secret inbox.
    Inbox(inbox::InboxArgs),

    /// Generate and manage JSON schemas.
    Schema(schema::SchemaArgs),

    /// Generate typed config code from secrets.
    Codegen(codegen::CodegenArgs),

    /// Import secrets from external stores.
    Import(import::ImportArgs),
}

impl Cli {
    pub fn run(self) -> Result<()> {
        let ctx = Context {
            himitsu_home: crate::config::himitsu_home(),
            remote_override: self.remote,
        };

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
            Command::Share(args) => share::run(args, &ctx),
            Command::Inbox(args) => inbox::run(args, &ctx),
            Command::Schema(args) => schema::run(args, &ctx),
            Command::Codegen(args) => codegen::run(args, &ctx),
            Command::Import(args) => import::run(args, &ctx),
        }
    }
}
