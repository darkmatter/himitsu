use clap::{Args, Subcommand};

/// Manage git-backed remotes.
#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    pub command: RemoteCommand,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Add a remote repository (clone existing or create new via --github).
    Add {
        /// Repository reference (e.g. org/repo). Optional when using --github.
        repo: Option<String>,

        /// Create a new GitHub repository.
        #[arg(long)]
        github: bool,

        /// GitHub org or user for repo creation.
        #[arg(long)]
        org: Option<String>,

        /// Repository name for repo creation.
        #[arg(long)]
        name: Option<String>,
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
