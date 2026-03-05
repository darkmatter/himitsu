use clap::Args;

/// Generate typed config code from secrets.
#[derive(Debug, Args)]
pub struct CodegenArgs {
    /// Target language (typescript, golang, python).
    pub lang: String,

    /// Output file path.
    pub output: String,

    /// Environment to generate for.
    #[arg(long)]
    pub env: Option<String>,
}

pub fn run(_args: CodegenArgs) {
    eprintln!("himitsu codegen: not yet implemented");
    std::process::exit(1);
}
