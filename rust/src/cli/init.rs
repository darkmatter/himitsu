use clap::Args;

/// Initialize a new himitsu store at ~/.himitsu.
#[derive(Debug, Args)]
pub struct InitArgs {}

pub fn run(_args: InitArgs) {
    eprintln!("himitsu init: not yet implemented");
    std::process::exit(1);
}
