//! Poll-based event loop.

use std::time::Duration;

use crossterm::event::{self, Event};

use crate::error::Result;
use crate::tui::app::App;
use crate::tui::terminal::Tui;

const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub fn run_loop(terminal: &mut Tui, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;

        if event::poll(POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                    app.on_key(key);
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}
