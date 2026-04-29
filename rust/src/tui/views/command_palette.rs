//! Command palette overlay: a centered, fuzzy-filterable list of actions
//! reachable from the current view.
//!
//! Inspired by the VS Code command palette and `fzf`. Opened with `Ctrl+P`
//! from the search view, it consolidates discoverability of every command
//! the app exposes so individual footers don't need to enumerate them.
//!
//! The palette is intentionally dumb: it owns no state about the views it
//! launches. Each entry resolves to a [`Command`] variant, and the host
//! view (currently `SearchView`) maps the variant onto its existing key
//! handlers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::tui::theme;

/// One command exposed in the palette. Listed roughly in order of expected
/// usage frequency so the default selection lands on the common case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewSecret,
    SwitchStore,
    ToggleStoreColumn,
    Envs,
    Help,
    Quit,
}

impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Command::NewSecret => "new secret",
            Command::SwitchStore => "switch store",
            Command::ToggleStoreColumn => "toggle store column",
            Command::Envs => "browse envs",
            Command::Help => "show help",
            Command::Quit => "quit",
        }
    }

    pub fn shortcut(&self) -> &'static str {
        match self {
            Command::NewSecret => "ctrl-n",
            Command::SwitchStore => "ctrl-s",
            Command::ToggleStoreColumn => "",
            Command::Envs => "shift-e",
            Command::Help => "?",
            Command::Quit => "esc",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Command::NewSecret => "Create a new encrypted secret",
            Command::SwitchStore => "Pick a different remote / checkout",
            Command::ToggleStoreColumn => "Show/hide the STORE column in the results table",
            Command::Envs => "Browse env presets defined in himitsu.yaml",
            Command::Help => "Open the contextual key reference",
            Command::Quit => "Exit the TUI",
        }
    }
}

/// Outcome of a key press while the palette is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPaletteOutcome {
    /// Stay open; redraw with updated state.
    Pending,
    /// User pressed Esc / Ctrl+C without picking anything.
    Cancelled,
    /// User picked `command`.
    Selected(Command),
}

/// Palette state — the filter buffer and the selection cursor over the
/// filtered subset of [`COMMANDS`].
pub struct CommandPalette {
    query: String,
    filtered: Vec<Command>,
    list_state: ListState,
}

const COMMANDS: &[Command] = &[
    Command::NewSecret,
    Command::SwitchStore,
    Command::ToggleStoreColumn,
    Command::Envs,
    Command::Help,
    Command::Quit,
];

impl CommandPalette {
    pub fn new() -> Self {
        let mut palette = Self {
            query: String::new(),
            filtered: COMMANDS.to_vec(),
            list_state: ListState::default(),
        };
        palette.list_state.select(Some(0));
        palette
    }

