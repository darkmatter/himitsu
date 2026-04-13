//! Dashboard view: environments list + secrets for the selected env.
//!
//! Data comes from internal Rust APIs (`remote::store::list_secrets`), never
//! from a subprocess. An "environment" is the first path segment of each
//! secret (e.g. `prod/DATABASE_URL` → env `prod`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::cli::search::SearchResult;
use crate::cli::Context;
use crate::remote::store;
use crate::tui::views::store_picker::{StorePicker, StorePickerOutcome};

/// Which of the two lists has keyboard focus.
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
    /// Enter was pressed on a secret in the right-hand list — open the viewer.
    ///
    /// The payload mirrors [`crate::tui::views::search::SearchAction::OpenViewer`]
    /// so the app router can route both identically.
    OpenViewer(SearchResult),
    /// Rebuild the dashboard against a new store checkout at this path
    /// (US-013). The caller is responsible for constructing a fresh
    /// `Context` and dashboard view. The switch is in-memory only — no
    /// config file is written.
    SwitchStore(PathBuf),
}

pub struct DashboardView {
    store_slug: String,
    /// Absolute path to the store that backs this dashboard — needed so the
    /// viewer can decrypt / rekey the selected secret.
    store_path: PathBuf,
    envs: Vec<String>,
    secrets_by_env: BTreeMap<String, Vec<String>>,
    env_state: ListState,
    secret_state: ListState,
    focus: DashboardFocus,
    /// Cached `ctx.stores_dir()` so the store picker can enumerate checkouts
    /// without needing a live `Context` reference.
    stores_dir: PathBuf,
    /// Store picker overlay, `Some` when open.
    picker: Option<StorePicker>,
}

impl DashboardView {
    pub fn new(ctx: &Context) -> Self {
        let store_slug = derive_store_slug(ctx);
        let (envs, secrets_by_env) = load_envs(&ctx.store);
        let mut env_state = ListState::default();
        if !envs.is_empty() {
            env_state.select(Some(0));
        }
        let mut secret_state = ListState::default();
        // Pre-select the first secret of the first env so pressing Tab and
        // then Enter "just works" without an extra j/k.
        if let Some(first_env) = envs.first() {
            if secrets_by_env
                .get(first_env)
                .is_some_and(|v: &Vec<String>| !v.is_empty())
            {
                secret_state.select(Some(0));
            }
        }
        Self {
            store_slug,
            store_path: ctx.store.clone(),
            envs,
            secrets_by_env,
            env_state,
            secret_state,
            focus: DashboardFocus::Envs,
            stores_dir: ctx.stores_dir(),
            picker: None,
        }
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
            // Esc has no parent view to return to from the dashboard — swallow it.
            (KeyCode::Esc, _) => DashboardAction::None,
            (KeyCode::Tab, _) | (KeyCode::BackTab, _) => {
                self.toggle_focus();
                DashboardAction::None
            }
            (KeyCode::Right | KeyCode::Char('l'), _) => {
                // h/l also cycle focus — feels natural for vim users and
                // doesn't clash with any other dashboard binding.
                self.set_focus(DashboardFocus::Secrets);
                DashboardAction::None
            }
            (KeyCode::Left | KeyCode::Char('h'), _) => {
                self.set_focus(DashboardFocus::Envs);
                DashboardAction::None
            }
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                self.select_prev();
                DashboardAction::None
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                self.select_next();
                DashboardAction::None
            }
            (KeyCode::Enter, _) => self.on_enter(),
            // US-013: `s` opens the store picker overlay.
            (KeyCode::Char('s'), KeyModifiers::NONE) => {
                self.picker = Some(StorePicker::new(&self.stores_dir, self.store_path.clone()));
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
        let Some(secret_path) = self.selected_secret_path().map(str::to_string) else {
            return DashboardAction::None;
        };
        DashboardAction::OpenViewer(SearchResult {
            store: self.store_slug.clone(),
            store_path: self.store_path.clone(),
            path: secret_path,
            created_at: None,
        })
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
            let len = self.selected_secrets().len();
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
        let len = self.selected_secrets().len();
        if len == 0 {
            return;
        }
        let i = self.secret_state.selected().unwrap_or(0);
        let next = if i == 0 { len - 1 } else { i - 1 };
        self.secret_state.select(Some(next));
    }

    fn secret_next(&mut self) {
        let len = self.selected_secrets().len();
        if len == 0 {
            return;
        }
        let i = self.secret_state.selected().unwrap_or(0);
        let next = (i + 1) % len;
        self.secret_state.select(Some(next));
    }

    fn reset_secret_selection(&mut self) {
        let len = self.selected_secrets().len();
        if len == 0 {
            self.secret_state.select(None);
        } else {
            self.secret_state.select(Some(0));
        }
    }

