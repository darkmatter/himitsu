//! Search view: fuzzy path search across all known stores.
//!
//! Data comes from [`crate::cli::search::search_core`] — the same function
//! that powers the `himitsu search` CLI, so both views stay in sync.
//!
//! Search is the TUI root: Esc quits, and the bindings that used to live on
//! the dashboard (new secret, switch store) are hosted here behind Ctrl
//! modifiers so the query field can keep eating ordinary letters.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::cli::search::{search_core, SearchResult};
use crate::cli::Context;
use crate::crypto::{age, secret_value};
use crate::remote::store;
use crate::tui::keymap::{Bindings, KeyMap};
use crate::tui::views::store_picker::{StorePicker, StorePickerOutcome};

/// Outcome of handling a key — lets the app router decide where to go next.
#[derive(Debug, Clone)]
pub enum SearchAction {
    /// Stay in the search view.
    None,
    /// User hit Enter on a result — open the secret viewer for this selection.
    OpenViewer(SearchResult),
    /// User requested the new-secret form (Ctrl+N).
    NewSecret,
    /// User picked a new active store via the embedded picker overlay.
    SwitchStore(PathBuf),
    /// User pressed Esc / Ctrl-C — root view, so quit the app.
    Quit,
    /// User pressed Ctrl+Y and we successfully copied the selected secret
    /// value to the clipboard. Carries the secret path for the toast.
    Copied(String),
    /// Ctrl+Y attempted but failed (no selection / decrypt error / no
    /// clipboard backend). Carries a human-readable error string.
    CopyFailed(String),
}

/// A row in the rendered results list. Store and Folder rows are visual-only
/// headers — they group the secrets that follow and are never selectable;
/// navigation steps over them. Stores group by origin (`org/repo` slug or
/// local path); folders group adjacent secrets sharing a top-level path
/// prefix within a store.
#[derive(Debug, Clone)]
enum Row {
    Store { name: String, count: usize },
    Folder { name: String, count: usize },
    Secret {
        result: SearchResult,
        /// Indentation depth in list-item cells (2 spaces per level). Level
        /// 0 = flat, 1 = under one header (folder or store), 2 = under both.
        indent: usize,
    },
}

