use clap::Args;

/// Get a secret value.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// Target environment (e.g. prod, dev).
    pub env: String,

    /// Secret key name.
    pub key: String,
}

pub fn run(_args: GetArgs) {
    eprintln!("himitsu get: not yet implemented");
    std::process::exit(1);
}
