mod build_info;
mod cli;
pub mod config;
pub mod crypto;
pub mod error;
pub mod git;

pub mod keyring;
pub mod proto;
pub mod reference;
pub mod remote;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::Cli;

/// Himitsu is an AGE-based secrets manager that enables the sharing of secrets with
/// other users. Secrets are stored in a local git-backed store. You may have as many
/// stores as you want and still interact wiht them as if you had a single store since
/// they are referenced using a path-based format. This enables sharing of secrets
/// between multiple users and teams.
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
