//! Search view: fuzzy path search across all known stores.
//!
//! Data comes from [`crate::cli::search::search_core`] — the same function
//! that powers the `himitsu search` CLI, so both views stay in sync.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::cli::search::{search_core, SearchResult};
use crate::cli::Context;

/// Outcome of handling a key — lets the app router decide where to go next.
#[derive(Debug, Clone)]
pub enum SearchAction {
    /// Stay in the search view.
    None,
    /// User pressed Esc — return to dashboard.
    Back,
    /// User hit Enter on a result — open the secret viewer for this selection.
    ///
    /// The payload is consumed once US-006 wires up the viewer; until then the
    /// router treats it as "go back to dashboard".
    #[allow(dead_code)]
    OpenViewer(SearchResult),
    /// User pressed q outside of any input (not expected here; kept for
    /// completeness with the global router).
    Quit,
}

pub struct SearchView {
    query: String,
    results: Vec<SearchResult>,
    list_state: ListState,
    /// Snapshot of the context used to build this view.
    ///
    /// We clone the bits we actually need (`store`, `state_dir`) so the view
    /// owns its own data — keeping borrow lifetimes simple in the app router.
    ctx: Context,
}

impl SearchView {
    pub fn new(ctx: &Context) -> Self {
        let ctx_owned = Context {
            data_dir: ctx.data_dir.clone(),
            state_dir: ctx.state_dir.clone(),
            store: ctx.store.clone(),
            recipients_path: ctx.recipients_path.clone(),
        };
        let mut view = Self {
            query: String::new(),
            results: Vec::new(),
            list_state: ListState::default(),
            ctx: ctx_owned,
        };
        view.refresh_results();
        view
    }

