//! Dashboard view: environments list + cross-store secrets table.
//!
//! Data is sourced from [`search_core`] with an empty query so every
//! registered store contributes rows — the same pipeline the search view
//! uses. An "environment" is the first path segment of each secret
//! (e.g. `prod/DATABASE_URL` → env `prod`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState,
};
use ratatui::Frame;

use crate::cli::search::{relative_time, search_core, SearchResult};
use crate::cli::Context;
use crate::tui::views::store_picker::{StorePicker, StorePickerOutcome};

/// Which of the two panes has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardFocus {
    Envs,
    Secrets,
}

/// Outcome of handling a key — lets the app router decide where to go next.
///
/// Not `Copy`: `OpenViewer` carries a `SearchResult` and `SwitchStore`
/// carries a `PathBuf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardAction {
    None,
    Quit,
    EnterSearch,
    OpenViewer(SearchResult),
    SwitchStore(PathBuf),
    NewSecret,
}

pub struct DashboardView {
    /// Slug of the store currently in `ctx.store`. Rendered dim in the STORE
    /// column when it matches a row; other stores are highlighted.
    current_store_slug: String,
    /// Cached clone of the invoking `Context`, used by `refresh_and_select`
    /// so repeated reloads see the same stores_dir as the initial load.
    ctx: Context,
    envs: Vec<String>,
    /// Rows grouped by env, each group already folders-first sorted.
    rows_by_env: BTreeMap<String, Vec<SearchResult>>,
    env_state: ListState,
    secret_state: TableState,
    focus: DashboardFocus,
    stores_dir: PathBuf,
    picker: Option<StorePicker>,
    status: Option<(String, StatusKind)>,
}

#[derive(Debug, Clone, Copy)]
enum StatusKind {
    Info,
    Error,
}

impl DashboardView {
    pub fn new(ctx: &Context) -> Self {
        let current_store_slug = derive_store_slug(ctx);
        let (envs, rows_by_env) = load_rows(ctx);
        let mut env_state = ListState::default();
        if !envs.is_empty() {
            env_state.select(Some(0));
        }
        let mut secret_state = TableState::default();
        if let Some(first_env) = envs.first() {
            if rows_by_env
                .get(first_env)
                .is_some_and(|v: &Vec<SearchResult>| !v.is_empty())
            {
                secret_state.select(Some(0));
            }
        }
        Self {
            current_store_slug,
            ctx: ctx.clone(),
            envs,
            rows_by_env,
            env_state,
            secret_state,
            focus: DashboardFocus::Envs,
            stores_dir: ctx.stores_dir(),
            picker: None,
            status: None,
        }
    }

    /// Env segment currently highlighted in the env pane, if any. Used by
    /// the router to pre-fill the new-secret form.
    pub fn selected_env(&self) -> Option<String> {
        self.env_state
            .selected()
            .and_then(|i| self.envs.get(i).cloned())
    }

    /// Re-read the store from disk and (when possible) re-select the secret
    /// path that was just created. Called after the new-secret form submits.
    pub fn refresh_and_select(&mut self, created_path: Option<&str>) {
        let (envs, rows_by_env) = load_rows(&self.ctx);
        self.envs = envs;
        self.rows_by_env = rows_by_env;

        if let Some(path) = created_path {
            if let Some((env, _)) = path.split_once('/') {
                if let Some(idx) = self.envs.iter().position(|e| e == env) {
                    self.env_state.select(Some(idx));
                    self.reset_secret_selection();
                    return;
                }
            }
        }
        if self.envs.is_empty() {
            self.env_state.select(None);
            self.secret_state.select(None);
        } else if self.env_state.selected().is_none() {
            self.env_state.select(Some(0));
            self.reset_secret_selection();
        }
    }

