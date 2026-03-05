use clap::{Args, Subcommand};

/// Manage git-backed remotes.
#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommand,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Add a remote repository.
    Add {
        /// Repository reference (e.g. org/repo).
        repo: String,
    },

    /// Push local changes to the remote.
    Push,

    /// Pull latest changes from the remote.
    Pull,

    /// Show the status of the remote.
    Status,
}

pub fn run(_args: RemoteArgs) {
    eprintln!("himitsu remote: not yet implemented");
    std::process::exit(1);
}