    pub fn on_key(&mut self, key: KeyEvent) -> SearchAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => SearchAction::Back,
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => SearchAction::Quit,
            (KeyCode::Enter, _) => match self.selected_result().cloned() {
                Some(r) => SearchAction::OpenViewer(r),
                None => SearchAction::None,
            },
            (KeyCode::Up, _) => {
                self.select_prev();
                SearchAction::None
            }
            (KeyCode::Down, _) => {
                self.select_next();
                SearchAction::None
            }
            (KeyCode::Backspace, _) => {
                if self.query.pop().is_some() {
                    self.refresh_results();
                }
                SearchAction::None
            }
            (KeyCode::Char(ch), _) => {
                self.query.push(ch);
                self.refresh_results();
                SearchAction::None
            }
            _ => SearchAction::None,
        }
    }

    fn refresh_results(&mut self) {
        self.results = search_core(&self.ctx, &self.query).unwrap_or_default();
        if self.results.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    fn selected_result(&self) -> Option<&SearchResult> {
        self.list_state.selected().and_then(|i| self.results.get(i))
    }

    fn select_prev(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = if i == 0 { self.results.len() - 1 } else { i - 1 };
        self.list_state.select(Some(next));
    }

    fn select_next(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1) % self.results.len();
        self.list_state.select(Some(next));
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_input(frame, chunks[1]);
        self.draw_results(frame, chunks[2]);
        self.draw_footer(frame, chunks[3]);
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let header = Line::from(vec![
            Span::styled(
                " himitsu ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled("search", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} result{}",
                    self.results.len(),
                    if self.results.len() == 1 { "" } else { "s" }
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(header), area);
    }

    fn draw_input(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title(" query ");
        let text = Line::from(vec![
            Span::raw(&self.query),
            Span::styled("█", Style::default().fg(Color::Cyan)),
        ]);
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_results(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title(" results ");

        if self.results.is_empty() {
            let msg = if self.query.is_empty() {
                "  no secrets found"
            } else {
                "  no matches"
            };
            let p = Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(Color::DarkGray),
            )))
            .block(block);
            frame.render_widget(p, area);
            return;
        }

        let path_w = self
            .results
            .iter()
            .map(|r| r.path.len())
            .max()
            .unwrap_or(0);
        let store_w = self
            .results
            .iter()
            .map(|r| r.store.len())
            .max()
            .unwrap_or(0);

        let items: Vec<ListItem> = self
            .results
            .iter()
            .map(|r| {
                let created = r.created_at.as_deref().unwrap_or("-");
                let line = Line::from(vec![
                    Span::raw(format!("{:<path_w$}  ", r.path, path_w = path_w)),
                    Span::styled(
                        format!("{:<store_w$}  ", r.store, store_w = store_w),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(created.to_string(), Style::default().fg(Color::DarkGray)),
                ]);
                ListItem::new(line)
            })
            .collect();

        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let footer = Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(Color::Cyan)),
            Span::raw(" navigate  "),
            Span::styled("enter", Style::default().fg(Color::Cyan)),
            Span::raw(" open  "),
            Span::styled("esc", Style::default().fg(Color::Cyan)),
            Span::raw(" back  "),
            Span::styled("ctrl-c", Style::default().fg(Color::Cyan)),
            Span::raw(" quit"),
        ]);
        frame.render_widget(Paragraph::new(footer), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_ctx(store: &std::path::Path) -> Context {
        Context {
            data_dir: PathBuf::new(),
            state_dir: store.parent().unwrap().to_path_buf(),
            store: store.to_path_buf(),
            recipients_path: None,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn seeded_store() -> TempDir {
        let dir = TempDir::new().unwrap();
        let store = dir.path().join("store");
        std::fs::create_dir_all(store.join(".himitsu/secrets/prod")).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/secrets/staging")).unwrap();
        // Minimal fake envelopes — search_core only reads paths + created_at,
        // and read_secret_meta falls back to default for unparseable files.
        std::fs::write(
            store.join(".himitsu/secrets/prod/API_KEY.yaml"),
            "value: ENC[age,placeholder]\nhimitsu:\n  created_at: '2026-01-01'\n  lastmodified: '2026-01-01T00:00:00Z'\n  age: []\n  history: []\n",
        )
        .unwrap();
        std::fs::write(
            store.join(".himitsu/secrets/prod/DATABASE_URL.yaml"),
            "value: ENC[age,placeholder]\nhimitsu:\n  created_at: '2026-01-02'\n  lastmodified: '2026-01-02T00:00:00Z'\n  age: []\n  history: []\n",
        )
        .unwrap();
        std::fs::write(
            store.join(".himitsu/secrets/staging/API_KEY.yaml"),
            "value: ENC[age,placeholder]\nhimitsu:\n  created_at: '2026-01-03'\n  lastmodified: '2026-01-03T00:00:00Z'\n  age: []\n  history: []\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn empty_query_returns_all_results() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let view = SearchView::new(&ctx);
        assert_eq!(view.results.len(), 3);
        assert_eq!(view.list_state.selected(), Some(0));
    }

    #[test]
    fn typing_narrows_results_live() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);

        view.on_key(key(KeyCode::Char('d')));
        view.on_key(key(KeyCode::Char('a')));
        view.on_key(key(KeyCode::Char('t')));
        assert!(view
            .results
            .iter()
            .all(|r| r.path.to_lowercase().contains("dat")));
        assert_eq!(view.results.len(), 1);
        assert_eq!(view.results[0].path, "prod/DATABASE_URL");
    }

    #[test]
    fn backspace_widens_results() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        view.on_key(key(KeyCode::Char('d')));
        view.on_key(key(KeyCode::Char('a')));
        view.on_key(key(KeyCode::Char('t')));
        assert_eq!(view.results.len(), 1);
        view.on_key(key(KeyCode::Backspace));
        view.on_key(key(KeyCode::Backspace));
        view.on_key(key(KeyCode::Backspace));
        assert_eq!(view.results.len(), 3);
    }

    #[test]
    fn esc_emits_back_action() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        assert!(matches!(view.on_key(key(KeyCode::Esc)), SearchAction::Back));
    }

    #[test]
    fn enter_emits_open_viewer_with_selection() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        view.on_key(key(KeyCode::Down));
        match view.on_key(key(KeyCode::Enter)) {
            SearchAction::OpenViewer(r) => assert_eq!(r.path, "prod/DATABASE_URL"),
            other => panic!("expected OpenViewer, got {other:?}"),
        }
    }

    #[test]
    fn enter_with_no_results_is_noop() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        view.on_key(key(KeyCode::Char('z')));
        view.on_key(key(KeyCode::Char('z')));
        view.on_key(key(KeyCode::Char('z')));
        assert_eq!(view.results.len(), 0);
        assert!(matches!(
            view.on_key(key(KeyCode::Enter)),
            SearchAction::None
        ));
    }

    #[test]
    fn nav_wraps_around() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        assert_eq!(view.list_state.selected(), Some(0));
        view.on_key(key(KeyCode::Up));
        assert_eq!(view.list_state.selected(), Some(2));
        view.on_key(key(KeyCode::Down));
        assert_eq!(view.list_state.selected(), Some(0));
    }
}
