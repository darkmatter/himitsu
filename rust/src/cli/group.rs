use clap::{Args, Subcommand};

/// Manage recipient groups.
#[derive(Debug, Args)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub command: GroupCommand,
}

#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Add a new group.
    Add {
        /// Name of the group to create.
        name: String,
    },

    /// Remove a group.
    Rm {
        /// Name of the group to remove.
        name: String,
    },

    /// List all groups.
    Ls,
}

pub fn run(_args: GroupArgs) {
    eprintln!("himitsu group: not yet implemented");
    std::process::exit(1);
}
