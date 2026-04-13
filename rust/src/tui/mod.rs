//! In-process ratatui TUI for himitsu.
//!
//! Entry point: [`run`] installs raw mode + alternate screen, runs the event
//! loop, and always restores the terminal — including on panic.

mod app;
mod event;
mod terminal;

use crate::error::Result;

pub fn run() -> Result<()> {
    let _guard = terminal::install()?;
    let mut terminal = terminal::new()?;
    let mut app = app::App::new();
    event::run_loop(&mut terminal, &mut app)?;
    Ok(())
}
