use clap::{Args, Subcommand};

/// Import secrets from external stores.
#[derive(Debug, Args)]
pub struct ImportArgs {
    #[command(subcommand)]
    pub command: ImportCommand,
}

#[derive(Debug, Subcommand)]
pub enum ImportCommand {
    /// Import from a SOPS-encrypted file.
    Sops {
        /// Path to the SOPS file.
        file: String,

        /// Target environment.
        #[arg(long)]
        env: String,

        /// Overwrite existing secrets.
        #[arg(long)]
        overwrite: bool,
    },

    /// Import from 1Password.
    Op {
        /// 1Password reference (e.g. op://vault/item/field).
        reference: String,

        /// Target environment.
        #[arg(long)]
        env: String,

        /// Secret key name (required for single field import).
        #[arg(long)]
        key: Option<String>,
    },
}

pub fn run(_args: ImportArgs) {
    eprintln!("himitsu import: not yet implemented");
    std::process::exit(1);
}
