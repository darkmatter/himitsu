use clap::Args;

/// Re-encrypt all secrets for current recipients.
#[derive(Debug, Args)]
pub struct EncryptArgs {
    /// Target environment. If omitted, re-encrypts all environments.
    pub env: Option<String>,
}

pub fn run(_args: EncryptArgs) {
    eprintln!("himitsu encrypt: not yet implemented");
    std::process::exit(1);
}
