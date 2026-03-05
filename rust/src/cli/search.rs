use clap::Args;

/// Search secrets across all remotes.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Search query to match against key names.
    pub query: String,

    /// Refresh the search index before querying.
    #[arg(long)]
    pub refresh: bool,
}

pub fn run(_args: SearchArgs) {
    eprintln!("himitsu search: not yet implemented");
    std::process::exit(1);
}