    /// Surface a one-line status message below the body. Cleared on the next
    /// navigation keypress.
    pub fn set_status_info(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), StatusKind::Info));
    }

    pub fn set_status_error(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), StatusKind::Error));
    }

    pub fn on_key(&mut self, key: KeyEvent) -> DashboardAction {
        // When the picker overlay is open, route all keys to it first.
        if let Some(picker) = self.picker.as_mut() {
            match picker.on_key(key) {
                StorePickerOutcome::Pending => return DashboardAction::None,
                StorePickerOutcome::Cancelled => {
                    self.picker = None;
                    return DashboardAction::None;
                }
                StorePickerOutcome::Selected(path) => {
                    self.picker = None;
                    return DashboardAction::SwitchStore(path);
                }
            }
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => DashboardAction::Quit,
            (KeyCode::Char('q'), _) => DashboardAction::Quit,
            (KeyCode::Char('/'), _) => DashboardAction::EnterSearch,
            (KeyCode::Char('n'), _) => {
                self.status = None;
                DashboardAction::NewSecret
            }
            // Esc has no parent view to return to from the dashboard — swallow it.
            (KeyCode::Esc, _) => DashboardAction::None,
            (KeyCode::Tab, _) | (KeyCode::BackTab, _) => {
                self.toggle_focus();
                DashboardAction::None
            }
            (KeyCode::Right | KeyCode::Char('l'), _) => {
                self.set_focus(DashboardFocus::Secrets);
                DashboardAction::None
            }
            (KeyCode::Left | KeyCode::Char('h'), _) => {
                self.set_focus(DashboardFocus::Envs);
                DashboardAction::None
            }
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                self.status = None;
                self.select_prev();
                DashboardAction::None
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                self.status = None;
                self.select_next();
                DashboardAction::None
            }
            (KeyCode::Enter, _) => self.on_enter(),
            // US-013: `s` opens the store picker overlay.
            (KeyCode::Char('s'), KeyModifiers::NONE) => {
                self.picker = Some(StorePicker::new(&self.stores_dir, self.ctx.store.clone()));
                DashboardAction::None
            }
            _ => DashboardAction::None,
        }
    }

    fn on_enter(&mut self) -> DashboardAction {
        // Enter on the envs list is a no-op: drilling into an env is what
        // Tab/focus is for. Only the Secrets focus opens the viewer.
        if self.focus != DashboardFocus::Secrets {
            return DashboardAction::None;
        }
        let Some(row) = self.selected_row().cloned() else {
            return DashboardAction::None;
        };
        DashboardAction::OpenViewer(row)
    }

    fn toggle_focus(&mut self) {
        let next = match self.focus {
            DashboardFocus::Envs => DashboardFocus::Secrets,
            DashboardFocus::Secrets => DashboardFocus::Envs,
        };
        self.set_focus(next);
    }

    fn set_focus(&mut self, focus: DashboardFocus) {
        self.focus = focus;
        // When entering the Secrets pane, make sure it has a valid selection
        // (or None if the list is empty).
        if focus == DashboardFocus::Secrets {
            let len = self.selected_rows().len();
            match self.secret_state.selected() {
                Some(i) if i < len => {}
                _ => {
                    if len == 0 {
                        self.secret_state.select(None);
                    } else {
                        self.secret_state.select(Some(0));
                    }
                }
            }
        }
    }

    fn select_prev(&mut self) {
        match self.focus {
            DashboardFocus::Envs => self.env_prev(),
            DashboardFocus::Secrets => self.secret_prev(),
        }
    }

    fn select_next(&mut self) {
        match self.focus {
            DashboardFocus::Envs => self.env_next(),
            DashboardFocus::Secrets => self.secret_next(),
        }
    }

    fn env_prev(&mut self) {
        if self.envs.is_empty() {
            return;
        }
        let i = self.env_state.selected().unwrap_or(0);
        let next = if i == 0 { self.envs.len() - 1 } else { i - 1 };
        self.env_state.select(Some(next));
        self.reset_secret_selection();
    }

    fn env_next(&mut self) {
        if self.envs.is_empty() {
            return;
        }
        let i = self.env_state.selected().unwrap_or(0);
        let next = (i + 1) % self.envs.len();
        self.env_state.select(Some(next));
        self.reset_secret_selection();
    }

    fn secret_prev(&mut self) {
        let len = self.selected_rows().len();
        if len == 0 {
            return;
        }
        let i = self.secret_state.selected().unwrap_or(0);
        let next = if i == 0 { len - 1 } else { i - 1 };
        self.secret_state.select(Some(next));
    }

    fn secret_next(&mut self) {
        let len = self.selected_rows().len();
        if len == 0 {
            return;
        }
        let i = self.secret_state.selected().unwrap_or(0);
        let next = (i + 1) % len;
        self.secret_state.select(Some(next));
    }

    fn reset_secret_selection(&mut self) {
        let len = self.selected_rows().len();
        if len == 0 {
            self.secret_state.select(None);
        } else {
            self.secret_state.select(Some(0));
        }
    }

    fn selected_rows(&self) -> &[SearchResult] {
        self.env_state
            .selected()
            .and_then(|i| self.envs.get(i))
            .and_then(|env| self.rows_by_env.get(env))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    fn selected_row(&self) -> Option<&SearchResult> {
        let rows = self.selected_rows();
        self.secret_state.selected().and_then(|i| rows.get(i))
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_body(frame, chunks[1]);
        self.draw_footer(frame, chunks[2]);

        // Render the store picker overlay last so it sits on top.
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
            Span::styled(
                &self.current_store_slug,
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} env{}",
                    self.envs.len(),
                    if self.envs.len() == 1 { "" } else { "s" }
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(header), area);
    }

    fn draw_body(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
            .split(area);

        self.draw_envs(frame, columns[0]);
        self.draw_secrets(frame, columns[1]);
    }

    fn draw_envs(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let focused = self.focus == DashboardFocus::Envs;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" envs ")
            .border_style(border_style(focused));

        if self.envs.is_empty() {
            let msg = Paragraph::new(Line::from(Span::styled(
                "  no envs",
                Style::default().fg(Color::DarkGray),
            )))
            .block(block);
            frame.render_widget(msg, area);
            return;
        }

        let items: Vec<ListItem> = self
            .envs
            .iter()
            .map(|e| ListItem::new(Line::from(Span::raw(e.clone()))))
            .collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(highlight_style(focused))
            .style(body_style(focused));

        frame.render_stateful_widget(list, area, &mut self.env_state);
    }

    fn draw_secrets(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let focused = self.focus == DashboardFocus::Secrets;
        let title = match self.env_state.selected().and_then(|i| self.envs.get(i)) {
            Some(env) => format!(" secrets · {env} "),
            None => " secrets ".to_string(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(border_style(focused));
        let rows_len = self.selected_rows().len();

        if rows_len == 0 {
            let empty_msg = if self.envs.is_empty() {
                "  no secrets in this store"
            } else {
                "  no secrets in this env"
            };
            let msg = Paragraph::new(Line::from(Span::styled(
                empty_msg,
                Style::default().fg(Color::DarkGray),
            )))
            .block(block);
            frame.render_widget(msg, area);
            return;
        }

        let header_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let header = Row::new(vec![
            Cell::from("PATH").style(header_style),
            Cell::from("DESCRIPTION").style(header_style),
            Cell::from("MODIFIED").style(header_style),
            Cell::from("STORE").style(header_style),
        ])
        .height(1);

        // description column is currently blank — surfacing it requires
        // decrypting each row, deferred until a session-scoped cache exists.
        let rows: Vec<Row> = self
            .selected_rows()
            .iter()
            .map(|r| {
                let modified = relative_time(r.updated_at.as_deref());
                let is_home = r.store == self.current_store_slug;
                let store_style = if is_home {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::Magenta)
                };
                Row::new(vec![
                    Cell::from(r.path.clone()),
                    Cell::from(""),
                    Cell::from(modified),
                    Cell::from(r.store.clone()).style(store_style),
                ])
            })
            .collect();

        let widths = [
            Constraint::Percentage(45),
            Constraint::Percentage(25),
            Constraint::Length(14),
            Constraint::Min(10),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .block(block)
            .row_highlight_style(highlight_style(focused))
            .style(body_style(focused));

        frame.render_stateful_widget(table, area, &mut self.secret_state);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = if let Some((msg, kind)) = &self.status {
            let color = match kind {
                StatusKind::Info => Color::Green,
                StatusKind::Error => Color::Red,
            };
            Line::from(Span::styled(msg.clone(), Style::default().fg(color)))
        } else {
            Line::from(vec![
                Span::styled("↑/↓ j/k", Style::default().fg(Color::Cyan)),
                Span::raw(" navigate  "),
                Span::styled("tab", Style::default().fg(Color::Cyan)),
                Span::raw(" focus  "),
                Span::styled("enter", Style::default().fg(Color::Cyan)),
                Span::raw(" open  "),
                Span::styled("/", Style::default().fg(Color::Cyan)),
                Span::raw(" search  "),
                Span::styled("n", Style::default().fg(Color::Cyan)),
                Span::raw(" new  "),
                Span::styled("s", Style::default().fg(Color::Cyan)),
                Span::raw(" switch store  "),
                Span::styled("q", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }
}

// ── Help overlay integration (US-012) ─────────────────────────────────
//
// Kept in its own impl block so parallel branches adding new bindings can
// extend `help_entries` without colliding with the main impl.
impl DashboardView {
    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("tab / h l", "switch focus (envs ↔ secrets)"),
            ("↑/↓ j/k", "navigate focused list"),
            ("enter", "open selected secret"),
            ("/", "search"),
            ("s", "switch store"),
            ("?", "toggle this help"),
            ("q / ctrl-c", "quit"),
        ]
    }

    pub fn help_title() -> &'static str {
        "dashboard · keys"
    }
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn body_style(focused: bool) -> Style {
    if focused {
        Style::default()
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn highlight_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        // Greyed-out highlight so the user still sees *where* the selection
        // will be if they re-focus this pane, but it's clearly inactive.
        Style::default().bg(Color::DarkGray).fg(Color::Black)
    }
}

/// Load every secret across every registered store via `search_core`, then
/// group by env with folders-first ordering inside each group.
fn load_rows(ctx: &Context) -> (Vec<String>, BTreeMap<String, Vec<SearchResult>>) {
    let results = search_core(ctx, "").unwrap_or_default();
    group_and_sort(results)
}

fn group_and_sort(
    results: Vec<SearchResult>,
) -> (Vec<String>, BTreeMap<String, Vec<SearchResult>>) {
    let mut map: BTreeMap<String, Vec<SearchResult>> = BTreeMap::new();
    for r in results {
        let env = match r.path.split_once('/') {
            Some((head, _)) if !head.is_empty() => head.to_string(),
            _ => continue,
        };
        map.entry(env).or_default().push(r);
    }
    for rows in map.values_mut() {
        sort_folders_first(rows);
    }
    let envs: Vec<String> = map.keys().cloned().collect();
    (envs, map)
}

/// Folders-first sort: within a single env, entries whose path (after the
/// env segment) contains a `/` are "folder" entries and come first, grouped
/// by the first folder segment. Bare leaves follow, sorted alphabetically.
fn sort_folders_first(rows: &mut [SearchResult]) {
    rows.sort_by(|a, b| {
        let (a_folder, a_rest) = split_folder(&a.path);
        let (b_folder, b_rest) = split_folder(&b.path);
        match (a_folder, b_folder) {
            (Some(af), Some(bf)) => af.cmp(bf).then_with(|| a_rest.cmp(b_rest)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a_rest.cmp(b_rest),
        }
    });
}

/// Given a secret path like `prod/db/primary`, returns `(Some("db"),
/// "primary")`. For a bare `prod/API_KEY` returns `(None, "API_KEY")`.
fn split_folder(path: &str) -> (Option<&str>, &str) {
    let after_env = match path.split_once('/') {
        Some((_, rest)) => rest,
        None => path,
    };
    match after_env.split_once('/') {
        Some((folder, rest)) => (Some(folder), rest),
        None => (None, after_env),
    }
}

fn derive_store_slug(ctx: &Context) -> String {
    if ctx.store.as_os_str().is_empty() {
        return "(no store)".to_string();
    }
    let stores_dir = ctx.stores_dir();
    if let Ok(rel) = ctx.store.strip_prefix(&stores_dir) {
        let s = rel.to_string_lossy().replace('\\', "/");
        if !s.is_empty() {
            return s;
        }
    }
    ctx.store
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| ctx.store.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mk_result(store: &str, path: &str) -> SearchResult {
        SearchResult {
            store: store.to_string(),
            store_path: PathBuf::from(format!("/tmp/{store}")),
            path: path.to_string(),
            created_at: None,
            updated_at: Some("2026-04-10T12:00:00Z".to_string()),
        }
    }

    fn make_view(envs: &[(&str, &[&str])]) -> DashboardView {
        let mut rows_by_env: BTreeMap<String, Vec<SearchResult>> = BTreeMap::new();
        let mut env_list: Vec<String> = Vec::new();
        for (env, secrets) in envs {
            env_list.push((*env).to_string());
            let rows: Vec<SearchResult> = secrets
                .iter()
                .map(|p| mk_result("test/store", p))
                .collect();
            rows_by_env.insert((*env).to_string(), rows);
        }
        let mut env_state = ListState::default();
        let mut secret_state = TableState::default();
        if !env_list.is_empty() {
            env_state.select(Some(0));
            if let Some(first) = env_list.first() {
                if rows_by_env
                    .get(first)
                    .is_some_and(|v: &Vec<SearchResult>| !v.is_empty())
                {
                    secret_state.select(Some(0));
                }
            }
        }
        DashboardView {
            current_store_slug: "test/store".to_string(),
            ctx: Context {
                data_dir: PathBuf::from("/tmp/himitsu-test-data"),
                state_dir: PathBuf::from("/tmp/himitsu-test-state"),
                store: PathBuf::from("/tmp/test/store"),
                recipients_path: None,
            },
            envs: env_list,
            rows_by_env,
            env_state,
            secret_state,
            focus: DashboardFocus::Envs,
            stores_dir: PathBuf::from("/tmp/himitsu-test-stores"),
            picker: None,
            status: None,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn folders_first_sort_orders_correctly() {
        let mut rows = vec![
            mk_result("s", "prod/API_KEY"),
            mk_result("s", "prod/DATABASE_URL"),
            mk_result("s", "prod/db/primary_password"),
            mk_result("s", "prod/db/replica_password"),
            mk_result("s", "prod/cache/redis_url"),
        ];
        sort_folders_first(&mut rows);
        let got: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            got,
            vec![
                "prod/cache/redis_url",
                "prod/db/primary_password",
                "prod/db/replica_password",
                "prod/API_KEY",
                "prod/DATABASE_URL",
            ]
        );
    }

    #[test]
    fn table_rows_render_all_columns() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut view = make_view(&[("prod", &["prod/API_KEY", "prod/db/primary"])]);
        view.set_focus(DashboardFocus::Secrets);

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| view.draw(f)).unwrap();

        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }

        assert!(text.contains("PATH"), "missing PATH header:\n{text}");
        assert!(
            text.contains("DESCRIPTION"),
            "missing DESCRIPTION header:\n{text}"
        );
        assert!(
            text.contains("MODIFIED"),
            "missing MODIFIED header:\n{text}"
        );
        assert!(text.contains("STORE"), "missing STORE header:\n{text}");
        assert!(
            text.contains("prod/db/primary") || text.contains("prod/API_KEY"),
            "missing at least one data row:\n{text}"
        );
    }

    #[test]
    fn load_envs_groups_by_first_segment() {
        let results = vec![
            mk_result("s", "prod/API_KEY"),
            mk_result("s", "prod/DATABASE_URL"),
            mk_result("s", "staging/API_KEY"),
            mk_result("s", "bare_no_slash"),
        ];
        let (envs, map) = group_and_sort(results);
        assert_eq!(envs, vec!["prod", "staging"]);
        assert_eq!(map["prod"].len(), 2);
        assert_eq!(map["staging"].len(), 1);
    }

    #[test]
    fn navigation_wraps_around() {
        let mut view = make_view(&[("prod", &["prod/A"]), ("staging", &["staging/B"])]);
        assert_eq!(view.env_state.selected(), Some(0));
        view.select_next();
        assert_eq!(view.env_state.selected(), Some(1));
        view.select_next();
        assert_eq!(view.env_state.selected(), Some(0));
        view.select_prev();
        assert_eq!(view.env_state.selected(), Some(1));
    }

    #[test]
    fn selected_rows_updates_with_selection() {
        let mut view =
            make_view(&[("prod", &["prod/A", "prod/B"]), ("staging", &["staging/X"])]);
        assert_eq!(view.selected_rows().len(), 2);
        view.select_next();
        assert_eq!(view.selected_rows().len(), 1);
        assert_eq!(view.selected_rows()[0].path, "staging/X");
    }

    #[test]
    fn empty_view_has_no_selection() {
        let view = make_view(&[]);
        assert_eq!(view.env_state.selected(), None);
        assert!(view.selected_rows().is_empty());
    }

    #[test]
    fn navigation_on_empty_view_is_noop() {
        let mut view = make_view(&[]);
        view.select_next();
        view.select_prev();
        assert_eq!(view.env_state.selected(), None);
    }

    #[test]
    fn n_emits_new_secret_action() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert_eq!(
            view.on_key(press(KeyCode::Char('n'))),
            DashboardAction::NewSecret
        );
    }

    #[test]
    fn selected_env_returns_highlighted_env_name() {
        let mut view = make_view(&[("prod", &["prod/A"]), ("staging", &["staging/B"])]);
        assert_eq!(view.selected_env().as_deref(), Some("prod"));
        view.select_next();
        assert_eq!(view.selected_env().as_deref(), Some("staging"));
    }

    #[test]
    fn slash_emits_enter_search_action() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert!(matches!(
            view.on_key(press(KeyCode::Char('/'))),
            DashboardAction::EnterSearch
        ));
    }

    #[test]
    fn q_emits_quit_action() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert!(matches!(
            view.on_key(press(KeyCode::Char('q'))),
            DashboardAction::Quit
        ));
    }

    #[test]
    fn ctrl_c_emits_quit_action() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert!(matches!(view.on_key(ctrl('c')), DashboardAction::Quit));
    }

    #[test]
    fn esc_is_swallowed_on_dashboard() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert!(matches!(
            view.on_key(press(KeyCode::Esc)),
            DashboardAction::None
        ));
    }

    #[test]
    fn navigation_keys_do_not_emit_actions() {
        let mut view = make_view(&[("prod", &["prod/A"]), ("staging", &["staging/B"])]);
        assert!(matches!(
            view.on_key(press(KeyCode::Down)),
            DashboardAction::None
        ));
        assert_eq!(view.env_state.selected(), Some(1));
    }

    #[test]
    fn tab_cycles_focus_between_envs_and_secrets() {
        let mut view = make_view(&[("prod", &["prod/A", "prod/B"])]);
        assert_eq!(view.focus, DashboardFocus::Envs);
        assert!(matches!(
            view.on_key(press(KeyCode::Tab)),
            DashboardAction::None
        ));
        assert_eq!(view.focus, DashboardFocus::Secrets);
        assert!(matches!(
            view.on_key(press(KeyCode::Tab)),
            DashboardAction::None
        ));
        assert_eq!(view.focus, DashboardFocus::Envs);
        // Shift-Tab (BackTab) also cycles.
        assert!(matches!(
            view.on_key(press(KeyCode::BackTab)),
            DashboardAction::None
        ));
        assert_eq!(view.focus, DashboardFocus::Secrets);
    }

    #[test]
    fn jk_on_secrets_table_moves_selection() {
        let mut view = make_view(&[("prod", &["prod/A", "prod/B", "prod/C"])]);
        view.on_key(press(KeyCode::Tab));
        assert_eq!(view.focus, DashboardFocus::Secrets);
        assert_eq!(view.secret_state.selected(), Some(0));
        view.on_key(press(KeyCode::Char('j')));
        assert_eq!(view.secret_state.selected(), Some(1));
        view.on_key(press(KeyCode::Char('j')));
        assert_eq!(view.secret_state.selected(), Some(2));
        // wraps
        view.on_key(press(KeyCode::Char('j')));
        assert_eq!(view.secret_state.selected(), Some(0));
        view.on_key(press(KeyCode::Char('k')));
        assert_eq!(view.secret_state.selected(), Some(2));
        // env selection unchanged.
        assert_eq!(view.env_state.selected(), Some(0));
    }

    #[test]
    fn enter_on_secrets_focus_emits_open_viewer_with_payload() {
        let mut view = make_view(&[("prod", &["prod/A", "prod/B"])]);
        view.on_key(press(KeyCode::Tab));
        view.on_key(press(KeyCode::Char('j'))); // select prod/B
        match view.on_key(press(KeyCode::Enter)) {
            DashboardAction::OpenViewer(r) => {
                assert_eq!(r.path, "prod/B");
                assert_eq!(r.store, "test/store");
            }
            other => panic!("expected OpenViewer, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_envs_focus_is_swallowed() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert_eq!(view.focus, DashboardFocus::Envs);
        assert!(matches!(
            view.on_key(press(KeyCode::Enter)),
            DashboardAction::None
        ));
    }

    #[test]
    fn enter_on_empty_secrets_list_is_noop() {
        let mut view = make_view(&[("prod", &[])]);
        view.on_key(press(KeyCode::Tab));
        assert_eq!(view.focus, DashboardFocus::Secrets);
        assert!(matches!(
            view.on_key(press(KeyCode::Enter)),
            DashboardAction::None
        ));
    }

    #[test]
    fn switching_env_resets_secret_selection() {
        let mut view = make_view(&[
            ("prod", &["prod/A", "prod/B"]),
            ("staging", &["staging/X"]),
        ]);
        // Focus secrets, move to B.
        view.on_key(press(KeyCode::Tab));
        view.on_key(press(KeyCode::Char('j')));
        assert_eq!(view.secret_state.selected(), Some(1));
        // Switch back to envs and move down.
        view.on_key(press(KeyCode::Tab));
        assert_eq!(view.focus, DashboardFocus::Envs);
        view.on_key(press(KeyCode::Char('j')));
        assert_eq!(view.env_state.selected(), Some(1));
        // Secret selection reset to 0 so we don't dangle off the new list.
        assert_eq!(view.secret_state.selected(), Some(0));
    }

    // ── US-013: store picker routing ─────────────────────────────────────

    #[test]
    fn s_key_opens_store_picker_without_emitting_action() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert!(view.picker.is_none());
        let action = view.on_key(press(KeyCode::Char('s')));
        assert_eq!(action, DashboardAction::None);
        assert!(view.picker.is_some());
    }

    #[test]
    fn picker_esc_closes_overlay_and_swallows_action() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        view.on_key(press(KeyCode::Char('s')));
        assert!(view.picker.is_some());
        let action = view.on_key(press(KeyCode::Esc));
        assert_eq!(action, DashboardAction::None);
        assert!(view.picker.is_none());
    }

    #[test]
    fn picker_emits_switch_store_on_valid_selection() {
        // Build a real store checkout under a tempdir so the picker has
        // something valid to enumerate and select.
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("acme").join("secrets");
        std::fs::create_dir_all(store.join(".himitsu").join("secrets")).unwrap();

        let mut view = make_view(&[("prod", &["prod/A"])]);
        view.stores_dir = tmp.path().to_path_buf();

        // Open the picker and submit the first entry.
        view.on_key(press(KeyCode::Char('s')));
        assert!(view.picker.is_some());
        let action = view.on_key(press(KeyCode::Enter));
        assert_eq!(action, DashboardAction::SwitchStore(store));
        assert!(view.picker.is_none());
    }

    #[test]
    fn picker_intercepts_dashboard_keys_while_open() {
        let mut view = make_view(&[("prod", &["prod/A"]), ("staging", &["staging/B"])]);
        let selected_before = view.env_state.selected();
        view.on_key(press(KeyCode::Char('s')));
        // 'j' should now go to the picker, not the env list.
        view.on_key(press(KeyCode::Char('j')));
        assert_eq!(view.env_state.selected(), selected_before);
        // 'q' should also be intercepted (not quit).
        let action = view.on_key(press(KeyCode::Char('q')));
        assert_eq!(action, DashboardAction::None);
        assert!(view.picker.is_some());
    }
}