pub struct SearchView {
    query: String,
    results: Vec<SearchResult>,
    rows: Vec<Row>,
    list_state: ListState,
    /// Snapshot of the context used to build this view.
    ///
    /// We clone the bits we actually need (`store`, `state_dir`) so the view
    /// owns its own data — keeping borrow lifetimes simple in the app router.
    ctx: Context,
    /// Embedded store-picker overlay. When `Some`, it intercepts every key.
    picker: Option<StorePicker>,
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
            rows: Vec::new(),
            list_state: ListState::default(),
            ctx: ctx_owned,
            picker: None,
        };
        view.refresh_results();
        view
    }

    pub fn on_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> SearchAction {
        // Picker overlay swallows every key while open.
        if let Some(picker) = self.picker.as_mut() {
            match picker.on_key(key) {
                StorePickerOutcome::Pending => return SearchAction::None,
                StorePickerOutcome::Cancelled => {
                    self.picker = None;
                    return SearchAction::None;
                }
                StorePickerOutcome::Selected(path) => {
                    self.picker = None;
                    return SearchAction::SwitchStore(path);
                }
            }
        }

        // Configurable action bindings take precedence over the fall-through
        // text-editing keys below. Quit is checked first so a user who rebinds
        // new_secret to a printable character still has an escape hatch.
        if keymap.quit.matches(&key) {
            return SearchAction::Quit;
        }
        if keymap.new_secret.matches(&key) {
            return SearchAction::NewSecret;
        }
        if keymap.switch_store.matches(&key) {
            self.picker = Some(StorePicker::new(
                &self.ctx.stores_dir(),
                self.ctx.store.clone(),
            ));
            return SearchAction::None;
        }
        if keymap.copy_selected.matches(&key) {
            return self.copy_selected_to_clipboard();
        }

        match (key.code, key.modifiers) {
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
            (KeyCode::Char(ch), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.query.push(ch);
                self.refresh_results();
                SearchAction::None
            }
            _ => SearchAction::None,
        }
    }

    /// Decrypt the currently selected secret and copy its value to the system
    /// clipboard. Returns a [`SearchAction`] carrying the copy outcome so the
    /// router can surface it as a toast; never panics on headless/no-selection.
    /// Mirrors the viewer's `y` binding so users can grab a value without
    /// having to step into the detail view.
    fn copy_selected_to_clipboard(&mut self) -> SearchAction {
        let Some(result) = self.selected_result().cloned() else {
            return SearchAction::CopyFailed("no selection to copy".to_string());
        };
        let value = match decrypt_value(&self.ctx, &result) {
            Ok(v) => v,
            Err(e) => return SearchAction::CopyFailed(format!("decrypt failed: {e}")),
        };
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(value)) {
            Ok(()) => SearchAction::Copied(result.path),
            Err(e) => SearchAction::CopyFailed(format!("clipboard unavailable: {e}")),
        }
    }

    fn refresh_results(&mut self) {
        self.results = search_core(&self.ctx, &self.query).unwrap_or_default();
        self.rows = build_rows(&self.results);
        self.list_state.select(self.first_selectable());
    }

    fn selected_result(&self) -> Option<&SearchResult> {
        self.list_state
            .selected()
            .and_then(|i| self.rows.get(i))
            .and_then(|row| match row {
                Row::Secret { result, .. } => Some(result),
                Row::Folder { .. } | Row::Store { .. } => None,
            })
    }

    fn is_selectable(&self, i: usize) -> bool {
        matches!(self.rows.get(i), Some(Row::Secret { .. }))
    }

    fn first_selectable(&self) -> Option<usize> {
        (0..self.rows.len()).find(|i| self.is_selectable(*i))
    }

    fn select_prev(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let Some(start) = self.list_state.selected() else {
            self.list_state.select(self.first_selectable());
            return;
        };
        let len = self.rows.len();
        for step in 1..=len {
            let idx = (start + len - step) % len;
            if self.is_selectable(idx) {
                self.list_state.select(Some(idx));
                return;
            }
        }
    }

    fn select_next(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let Some(start) = self.list_state.selected() else {
            self.list_state.select(self.first_selectable());
            return;
        };
        let len = self.rows.len();
        for step in 1..=len {
            let idx = (start + step) % len;
            if self.is_selectable(idx) {
                self.list_state.select(Some(idx));
                return;
            }
        }
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

        // Render the picker overlay last so it sits on top of the rest.
        if let Some(picker) = self.picker.as_mut() {
            picker.draw(frame);
        }
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
        let outer = Block::default().borders(Borders::ALL).title(" results ");
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        if self.rows.is_empty() {
            let msg = if self.query.is_empty() {
                "  no secrets found"
            } else {
                "  no matches"
            };
            let p = Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(p, inner);
            return;
        }

        // When multi-store grouping is active the store name is already in
        // the header row and we drop the redundant per-row store column.
        let has_store_headers = self.rows.iter().any(|r| matches!(r, Row::Store { .. }));

        // Column widths are computed against the widest secret row and the
        // header label itself, so short paths still leave room for "PATH" /
        // "STORE" to read cleanly.
        let path_w = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Secret { result, indent } => Some(result.path.len() + indent * 2),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            .max("PATH".len());
        let store_w = if has_store_headers {
            0
        } else {
            self.rows
                .iter()
                .filter_map(|row| match row {
                    Row::Secret { result, .. } => Some(result.store.len()),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
                .max("STORE".len())
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        let header_style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD);
        let mut header_spans = vec![Span::styled(
            format!("{:<path_w$}  ", "PATH", path_w = path_w),
            header_style,
        )];
        if !has_store_headers {
            header_spans.push(Span::styled(
                format!("{:<store_w$}  ", "STORE", store_w = store_w),
                header_style,
            ));
        }
        header_spans.push(Span::styled("CREATED", header_style));
        frame.render_widget(Paragraph::new(Line::from(header_spans)), chunks[0]);

        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|row| match row {
                Row::Store { name, count } => {
                    let line = Line::from(vec![
                        Span::styled(
                            format!("■ {name}"),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            format!("({count})"),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]);
                    ListItem::new(line)
                }
                Row::Folder { name, count } => {
                    let line = Line::from(vec![
                        Span::styled(
                            format!("▸ {name}/"),
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            format!("({count})"),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]);
                    ListItem::new(line)
                }
                Row::Secret { result, indent } => {
                    let prefix = "  ".repeat(*indent);
                    let created = result.created_at.as_deref().unwrap_or("-");
                    let padded_path = format!("{prefix}{}", result.path);
                    let mut spans = vec![Span::raw(format!("{padded_path:<path_w$}  "))];
                    if !has_store_headers {
                        spans.push(Span::styled(
                            format!("{:<store_w$}  ", result.store, store_w = store_w),
                            Style::default().fg(Color::Cyan),
                        ));
                    }
                    spans.push(Span::styled(
                        created.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ));
                    ListItem::new(Line::from(spans))
                }
            })
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, chunks[1], &mut self.list_state);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(Color::Cyan)),
            Span::raw(" navigate  "),
            Span::styled("enter", Style::default().fg(Color::Cyan)),
            Span::raw(" open  "),
            Span::styled("ctrl-n", Style::default().fg(Color::Cyan)),
            Span::raw(" new  "),
            Span::styled("ctrl-s", Style::default().fg(Color::Cyan)),
            Span::raw(" switch store  "),
            Span::styled("ctrl-y", Style::default().fg(Color::Cyan)),
            Span::raw(" copy  "),
            Span::styled("?", Style::default().fg(Color::Cyan)),
            Span::raw(" help  "),
            Span::styled("esc", Style::default().fg(Color::Cyan)),
            Span::raw(" quit"),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }
}

