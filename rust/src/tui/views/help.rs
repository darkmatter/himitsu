//! Modal help overlay: shows keybindings for the currently active view.
//!
//! Opened by `?` from the app router; dismissed by `?` or `Esc`. The overlay
//! is a centered popup rendered on top of whatever view is underneath — the
//! underlying view keeps drawing normally so context is preserved.
//!
//! Help content is not owned by this module — each view exposes its own
//! `help_entries()` + `help_title()` associated functions, and the router
//! plugs them in when the overlay is opened.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};

use crate::tui::theme;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

/// Outcome of handling a key while the help overlay is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpAction {
    /// Keep the overlay open.
    None,
    /// Dismiss the overlay.
    Close,
}

/// Modal help popup bound to a static set of `(key, description)` rows.
pub struct HelpView {
    entries: &'static [(&'static str, &'static str)],
    title: &'static str,
}

impl HelpView {
    pub fn new(entries: &'static [(&'static str, &'static str)], title: &'static str) -> Self {
        Self { entries, title }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> HelpAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') => HelpAction::Close,
            _ => HelpAction::None,
        }
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(60, 50, frame.area());

        // Clear the area first so underlying content is blanked out.
        frame.render_widget(Clear, area);

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            format!(" {} ", self.title),
            Style::default()
                .fg(theme::border_label())
                .add_modifier(Modifier::BOLD),
        ));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Reserve the bottom row for the active theme indicator so users can
        // tell which palette is loaded (especially relevant when `theme: random`
        // picked something for them).
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        let key_w = self.entries.iter().map(|(k, _)| k.len()).max().unwrap_or(0);

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|(k, desc)| {
                let line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<key_w$}", k, key_w = key_w),
                        Style::default()
                            .fg(theme::accent())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::raw(*desc),
                ]);
                ListItem::new(line)
            })
            .collect();

        frame.render_widget(List::new(items), rows[0]);

        let footer = Line::from(vec![
            Span::styled("theme ", Style::default().fg(theme::footer_text())),
            Span::styled(
                theme::current_theme_name(),
                Style::default()
                    .fg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]);
        frame.render_widget(Paragraph::new(footer).alignment(Alignment::Right), rows[1]);
    }
}

/// Build a centered rectangle that is `percent_x` wide and `percent_y` tall
/// relative to `area`.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    const SAMPLE: &[(&str, &str)] = &[("?", "help"), ("q", "quit")];

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn esc_closes_overlay() {
        let mut view = HelpView::new(SAMPLE, "help");
        assert_eq!(view.on_key(press(KeyCode::Esc)), HelpAction::Close);
    }

    #[test]
    fn question_mark_closes_overlay() {
        let mut view = HelpView::new(SAMPLE, "help");
        assert_eq!(view.on_key(press(KeyCode::Char('?'))), HelpAction::Close);
    }

    #[test]
    fn other_keys_keep_overlay_open() {
        let mut view = HelpView::new(SAMPLE, "help");
        assert_eq!(view.on_key(press(KeyCode::Char('q'))), HelpAction::None);
        assert_eq!(view.on_key(press(KeyCode::Down)), HelpAction::None);
        assert_eq!(view.on_key(press(KeyCode::Enter)), HelpAction::None);
    }
}
