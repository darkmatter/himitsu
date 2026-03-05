use clap::Args;

/// Initialize a new himitsu store.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Path to the himitsu directory.
    #[arg(short, long)]
    pub dir: Option<String>,
}

pub fn run(_args: InitArgs) {
    eprintln!("himitsu init: not yet implemented");
    std::process::exit(1);
}