/// Decrypt a search result's ciphertext to its UTF-8 value, using the
/// identity file tied to the result's origin store. Kept as a free function
/// so it stays trivially testable without the full view state.
fn decrypt_value(ctx: &Context, result: &SearchResult) -> crate::error::Result<String> {
    let mut ctx_for_store = ctx.clone();
    ctx_for_store.store = result.store_path.clone();
    let ciphertext = store::read_secret(&result.store_path, &result.path)?;
    let identity = age::read_identity(&ctx_for_store.key_path())?;
    let plain = age::decrypt(&ciphertext, &identity)?;
    let decoded = secret_value::decode(&plain);
    Ok(String::from_utf8_lossy(&decoded.data).into_owned())
}

/// Group a flat list of results into rows.
///
/// When results span **multiple stores**, rows are partitioned per-store with
/// a `Store` header row per bucket; within each bucket we apply path-prefix
/// folder grouping. When only one store is present we fall back to the
/// single-store layout (no store header, flat path-prefix grouping).
///
/// A "folder" is any top-level path segment that contains ≥ 2 leaves. Single
/// leaves render flat. Folders always sort before singles; within each group
/// entries are alphabetized so the layout is stable regardless of input order.
fn build_rows(results: &[SearchResult]) -> Vec<Row> {
    use std::collections::BTreeMap;

    // Bucket by store, preserving deterministic alphabetical order so the
    // rendering is stable across calls.
    let mut by_store: BTreeMap<String, Vec<SearchResult>> = BTreeMap::new();
    for r in results {
        by_store.entry(r.store.clone()).or_default().push(r.clone());
    }

    let multi_store = by_store.len() > 1;
    let mut rows = Vec::new();
    for (store_name, bucket) in by_store {
        if multi_store {
            rows.push(Row::Store {
                name: store_name,
                count: bucket.len(),
            });
        }
        append_folder_grouped_rows(&mut rows, bucket, multi_store);
    }
    rows
}

