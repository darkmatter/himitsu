use clap::Args;

use super::Context;
use crate::error::{HimitsuError, Result};

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

pub fn run(_args: CodegenArgs, _ctx: &Context) -> Result<()> {
    Err(HimitsuError::NotSupported(
        "codegen is not yet implemented (planned for Phase 8)".into(),
    ))
}
