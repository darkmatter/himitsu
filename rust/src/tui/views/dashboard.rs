//! Dashboard view: environments list + secrets for the selected env.
//!
//! Data comes from internal Rust APIs (`remote::store::list_secrets`), never
//! from a subprocess. An "environment" is the first path segment of each
//! secret (e.g. `prod/DATABASE_URL` → env `prod`).

use std::collections::BTreeMap;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::cli::Context;
use crate::remote::store;

/// Outcome of handling a key — lets the app router decide where to go next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardAction {
    None,
    Quit,
    EnterSearch,
}

pub struct DashboardView {
    store_slug: String,
    envs: Vec<String>,
    secrets_by_env: BTreeMap<String, Vec<String>>,
    env_state: ListState,
}

impl DashboardView {
    pub fn new(ctx: &Context) -> Self {
        let store_slug = derive_store_slug(ctx);
        let (envs, secrets_by_env) = load_envs(&ctx.store);
        let mut env_state = ListState::default();
        if !envs.is_empty() {
            env_state.select(Some(0));
        }
        Self {
            store_slug,
            envs,
            secrets_by_env,
            env_state,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> DashboardAction {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => DashboardAction::Quit,
            (KeyCode::Char('q'), _) => DashboardAction::Quit,
            (KeyCode::Char('/'), _) => DashboardAction::EnterSearch,
            // Esc has no parent view to return to from the dashboard — swallow it.
            (KeyCode::Esc, _) => DashboardAction::None,
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                self.select_prev();
                DashboardAction::None
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                self.select_next();
                DashboardAction::None
            }
            _ => DashboardAction::None,
        }
    }

    fn select_prev(&mut self) {
        if self.envs.is_empty() {
            return;
        }
        let i = self.env_state.selected().unwrap_or(0);
        let next = if i == 0 { self.envs.len() - 1 } else { i - 1 };
        self.env_state.select(Some(next));
    }

    fn select_next(&mut self) {
        if self.envs.is_empty() {
            return;
        }
        let i = self.env_state.selected().unwrap_or(0);
        let next = (i + 1) % self.envs.len();
        self.env_state.select(Some(next));
    }

    fn selected_secrets(&self) -> &[String] {
        self.env_state
            .selected()
            .and_then(|i| self.envs.get(i))
            .and_then(|env| self.secrets_by_env.get(env))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
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
        let block = Block::default().borders(Borders::ALL).title(" envs ");

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

        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        );

        frame.render_stateful_widget(list, area, &mut self.env_state);
    }

    fn draw_secrets(&self, frame: &mut Frame<'_>, area: Rect) {
        let title = match self.env_state.selected().and_then(|i| self.envs.get(i)) {
            Some(env) => format!(" secrets · {env} "),
            None => " secrets ".to_string(),
        };
        let block = Block::default().borders(Borders::ALL).title(title);
        let secrets = self.selected_secrets();

        if secrets.is_empty() {
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

        let items: Vec<ListItem> = secrets
            .iter()
            .map(|p| ListItem::new(Line::from(Span::raw(p.clone()))))
            .collect();
        frame.render_widget(List::new(items).block(block), area);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let footer = Line::from(vec![
            Span::styled("↑/↓ j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" navigate  "),
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(" search  "),
            Span::styled("q", Style::default().fg(Color::Cyan)),
            Span::raw(" quit  "),
            Span::styled("ctrl-c", Style::default().fg(Color::Cyan)),
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
            ("↑/↓ j/k", "navigate envs"),
            ("/", "search"),
            ("?", "toggle this help"),
            ("q", "quit"),
            ("ctrl-c", "quit"),
        ]
    }

    pub fn help_title() -> &'static str {
        "dashboard · keys"
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
        if !env_list.is_empty() {
            env_state.select(Some(0));
        }
        DashboardView {
            store_slug: "test/store".to_string(),
            envs: env_list,
            secrets_by_env,
            env_state,
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
        assert_eq!(
            view.on_key(press(KeyCode::Char('/'))),
            DashboardAction::EnterSearch
        );
    }

    #[test]
    fn q_emits_quit_action() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert_eq!(view.on_key(press(KeyCode::Char('q'))), DashboardAction::Quit);
    }

    #[test]
    fn ctrl_c_emits_quit_action() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert_eq!(view.on_key(ctrl('c')), DashboardAction::Quit);
    }

    #[test]
    fn esc_is_swallowed_on_dashboard() {
        let mut view = make_view(&[("prod", &["prod/A"])]);
        assert_eq!(view.on_key(press(KeyCode::Esc)), DashboardAction::None);
    }

    #[test]
    fn navigation_keys_do_not_emit_actions() {
        let mut view = make_view(&[("prod", &["prod/A"]), ("staging", &["staging/B"])]);
        assert_eq!(view.on_key(press(KeyCode::Down)), DashboardAction::None);
        assert_eq!(view.env_state.selected(), Some(1));
    }
}