/// Append `bucket` rows to `rows` applying path-prefix folder grouping.
///
/// When `under_store_header` is true, every row gets an extra level of
/// indentation so the store header visually owns its children.
fn append_folder_grouped_rows(
    rows: &mut Vec<Row>,
    bucket: Vec<SearchResult>,
    under_store_header: bool,
) {
    use std::collections::HashMap;

    let store_indent: usize = if under_store_header { 1 } else { 0 };

    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<SearchResult>> = HashMap::new();
    for r in bucket {
        let prefix = match r.path.split_once('/') {
            Some((head, _)) => head.to_string(),
            None => r.path.clone(),
        };
        if !groups.contains_key(&prefix) {
            order.push(prefix.clone());
        }
        groups.entry(prefix).or_default().push(r);
    }

    let mut folders: Vec<(String, Vec<SearchResult>)> = Vec::new();
    let mut singles: Vec<(String, Vec<SearchResult>)> = Vec::new();
    for name in order {
        let items = groups.remove(&name).unwrap_or_default();
        if items.len() >= 2 {
            folders.push((name, items));
        } else {
            singles.push((name, items));
        }
    }
    folders.sort_by(|a, b| a.0.cmp(&b.0));
    singles.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, mut items) in folders {
        let count = items.len();
        rows.push(Row::Folder { name, count });
        items.sort_by(|a, b| a.path.cmp(&b.path));
        for result in items {
            rows.push(Row::Secret {
                result,
                indent: store_indent + 1,
            });
        }
    }
    for (_, items) in singles {
        for result in items {
            rows.push(Row::Secret {
                result,
                indent: store_indent,
            });
        }
    }
}

