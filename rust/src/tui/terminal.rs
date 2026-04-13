//! Terminal setup/teardown with panic-safe restoration.

use std::io::{self, Stdout};

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::error::Result;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Drop guard that restores the terminal on scope exit.
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Enter raw mode + alternate screen and install a panic hook that restores
/// the terminal before the default hook runs. Returns a drop guard that
/// restores the terminal when dropped (normal-exit path).
pub fn install() -> Result<Guard> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    Ok(Guard)
}

pub fn new() -> Result<Tui> {
    let backend = CrosstermBackend::new(io::stdout());
    Ok(Terminal::new(backend)?)
}
