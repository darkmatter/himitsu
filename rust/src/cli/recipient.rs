use clap::{Args, Subcommand};

/// Manage recipients.
#[derive(Debug, Args)]
pub struct RecipientArgs {
    #[command(subcommand)]
    pub command: RecipientCommand,
}

#[derive(Debug, Subcommand)]
pub enum RecipientCommand {
    /// Add a recipient.
    Add {
        /// Recipient name (e.g. laptop-a, alice).
        name: String,

        /// Add yourself as a recipient.
        #[arg(long = "self")]
        self_: bool,

        /// Explicit age public key.
        #[arg(long)]
        age_key: Option<String>,

        /// Group to add the recipient to.
        #[arg(long)]
        group: Option<String>,
    },

    /// Remove a recipient.
    Rm {
        /// Name of the recipient to remove.
        name: String,

        /// Group to remove the recipient from.
        #[arg(long)]
        group: Option<String>,
    },

    /// List recipients.
    Ls {
        /// Filter by group.
        #[arg(long)]
        group: Option<String>,
    },
}

pub fn run(_args: RecipientArgs) {
    eprintln!("himitsu recipient: not yet implemented");
    std::process::exit(1);
}
