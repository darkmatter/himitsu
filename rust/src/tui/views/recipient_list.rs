//! "Recipients" list view — an in-TUI affordance for `himitsu recipient ls`
//! plus add/remove entry points.
//!
//! Renders the recipients in the active store (name, fingerprint, description),
//! lets the user open the add form (`a` / `ctrl-n`), and remove the selected
//! recipient (`d`, confirmed with `y`). All mutations route through the same
//! [`crate::cli::recipient`] functions the CLI uses, so the surfaces can't
//! drift.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::standard_canvas;
use crate::cli::recipient::{self, RecipientEntry};
use crate::cli::Context;
use crate::tui::keymap::KeyMap;
use crate::tui::theme;

/// Outcome of handling a key — routed by [`crate::tui::app::App`].
#[derive(Debug, Clone)]
pub enum RecipientListAction {
    None,
    /// Esc — return to the search dashboard.
    Back,
    /// Ctrl-C quit.
    Quit,
    /// Open the add-recipient form.
    OpenAdd,
    /// A recipient was removed; carries the name for the toast.
    Removed(String),
    /// Removal failed; carries the error message for the toast.
    Failed(String),
}

pub struct RecipientListView {
    ctx: Context,
    entries: Vec<RecipientEntry>,
    list_state: ListState,
    /// When `Some(name)`, a delete of `name` is awaiting `y` confirmation.
    confirm_delete: Option<String>,
    /// Load error surfaced in the footer, if any.
    error: Option<String>,
}

