use clap::Args;

/// Sync encrypted secrets to configured project destinations.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// Target environment. If omitted, syncs all environments.
    pub env: Option<String>,
}

pub fn run(_args: SyncArgs) {
    eprintln!("himitsu sync: not yet implemented");
    std::process::exit(1);
}
