use clap::Args;

/// Set a secret value.
#[derive(Debug, Args)]
pub struct SetArgs {
    /// Target environment (e.g. prod, dev).
    pub env: String,

    /// Secret key name.
    pub key: String,

    /// Secret value.
    pub value: String,
}

pub fn run(_args: SetArgs) {
    eprintln!("himitsu set: not yet implemented");
    std::process::exit(1);
}
