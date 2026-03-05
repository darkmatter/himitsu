use clap::Args;

/// Generate typed config code from secrets.
///
/// When run without arguments, reads language and output path from
/// the project's .himitsu.yaml codegen config.
#[derive(Debug, Args)]
pub struct CodegenArgs {
    /// Target language (typescript, golang, python). Overrides .himitsu.yaml.
    #[arg(long)]
    pub lang: Option<String>,

    /// Output file path. Overrides .himitsu.yaml.
    #[arg(long, short)]
    pub output: Option<String>,

    /// Environment to generate for.
    #[arg(long)]
    pub env: Option<String>,
}

pub fn run(_args: CodegenArgs) {
    eprintln!("himitsu codegen: not yet implemented");
    std::process::exit(1);
}
