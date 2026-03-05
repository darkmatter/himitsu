mod cli;
mod error;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::Cli;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    cli.run();
}
