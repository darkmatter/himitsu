use clap::Args;

/// Decrypt secrets (not supported - secrets are never stored in plaintext).
#[derive(Debug, Args)]
pub struct DecryptArgs {
    /// Target environment.
    pub env: Option<String>,
}

pub fn run(_args: DecryptArgs) {
    eprintln!("himitsu decrypt: not yet implemented");
    std::process::exit(1);
}