    fn selected_secrets(&self) -> &[String] {
        self.env_state
            .selected()
            .and_then(|i| self.envs.get(i))
            .and_then(|env| self.secrets_by_env.get(env))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    fn selected_secret_path(&self) -> Option<&str> {
        let secrets = self.selected_secrets();
        self.secret_state
            .selected()
            .and_then(|i| secrets.get(i))
            .map(String::as_str)
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
                &self.store_slug,
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
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
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
        let secrets_len = self.selected_secrets().len();

        if secrets_len == 0 {
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

        let items: Vec<ListItem> = self
            .selected_secrets()
            .iter()
            .map(|p| ListItem::new(Line::from(Span::raw(p.clone()))))
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(highlight_style(focused))
            .style(body_style(focused));
        frame.render_stateful_widget(list, area, &mut self.secret_state);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let footer = Line::from(vec![
            Span::styled("↑/↓ j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" navigate  "),
            Span::styled("tab", Style::default().fg(Color::Cyan)),
            Span::raw(" focus  "),
            Span::styled("enter", Style::default().fg(Color::Cyan)),
            Span::raw(" open  "),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(" search  "),
            Span::styled("s", Style::default().fg(Color::Cyan)),
            Span::raw(" switch store  "),
            Span::styled("q", Style::default().fg(Color::Cyan)),
            Span::raw(" quit"),
        ]);
        frame.render_widget(Paragraph::new(footer), area);
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
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::Black)
    }
}

fn load_envs(store: &Path) -> (Vec<String>, BTreeMap<String, Vec<String>>) {
    if store.as_os_str().is_empty() {
        return (Vec::new(), BTreeMap::new());
    }

    let paths = store::list_secrets(store, None).unwrap_or_default();
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in paths {
        let env = match path.split_once('/') {
            Some((head, _)) if !head.is_empty() => head.to_string(),
            _ => continue,
        };
        map.entry(env).or_default().push(path);
    }
    for secrets in map.values_mut() {
        secrets.sort();
    }
    let envs: Vec<String> = map.keys().cloned().collect();
    (envs, map)
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

    fn make_view(envs: &[(&str, &[&str])]) -> DashboardView {
        let mut secrets_by_env = BTreeMap::new();
        let mut env_list: Vec<String> = Vec::new();
        for (env, secrets) in envs {
            env_list.push((*env).to_string());
            secrets_by_env.insert(
                (*env).to_string(),
                secrets.iter().map(|s| (*s).to_string()).collect(),
            );
        }
        let mut env_state = ListState::default();
        let mut secret_state = ListState::default();
        if !env_list.is_empty() {
            env_state.select(Some(0));
            if let Some(first) = env_list.first() {
                if secrets_by_env
                    .get(first)
                    .is_some_and(|v: &Vec<String>| !v.is_empty())
                {
                    secret_state.select(Some(0));
                }
            }
        }
        DashboardView {
            store_slug: "test/store".to_string(),
            store_path: PathBuf::from("/tmp/test/store"),
            envs: env_list,
            secrets_by_env,
            env_state,
            secret_state,
            focus: DashboardFocus::Envs,
            stores_dir: PathBuf::from("/tmp/himitsu-test-stores"),
            picker: None,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn load_envs_groups_by_first_segment() {
        let paths = vec![
            "prod/API_KEY".to_string(),
            "prod/DATABASE_URL".to_string(),
            "staging/API_KEY".to_string(),
            "bare_no_slash".to_string(),
        ];
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in paths {
            if let Some((head, _)) = path.split_once('/') {
                if !head.is_empty() {
                    map.entry(head.to_string()).or_default().push(path);
                }
            }
        }
        let envs: Vec<String> = map.keys().cloned().collect();
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
    fn selected_secrets_updates_with_selection() {
        let mut view = make_view(&[("prod", &["prod/A", "prod/B"]), ("staging", &["staging/X"])]);
        assert_eq!(view.selected_secrets().len(), 2);
        view.select_next();
        assert_eq!(view.selected_secrets(), &["staging/X".to_string()]);
    }

    #[test]
    fn empty_view_has_no_selection() {
        let view = make_view(&[]);
        assert_eq!(view.env_state.selected(), None);
        assert!(view.selected_secrets().is_empty());
    }

    #[test]
    fn navigation_on_empty_view_is_noop() {
        let mut view = make_view(&[]);
        view.select_next();
        view.select_prev();
        assert_eq!(view.env_state.selected(), None);
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
    fn jk_on_secrets_list_moves_secret_selection() {
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
                assert_eq!(r.store_path, PathBuf::from("/tmp/test/store"));
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
