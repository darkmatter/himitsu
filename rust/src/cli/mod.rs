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

use clap::{Parser, Subcommand};

/// Himitsu - age-based secrets management with transport-agnostic sharing.
#[derive(Debug, Parser)]
#[command(name = "himitsu", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new himitsu store.
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
    pub fn run(self) {
        match self.command {
            Command::Init(args) => init::run(args),
            Command::Set(args) => set::run(args),
            Command::Get(args) => get::run(args),
            Command::Ls(args) => ls::run(args),
            Command::Encrypt(args) => encrypt::run(args),
            Command::Decrypt(args) => decrypt::run(args),
            Command::Sync(args) => sync::run(args),
            Command::Search(args) => search::run(args),
            Command::Recipient(args) => recipient::run(args),
            Command::Group(args) => group::run(args),
            Command::Remote(args) => remote::run(args),
            Command::Share(args) => share::run(args),
            Command::Inbox(args) => inbox::run(args),
            Command::Schema(args) => schema::run(args),
            Command::Codegen(args) => codegen::run(args),
            Command::Import(args) => import::run(args),
        }
    }
}
