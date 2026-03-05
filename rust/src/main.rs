mod cli;
mod error;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::Cli;

fn main() {
    let cli = Cli::parse();

    let filter = match cli.verbose {
        0 => EnvFilter::from_default_env(),
        1 => EnvFilter::new("himitsu=debug"),
        _ => EnvFilter::new("himitsu=trace"),
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();

    cli.run();
}
