use clap::{Args, Subcommand};

/// Generate and manage JSON schemas for himitsu config files.
#[derive(Debug, Args)]
pub struct SchemaArgs {
    #[command(subcommand)]
    pub command: SchemaCommand,
}

#[derive(Debug, Subcommand)]
pub enum SchemaCommand {
    /// Refresh dynamic schemas from current state.
    Refresh,
}

pub fn run(_args: SchemaArgs) {
    eprintln!("himitsu schema: not yet implemented");
    std::process::exit(1);
}
