use clap::{Args, Subcommand};

/// Manage the incoming secret inbox.
#[derive(Debug, Args)]
pub struct InboxArgs {
    #[command(subcommand)]
    pub command: InboxCommand,
}

#[derive(Debug, Subcommand)]
pub enum InboxCommand {
    /// List pending envelopes.
    List {
        /// Transport to query (e.g. github, nostr).
        #[arg(long)]
        transport: Option<String>,
    },

    /// Accept an envelope and write the secret.
    Accept {
        /// Envelope ID to accept.
        id: String,
    },

    /// Reject an envelope without writing the secret.
    Reject {
        /// Envelope ID to reject.
        id: String,
    },
}

pub fn run(_args: InboxArgs) {
    eprintln!("himitsu inbox: not yet implemented");
    std::process::exit(1);
}
