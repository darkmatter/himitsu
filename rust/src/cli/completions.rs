use std::io;

use clap::{Args, CommandFactory};
use clap_complete::{generate, Shell};

use crate::error::Result;

/// Generate shell completion script and print it to stdout.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Target shell for completion output.
    pub shell: Shell,
}

pub fn run(args: CompletionsArgs) -> Result<()> {
    let mut cmd = super::Cli::command();
    generate(args.shell, &mut cmd, "himitsu", &mut io::stdout());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_complete::Shell;

    #[test]
    fn bash_completions_are_generated() {
        let mut cmd = super::super::Cli::command();
        let mut buf: Vec<u8> = Vec::new();
        generate(Shell::Bash, &mut cmd, "himitsu", &mut buf);
        let text = String::from_utf8(buf).unwrap();
        assert!(!text.is_empty());
        assert!(text.contains("himitsu"));
    }
}
