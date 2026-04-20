use std::io::{self, IsTerminal, Write};

use crate::error::Result;

const README: &str = include_str!("../../../README.md");

/// Display the himitsu README in the terminal.
pub fn run() -> Result<()> {
    if io::stdout().is_terminal() {
        let skin = termimad::MadSkin::default_dark();
        let rendered = skin.term_text(README);
        // Pipe through $PAGER if available, otherwise print directly.
        if let Some(pager) = pager_cmd() {
            if pipe_to_pager(&pager, &rendered.to_string()).is_ok() {
                return Ok(());
            }
        }
        print!("{rendered}");
    } else {
        // Non-TTY: emit raw markdown for piping/redirection.
        io::stdout().write_all(README.as_bytes())?;
    }
    Ok(())
}

fn pager_cmd() -> Option<String> {
    std::env::var("PAGER").ok().or_else(|| {
        // Check if `less` is available.
        std::process::Command::new("less")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()
            .filter(|s| s.success())
            .map(|_| "less -R".to_string())
    })
}

fn pipe_to_pager(cmd: &str, content: &str) -> std::io::Result<()> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let (program, args) = parts.split_first().unwrap();
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // Ignore broken pipe — user may quit the pager early.
        let _ = stdin.write_all(content.as_bytes());
    }
    child.wait()?;
    Ok(())
}