    pub fn on_key(&mut self, key: KeyEvent) -> CommandPaletteOutcome {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                CommandPaletteOutcome::Cancelled
            }
            (KeyCode::Enter, _) => match self.selected_command() {
                Some(cmd) => CommandPaletteOutcome::Selected(cmd),
                None => CommandPaletteOutcome::Pending,
            },
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.move_selection(-1);
                CommandPaletteOutcome::Pending
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.move_selection(1);
                CommandPaletteOutcome::Pending
            }
            (KeyCode::Backspace, _) => {
                self.query.pop();
                self.refilter();
                CommandPaletteOutcome::Pending
            }
            (KeyCode::Char(c), modifiers)
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(c);
                self.refilter();
                CommandPaletteOutcome::Pending
            }
            _ => CommandPaletteOutcome::Pending,
        }
    }

    fn selected_command(&self) -> Option<Command> {
        self.list_state
            .selected()
            .and_then(|idx| self.filtered.get(idx).copied())
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.list_state.select(None);
            return;
        }
        let len = self.filtered.len() as isize;
        let current = self.list_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        self.list_state.select(Some(next));
    }

    /// Recompute [`Self::filtered`] for the current `query`. Uses a simple
    /// case-insensitive substring match across `label` + `description` —
    /// good enough for ~10 commands; revisit if the catalog grows large
    /// enough that a real fuzzy matcher matters.
    fn refilter(&mut self) {
        let q = self.query.to_ascii_lowercase();
        self.filtered = COMMANDS
            .iter()
            .copied()
            .filter(|cmd| {
                if q.is_empty() {
                    return true;
                }
                cmd.label().to_ascii_lowercase().contains(&q)
                    || cmd.description().to_ascii_lowercase().contains(&q)
                    || cmd.shortcut().to_ascii_lowercase().contains(&q)
            })
            .collect();
        if self.filtered.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = centered_rect(60, 50, frame.area());
        frame.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::border()))
            .title(Line::from(theme::brand_chip("command palette")));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // prompt
                Constraint::Length(1), // separator
                Constraint::Min(1),    // results
                Constraint::Length(1), // footer
            ])
            .split(inner);

        // Prompt row: > <query>█
        let prompt = Line::from(vec![
            Span::styled(" > ", Style::default().fg(theme::accent())),
            Span::raw(self.query.clone()),
            Span::styled("█", Style::default().fg(theme::accent())),
        ]);
        frame.render_widget(Paragraph::new(prompt), rows[0]);

        // Separator
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(theme::border()),
            ))),
            rows[1],
        );

        // Results
        if self.filtered.is_empty() {
            let msg = Paragraph::new(Line::from(Span::styled(
                "  no matching commands",
                Style::default().fg(theme::muted()),
            )));
            frame.render_widget(msg, rows[2]);
        } else {
            // Pad shortcuts to the widest entry so the descriptions line up.
            let shortcut_w = self
                .filtered
                .iter()
                .map(|c| c.shortcut().len())
                .max()
                .unwrap_or(0);
            let label_w = self
                .filtered
                .iter()
                .map(|c| c.label().len())
                .max()
                .unwrap_or(0);

            let items: Vec<ListItem> = self
                .filtered
                .iter()
                .map(|cmd| {
                    let line = Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            format!("{:<shortcut_w$}", cmd.shortcut()),
                            Style::default().fg(theme::accent()),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            format!("{:<label_w$}", cmd.label()),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(cmd.description(), Style::default().fg(theme::muted())),
                    ]);
                    ListItem::new(line)
                })
                .collect();
            let list = List::new(items).highlight_style(
                Style::default()
                    .bg(theme::accent())
                    .fg(theme::on_accent())
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(list, rows[2], &mut self.list_state);
        }

        // Footer
        let footer = Style::default().fg(theme::footer_text());
        let key = Style::default().fg(theme::accent());
        let line = Line::from(vec![
            Span::styled("↑/↓", key),
            Span::styled(" navigate    ", footer),
            Span::styled("enter", key),
            Span::styled(" run    ", footer),
            Span::styled("esc", key),
            Span::styled(" close", footer),
        ]);
        frame.render_widget(Paragraph::new(line), rows[3]);
    }
}

/// Build a centered rectangle within `area`.
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

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn esc_cancels() {
        let mut p = CommandPalette::new();
        assert_eq!(
            p.on_key(press(KeyCode::Esc)),
            CommandPaletteOutcome::Cancelled
        );
    }

    #[test]
    fn enter_returns_selected_command() {
        let mut p = CommandPalette::new();
        // Default selection is the first command.
        assert_eq!(
            p.on_key(press(KeyCode::Enter)),
            CommandPaletteOutcome::Selected(Command::NewSecret),
        );
    }

    #[test]
    fn typing_filters_the_list() {
        let mut p = CommandPalette::new();
        for ch in "env".chars() {
            p.on_key(press(KeyCode::Char(ch)));
        }
        assert_eq!(p.filtered, vec![Command::Envs]);
        assert_eq!(
            p.on_key(press(KeyCode::Enter)),
            CommandPaletteOutcome::Selected(Command::Envs),
        );
    }

    #[test]
    fn down_arrow_advances_selection() {
        let mut p = CommandPalette::new();
        p.on_key(press(KeyCode::Down));
        assert_eq!(
            p.on_key(press(KeyCode::Enter)),
            CommandPaletteOutcome::Selected(Command::SwitchStore),
        );
    }

    #[test]
    fn empty_filter_disables_enter() {
        let mut p = CommandPalette::new();
        for ch in "zzz".chars() {
            p.on_key(press(KeyCode::Char(ch)));
        }
        assert!(p.filtered.is_empty());
        assert_eq!(
            p.on_key(press(KeyCode::Enter)),
            CommandPaletteOutcome::Pending,
        );
    }
}
