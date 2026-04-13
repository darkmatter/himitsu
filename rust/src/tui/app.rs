//! Application state and top-level render/input dispatch.
//!
//! Individual views live alongside this file; for now the scaffold only
//! implements a placeholder screen that US-003/US-004/etc. will flesh out.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

#[derive(Debug, Default)]
pub struct App {
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => self.should_quit = true,
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.should_quit = true,
            _ => {}
        }
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        let header = Paragraph::new(Span::styled(
            " himitsu ",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(header, layout[0]);

        let body = Paragraph::new(vec![
            Line::from(""),
            Line::from("  ratatui scaffold"),
            Line::from(""),
            Line::from("  Views land in US-003..US-007."),
        ])
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(body, layout[1]);

        let footer = Paragraph::new(Span::raw("q quit  ctrl-c quit")).alignment(Alignment::Left);
        frame.render_widget(footer, layout[2]);
    }
}
