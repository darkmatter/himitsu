mod cli;
pub mod config;
pub mod crypto;
pub mod error;
pub mod git;
pub mod index;
pub mod keyring;
pub mod remote;

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

    if let Err(e) = cli.run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
