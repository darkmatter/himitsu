use clap::Args;

/// List environments or secrets within an environment.
#[derive(Debug, Args)]
pub struct LsArgs {
    /// Environment to list secrets for. If omitted, lists all environments.
    pub env: Option<String>,
}

pub fn run(_args: LsArgs) {
    eprintln!("himitsu ls: not yet implemented");
    std::process::exit(1);
}