impl RecipientListView {
    pub fn new(ctx: &Context) -> Self {
        let (entries, error) = match recipient::list_recipients(ctx) {
            Ok(entries) => (entries, None),
            Err(e) => (Vec::new(), Some(format!("{e}"))),
        };
        let mut list_state = ListState::default();
        if !entries.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            ctx: ctx.clone(),
            entries,
            list_state,
            confirm_delete: None,
            error,
        }
    }

    /// Reload the recipient list from disk (after a mutation).
    fn reload(&mut self) {
        match recipient::list_recipients(&self.ctx) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(format!("{e}"));
            }
        }
        let len = self.entries.len();
        match self.list_state.selected() {
            _ if len == 0 => self.list_state.select(None),
            Some(i) if i >= len => self.list_state.select(Some(len - 1)),
            None => self.list_state.select(Some(0)),
            _ => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, _keymap: &KeyMap) -> RecipientListAction {
        // Ctrl-C always quits, even mid-confirmation.
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
        ) {
            return RecipientListAction::Quit;
        }

        // A pending delete confirmation captures the next key.
        if let Some(name) = self.confirm_delete.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_delete = None;
                    return self.do_remove(&name);
                }
                _ => {
                    // Any other key cancels the confirmation.
                    self.confirm_delete = None;
                    return RecipientListAction::None;
                }
            }
        }

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => RecipientListAction::Back,
            (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.select_prev();
                RecipientListAction::None
            }
            (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.select_next();
                RecipientListAction::None
            }
            (KeyCode::Char('a'), KeyModifiers::NONE)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => RecipientListAction::OpenAdd,
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                if let Some(entry) = self.selected_entry() {
                    self.confirm_delete = Some(entry.name.clone());
                }
                RecipientListAction::None
            }
            _ => RecipientListAction::None,
        }
    }

    fn do_remove(&mut self, name: &str) -> RecipientListAction {
        // The mutation core runs the commit/push/completions chain.
        match crate::cli::store_ops::recipient_rm(&self.ctx, name) {
            Ok(()) => {
                self.reload();
                RecipientListAction::Removed(name.to_string())
            }
            Err(e) => RecipientListAction::Failed(format!("{e}")),
        }
    }

    fn selected_entry(&self) -> Option<&RecipientEntry> {
        self.list_state.selected().and_then(|i| self.entries.get(i))
    }

    fn select_prev(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = if i == 0 {
            self.entries.len() - 1
        } else {
            i - 1
        };
        self.list_state.select(Some(next));
    }

    fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1) % self.entries.len();
        self.list_state.select(Some(next));
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = standard_canvas(frame.area());

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(theme::brand_chip("recipients")))
            .border_style(Style::default().fg(theme::border()));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        self.draw_list(frame, rows[0]);
        self.draw_footer(frame, rows[1]);
    }

    fn draw_list(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if self.entries.is_empty() {
            let msg = if let Some(err) = &self.error {
                format!("  error: {err}")
            } else {
                "  no recipients — press 'a' to add one".to_string()
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    msg,
                    Style::default().fg(theme::muted()),
                ))),
                area,
            );
            return;
        }

        // Pad the name column so fingerprints line up.
        let name_w = self.entries.iter().map(|e| e.name.len()).max().unwrap_or(0);

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|e| {
                let mut spans = vec![
                    Span::raw(" "),
                    Span::raw(format!("{:<name_w$}", e.name, name_w = name_w)),
                    Span::raw("  "),
                    Span::styled(e.short_key.clone(), Style::default().fg(theme::accent())),
                ];
                if !e.description.is_empty() {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        e.description.clone(),
                        Style::default().fg(theme::muted()),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme::accent())
                .fg(theme::on_accent())
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = if let Some(name) = &self.confirm_delete {
            Line::from(Span::styled(
                format!("delete recipient '{name}'? press y to confirm, any other key to cancel"),
                Style::default().fg(theme::danger()),
            ))
        } else {
            let footer = Style::default().fg(theme::footer_text());
            let accent = Style::default().fg(theme::accent());
            Line::from(vec![
                Span::styled("a", accent),
                Span::styled(" add    ", footer),
                Span::styled("d", accent),
                Span::styled(" remove    ", footer),
                Span::styled("↑/↓", accent),
                Span::styled(" move    ", footer),
                Span::styled("esc", accent),
                Span::styled(" back", footer),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }

    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("up / down (k / j)", "move selection"),
            ("a / ctrl-n", "add a recipient"),
            ("d", "remove selected (confirm with y)"),
            ("esc", "back to search"),
            ("ctrl-c", "quit"),
            ("?", "toggle this help"),
        ]
    }

    pub fn help_title() -> &'static str {
        "recipients · keys"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    const AGE_KEY_1: &str = "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";
    const AGE_KEY_2: &str = "age1lvyvwawkr0mcnnnncaghunadrqkmuf9e6507x9y920xxpp866cnql7dp2z";

    fn mk_ctx() -> (TempDir, Context) {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().to_path_buf();
        std::fs::create_dir_all(crate::remote::store::recipients_dir(&store)).unwrap();
        let ctx = Context {
            data_dir: tmp.path().join("data"),
            state_dir: tmp.path().join("state"),
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
            project_root: None,
            git: std::sync::Arc::new(crate::git::CliGitAdapter),
            project_config_cell: Default::default(),
        };
        (tmp, ctx)
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn empty_ctx() -> Context {
        Context {
            data_dir: PathBuf::new(),
            state_dir: PathBuf::new(),
            store: PathBuf::new(),
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
            project_root: None,
            git: std::sync::Arc::new(crate::git::CliGitAdapter),
            project_config_cell: Default::default(),
        }
    }

    #[test]
    fn esc_goes_back() {
        let km = KeyMap::default();
        let mut view = RecipientListView::new(&empty_ctx());
        assert!(matches!(
            view.on_key(press(KeyCode::Esc), &km),
            RecipientListAction::Back
        ));
    }

    #[test]
    fn ctrl_c_quits() {
        let km = KeyMap::default();
        let mut view = RecipientListView::new(&empty_ctx());
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(view.on_key(key, &km), RecipientListAction::Quit));
    }

    #[test]
    fn a_opens_add_form() {
        let km = KeyMap::default();
        let mut view = RecipientListView::new(&empty_ctx());
        assert!(matches!(
            view.on_key(press(KeyCode::Char('a')), &km),
            RecipientListAction::OpenAdd
        ));
    }

    #[test]
    fn lists_recipients_from_store() {
        let (_tmp, ctx) = mk_ctx();
        crate::cli::store_ops::recipient_add(&ctx, "alice", AGE_KEY_1, Some("Alice".into()))
            .unwrap();
        crate::cli::store_ops::recipient_add(&ctx, "bob", AGE_KEY_2, None).unwrap();

        let view = RecipientListView::new(&ctx);
        assert_eq!(view.entries.len(), 2);
        let names: Vec<&str> = view.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"alice"));
        assert!(names.contains(&"bob"));
    }

    #[test]
    fn delete_requires_confirmation_then_removes() {
        let km = KeyMap::default();
        let (_tmp, ctx) = mk_ctx();
        crate::cli::store_ops::recipient_add(&ctx, "alice", AGE_KEY_1, None).unwrap();

        let mut view = RecipientListView::new(&ctx);
        assert_eq!(view.entries.len(), 1);

        // First `d` arms the confirmation but does not remove.
        assert!(matches!(
            view.on_key(press(KeyCode::Char('d')), &km),
            RecipientListAction::None
        ));
        assert!(view.confirm_delete.is_some());
        assert_eq!(view.entries.len(), 1);

        // `y` confirms and removes.
        match view.on_key(press(KeyCode::Char('y')), &km) {
            RecipientListAction::Removed(name) => assert_eq!(name, "alice"),
            other => panic!("expected Removed, got {other:?}"),
        }
        assert!(view.confirm_delete.is_none());
        assert_eq!(view.entries.len(), 0);
    }

    #[test]
    fn delete_confirmation_cancels_on_other_key() {
        let km = KeyMap::default();
        let (_tmp, ctx) = mk_ctx();
        crate::cli::store_ops::recipient_add(&ctx, "alice", AGE_KEY_1, None).unwrap();

        let mut view = RecipientListView::new(&ctx);
        view.on_key(press(KeyCode::Char('d')), &km);
        assert!(view.confirm_delete.is_some());
        // Pressing `n` (not `y`) cancels.
        view.on_key(press(KeyCode::Char('n')), &km);
        assert!(view.confirm_delete.is_none());
        assert_eq!(view.entries.len(), 1);
    }
}
