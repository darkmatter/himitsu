//! `himitsu prime` — emit an AGENTS.md snippet for AI agents.

use std::io::{self, Write};

use crate::error::Result;

const PRIME_TEMPLATE: &str = include_str!("../agents/prime.md");

/// Output an AGENTS.md-compatible snippet that teaches agents how to use
/// himitsu in this repository. Pipe-friendly: `himitsu prime >> AGENTS.md`.
pub fn run() -> Result<()> {
    io::stdout().write_all(PRIME_TEMPLATE.as_bytes())?;
    Ok(())
}
