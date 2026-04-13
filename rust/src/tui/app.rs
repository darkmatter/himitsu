//! Top-level TUI router for the dashboard loop.
//!
//! The init wizard has its own standalone event loop in [`crate::tui::run_init_flow`];
//! this [`App`] wraps the [`DashboardView`] once himitsu is initialized.

use crossterm::event::KeyEvent;
use ratatui::Frame;

use crate::cli::Context;
use crate::tui::views::dashboard::DashboardView;

pub struct App {
    pub should_quit: bool,
    dashboard: DashboardView,
}

impl App {
    pub fn new(ctx: &Context) -> Self {
        Self {
            should_quit: false,
            dashboard: DashboardView::new(ctx),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.dashboard.on_key(key);
        if self.dashboard.should_quit {
            self.should_quit = true;
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        self.dashboard.draw(frame);
    }
}