// ── Help overlay integration (US-012) ─────────────────────────────────
//
// In its own impl block so parallel branches adding new bindings can extend
// `help_entries` without colliding with the main impl.
impl SearchView {
    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("type", "filter results"),
            ("↑/↓", "navigate"),
            ("enter", "open selection"),
            ("backspace", "delete char"),
            ("ctrl-n", "new secret"),
            ("ctrl-s", "switch store"),
            ("ctrl-y", "copy selection to clipboard"),
            ("?", "toggle this help"),
            ("esc / ctrl-c", "quit"),
        ]
    }

    pub fn help_title() -> &'static str {
        "search · keys"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keymap::KeyMap;
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

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
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
        // Rows: [Folder(prod,2), Secret(prod/API_KEY), Secret(prod/DATABASE_URL), Secret(staging/API_KEY)]
        assert_eq!(view.rows.len(), 4);
        assert!(matches!(view.rows[0], Row::Folder { ref name, count: 2 } if name == "prod"));
        assert_eq!(view.list_state.selected(), Some(1));
    }

    #[test]
    fn folders_first_grouping() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let view = SearchView::new(&ctx);
        let kinds: Vec<&'static str> = view
            .rows
            .iter()
            .map(|r| match r {
                Row::Store { .. } => "store",
                Row::Folder { .. } => "folder",
                Row::Secret { indent, .. } if *indent > 0 => "child",
                Row::Secret { .. } => "leaf",
            })
            .collect();
        assert_eq!(kinds, vec!["folder", "child", "child", "leaf"]);
        match &view.rows[3] {
            Row::Secret { result, .. } => assert_eq!(result.path, "staging/API_KEY"),
            _ => panic!("expected secret leaf at row 3"),
        }
    }

    #[test]
    fn typing_narrows_results_live() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);

        view.on_key(key(KeyCode::Char('d')), &km);
        view.on_key(key(KeyCode::Char('a')), &km);
        view.on_key(key(KeyCode::Char('t')), &km);
        assert!(view
            .results
            .iter()
            .all(|r| r.path.to_lowercase().contains("dat")));
        assert_eq!(view.results.len(), 1);
        assert_eq!(view.results[0].path, "prod/DATABASE_URL");
    }

    #[test]
    fn backspace_widens_results() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        view.on_key(key(KeyCode::Char('d')), &km);
        view.on_key(key(KeyCode::Char('a')), &km);
        view.on_key(key(KeyCode::Char('t')), &km);
        assert_eq!(view.results.len(), 1);
        view.on_key(key(KeyCode::Backspace), &km);
        view.on_key(key(KeyCode::Backspace), &km);
        view.on_key(key(KeyCode::Backspace), &km);
        assert_eq!(view.results.len(), 3);
    }

    #[test]
    fn esc_emits_quit_action() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        assert!(matches!(view.on_key(key(KeyCode::Esc), &km), SearchAction::Quit));
    }

    #[test]
    fn enter_emits_open_viewer_with_selection() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        view.on_key(key(KeyCode::Down), &km);
        match view.on_key(key(KeyCode::Enter), &km) {
            SearchAction::OpenViewer(r) => assert_eq!(r.path, "prod/DATABASE_URL"),
            other => panic!("expected OpenViewer, got {other:?}"),
        }
    }

    #[test]
    fn enter_with_no_results_is_noop() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        view.on_key(key(KeyCode::Char('z')), &km);
        view.on_key(key(KeyCode::Char('z')), &km);
        view.on_key(key(KeyCode::Char('z')), &km);
        assert_eq!(view.results.len(), 0);
        assert!(matches!(
            view.on_key(key(KeyCode::Enter), &km),
            SearchAction::None
        ));
    }

    #[test]
    fn nav_wraps_around() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        // Row 0 is a Folder header (unselectable); first secret is row 1.
        assert_eq!(view.list_state.selected(), Some(1));
        view.on_key(key(KeyCode::Up), &km);
        // Up skips the folder at row 0 and wraps to the last secret (row 3).
        assert_eq!(view.list_state.selected(), Some(3));
        view.on_key(key(KeyCode::Down), &km);
        // Down from row 3 wraps; row 0 is a folder so lands on row 1.
        assert_eq!(view.list_state.selected(), Some(1));
    }

    #[test]
    fn ctrl_n_emits_new_secret_action() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        assert!(matches!(
            view.on_key(ctrl('n'), &km),
            SearchAction::NewSecret
        ));
    }

    #[test]
    fn column_headers_are_rendered_above_results() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut rendered = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }
        assert!(rendered.contains("PATH"), "missing PATH header: {rendered}");
        assert!(rendered.contains("STORE"), "missing STORE header: {rendered}");
        assert!(
            rendered.contains("CREATED"),
            "missing CREATED header: {rendered}"
        );
    }

    /// Seed two stores under `<tmp>/state/stores/<org>/<repo>/.himitsu/secrets/`
    /// so `collect_stores()` walks `stores_dir` and returns both.
    fn seeded_multi_store() -> TempDir {
        let dir = TempDir::new().unwrap();
        let state = dir.path().join("state");
        let stores = state.join("stores");

        let alpha = stores.join("acme/alpha");
        let beta = stores.join("acme/beta");
        std::fs::create_dir_all(alpha.join(".himitsu/secrets/prod")).unwrap();
        std::fs::create_dir_all(beta.join(".himitsu/secrets/prod")).unwrap();

        let envelope = "value: ENC[age,placeholder]\nhimitsu:\n  created_at: '2026-01-01'\n  lastmodified: '2026-01-01T00:00:00Z'\n  age: []\n  history: []\n";
        std::fs::write(alpha.join(".himitsu/secrets/prod/API_KEY.yaml"), envelope).unwrap();
        std::fs::write(
            alpha.join(".himitsu/secrets/prod/DATABASE_URL.yaml"),
            envelope,
        )
        .unwrap();
        std::fs::write(beta.join(".himitsu/secrets/prod/API_KEY.yaml"), envelope).unwrap();

        // ctx.store will point at an empty dir so the explicit-store bucket
        // adds nothing and we rely entirely on the stores_dir scan.
        std::fs::create_dir_all(state.join("empty")).unwrap();
        dir
    }

    fn multi_ctx(root: &std::path::Path) -> Context {
        let state = root.join("state");
        Context {
            data_dir: PathBuf::new(),
            state_dir: state.clone(),
            store: state.join("empty"),
            recipients_path: None,
        }
    }

    #[test]
    fn multi_store_grouped_by_store_header() {
        let dir = seeded_multi_store();
        let ctx = multi_ctx(dir.path());
        let view = SearchView::new(&ctx);

        // Three results across two stores: alpha(2) + beta(1).
        assert_eq!(view.results.len(), 3);

        // Row 0: store header for acme/alpha (alphabetically first).
        match &view.rows[0] {
            Row::Store { name, count } => {
                assert_eq!(name, "acme/alpha");
                assert_eq!(*count, 2);
            }
            other => panic!("row 0 expected Store, got {other:?}"),
        }

        // Next: folder header "prod/" + two children under alpha (indented).
        assert!(matches!(view.rows[1], Row::Folder { ref name, count: 2 } if name == "prod"));
        match &view.rows[2] {
            Row::Secret { result, indent } => {
                assert_eq!(result.store, "acme/alpha");
                assert!(*indent >= 2, "alpha child indent >= 2 (got {indent})");
            }
            other => panic!("row 2 expected Secret, got {other:?}"),
        }

        // Beta store header appears later; its secret is indented >= 1.
        let beta_idx = view
            .rows
            .iter()
            .position(|r| matches!(r, Row::Store { name, .. } if name == "acme/beta"))
            .expect("acme/beta store header missing");
        match &view.rows[beta_idx + 1] {
            Row::Secret { result, indent } => {
                assert_eq!(result.store, "acme/beta");
                assert!(*indent >= 1);
            }
            other => panic!("row after beta header expected Secret, got {other:?}"),
        }

        // Initial selection lands on a secret, not a header.
        let sel = view.list_state.selected().unwrap();
        assert!(matches!(view.rows[sel], Row::Secret { .. }));
    }

    #[test]
    fn multi_store_nav_skips_headers() {
        let km = KeyMap::default();
        let dir = seeded_multi_store();
        let ctx = multi_ctx(dir.path());
        let mut view = SearchView::new(&ctx);
        for _ in 0..view.rows.len() * 2 {
            view.on_key(key(KeyCode::Down), &km);
            let sel = view.list_state.selected().unwrap();
            assert!(
                matches!(view.rows[sel], Row::Secret { .. }),
                "Down landed on non-secret row {sel}"
            );
        }
    }

    /// Seed a store with a real age identity + one encrypted secret so the
    /// search copy path has something decryptable to operate on.
    fn seeded_store_with_real_secret() -> (TempDir, Context) {
        use ::age::x25519::Identity;
        use secrecy::ExposeSecret;
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data");
        let state_dir = dir.path().join("state");
        let store = state_dir.join("stores/acme/prod");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/secrets")).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/recipients")).unwrap();

        let identity = Identity::generate();
        let pubkey = identity.to_public().to_string();
        let secret_key = identity.to_string().expose_secret().to_string();
        std::fs::write(data_dir.join("key"), &secret_key).unwrap();
        std::fs::write(
            store.join(".himitsu/recipients/me.pub"),
            format!("{pubkey}\n"),
        )
        .unwrap();

        let recipients = crate::crypto::age::collect_recipients(&store, None).unwrap();
        let ct = crate::crypto::age::encrypt(b"copied!", &recipients).unwrap();
        crate::remote::store::write_secret(&store, "prod/API_KEY", &ct).unwrap();

        let ctx = Context {
            data_dir,
            state_dir,
            store,
            recipients_path: None,
        };
        (dir, ctx)
    }

    #[test]
    fn ctrl_y_surfaces_a_status_line_for_copy() {
        let km = KeyMap::default();
        let (_dir, ctx) = seeded_store_with_real_secret();
        let mut view = SearchView::new(&ctx);
        assert!(view.selected_result().is_some());
        let action = view.on_key(ctrl('y'), &km);
        assert!(
            matches!(action, SearchAction::Copied(_) | SearchAction::CopyFailed(_)),
            "ctrl-y should always emit a copy action, got {action:?}"
        );
    }

    #[test]
    fn ctrl_y_with_no_selection_reports_error() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        // Narrow to zero results so there's no selection to copy.
        for ch in "zzzzz".chars() {
            view.on_key(key(KeyCode::Char(ch)), &km);
        }
        assert_eq!(view.results.len(), 0);
        let action = view.on_key(ctrl('y'), &km);
        match action {
            SearchAction::CopyFailed(msg) => {
                assert!(msg.contains("no selection"), "unexpected msg: {msg}");
            }
            other => panic!("expected CopyFailed, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_s_opens_store_picker_overlay() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        assert!(view.picker.is_none());
        let action = view.on_key(ctrl('s'), &km);
        assert!(matches!(action, SearchAction::None));
        assert!(view.picker.is_some());
    }
}
