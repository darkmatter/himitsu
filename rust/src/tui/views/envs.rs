//! Envs view: browse and delete preset envs (project + global scope).
//!
//! Two-pane layout: the left pane lists env labels grouped by scope (project
//! first, then global); the right pane renders the resolved [`EnvNode`] tree
//! for whichever label is selected. Deletion goes through
//! [`crate::config::envs_mut::delete`] with a scope hint derived from which
//! section the label lives in, so a delete is never ambiguous.
//!
//! v1 is intentionally **read-only + delete**. Creation and inline editing are
//! tracked as follow-up issues and not wired here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};

use crate::tui::theme;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::cli::Context;
use crate::config::env_cache::Scope;
use crate::config::env_resolver::{self, EnvNode};
use crate::config::envs_mut::{self, ScopeHint};
use crate::config::EnvEntry;
use crate::remote::store;
use crate::tui::keymap::{Bindings, KeyMap};

/// Outcome of handling a key — routed by the app.
#[derive(Debug, Clone)]
pub enum EnvsAction {
    /// Stay in the envs view.
    None,
    /// Return to the search view (Esc).
    Back,
    /// Ctrl-C pressed — propagate a quit to the app.
    Quit,
    /// A label was deleted. Carries `(label, scope)` so the router can emit
    /// a toast reflecting which scope the label lived in.
    Deleted { label: String, scope: Scope },
    /// A delete attempt failed; carries the message to surface as a toast.
    DeleteFailed(String),
}

/// One row in the left pane: either a section header (scope grouping) or a
/// selectable label row belonging to a scope.
#[derive(Debug, Clone)]
enum Row {
    /// Non-selectable scope-header row (e.g. `Project` / `Global`).
    Header { scope: Scope },
    /// Selectable env label owned by `scope`.
    Label { label: String, scope: Scope },
}

/// Confirmation modal state for a pending delete.
struct ConfirmDelete {
    label: String,
    scope: Scope,
}

pub struct EnvsView {
    ctx: Context,
    rows: Vec<Row>,
    /// Map of `(scope, label)` → resolved entries, for the right-pane preview.
    /// Rebuilt every time we reload from disk.
    entries: BTreeMap<(u8, String), Vec<EnvEntry>>,
    list_state: ListState,
    confirm: Option<ConfirmDelete>,
    /// Project config path, if any — displayed in the status bar so the user
    /// can see exactly which file would be touched by a delete.
    project_config_path: Option<PathBuf>,
    /// Cached list of available secret paths in the active store. Fed to the
    /// resolver so wildcard envs expand correctly in the preview.
    available_secrets: Vec<String>,
}

impl EnvsView {
    pub fn new(ctx: &Context) -> Self {
        let ctx_owned = clone_ctx(ctx);
        let mut view = Self {
            ctx: ctx_owned,
            rows: Vec::new(),
            entries: BTreeMap::new(),
            list_state: ListState::default(),
            confirm: None,
            project_config_path: None,
            available_secrets: Vec::new(),
        };
        view.reload();
        view
    }

    /// Refresh the left-pane rows from on-disk YAML. Preserves the current
    /// selection when possible.
    fn reload(&mut self) {
        let prev = self.selected_label_scope().map(|(l, s)| (l.to_string(), s));

        self.rows.clear();
        self.entries.clear();

        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        // Probe project scope via `Auto` so we don't error when only a global
        // config is present. If Auto resolves to Project we keep those rows;
        // otherwise we fall through and just render global.
        let (project_rows, project_path) = match envs_mut::read(ScopeHint::Auto, &cwd) {
            Ok((resolved, envs)) if resolved.scope == Scope::Project => {
                let path = resolved.config_path.clone();
                let mut rows: Vec<(String, Vec<EnvEntry>)> = envs.into_iter().collect();
                rows.sort_by(|a, b| a.0.cmp(&b.0));
                (rows, Some(path))
            }
            _ => (Vec::new(), None),
        };
        self.project_config_path = project_path.clone();

        if project_path.is_some() {
            self.rows.push(Row::Header {
                scope: Scope::Project,
            });
            for (label, entries) in project_rows {
                self.entries
                    .insert((scope_key(Scope::Project), label.clone()), entries);
                self.rows.push(Row::Label {
                    label,
                    scope: Scope::Project,
                });
            }
        }

        // Global: read is always safe. An empty map is fine — we still emit
        // the header so the user understands the scope exists.
        if let Ok((_resolved, envs)) = envs_mut::read(ScopeHint::Global, &cwd) {
            self.rows.push(Row::Header {
                scope: Scope::Global,
            });
            let mut rows: Vec<(String, Vec<EnvEntry>)> = envs.into_iter().collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            for (label, entries) in rows {
                self.entries
                    .insert((scope_key(Scope::Global), label.clone()), entries);
                self.rows.push(Row::Label {
                    label,
                    scope: Scope::Global,
                });
            }
        }

        // Available secrets for resolver preview. A missing / empty store is
        // not fatal — concrete entries still render, wildcards just produce
        // an empty branch.
        self.available_secrets = store::list_secrets(&self.ctx.store, None).unwrap_or_default();

        // Try to restore the prior selection; otherwise land on the first
        // selectable label.
        let restored = prev.and_then(|(l, s)| self.row_index_for(&l, s));
        self.list_state
            .select(restored.or_else(|| self.first_selectable()));
    }

    fn first_selectable(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Label { .. }))
    }

    fn row_index_for(&self, label: &str, scope: Scope) -> Option<usize> {
        self.rows.iter().position(|r| match r {
            Row::Label { label: l, scope: s } => l == label && *s == scope,
            _ => false,
        })
    }

    fn is_selectable(&self, i: usize) -> bool {
        matches!(self.rows.get(i), Some(Row::Label { .. }))
    }

    fn selected_label_scope(&self) -> Option<(&str, Scope)> {
        self.list_state
            .selected()
            .and_then(|i| self.rows.get(i))
            .and_then(|r| match r {
                Row::Label { label, scope } => Some((label.as_str(), *scope)),
                _ => None,
            })
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

    pub fn on_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> EnvsAction {
        // Confirmation modal intercepts every key while open.
        if let Some(pending) = self.confirm.as_ref() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let label = pending.label.clone();
                    let scope = pending.scope;
                    self.confirm = None;
                    return self.perform_delete(label, scope);
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Enter => {
                    self.confirm = None;
                    return EnvsAction::None;
                }
                _ => return EnvsAction::None,
            }
        }

        // Ctrl-C / quit binding maps to Quit; Esc is Back (not Quit) because
        // envs is a sub-view under search, not the root.
        if key.code == KeyCode::Esc || matches!(key.code, KeyCode::Char('q')) {
            return EnvsAction::Back;
        }
        // Ctrl-C via the configured quit binding still propagates.
        if keymap.quit.matches(&key) && key.code != KeyCode::Esc {
            return EnvsAction::Quit;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                self.select_prev();
                EnvsAction::None
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                self.select_next();
                EnvsAction::None
            }
            (KeyCode::Char('d'), _) => {
                if let Some((label, scope)) = self.selected_label_scope() {
                    self.confirm = Some(ConfirmDelete {
                        label: label.to_string(),
                        scope,
                    });
                }
                EnvsAction::None
            }
            _ => EnvsAction::None,
        }
    }

    /// Execute a confirmed delete through [`envs_mut::delete`] and reload.
    fn perform_delete(&mut self, label: String, scope: Scope) -> EnvsAction {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let hint = match scope {
            Scope::Project => ScopeHint::Project,
            Scope::Global => ScopeHint::Global,
        };
        match envs_mut::delete(&label, hint, &cwd) {
            Ok(_) => {
                self.reload();
                EnvsAction::Deleted { label, scope }
            }
            Err(e) => EnvsAction::DeleteFailed(format!("delete failed: {e}")),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);

        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(chunks[1]);
        self.draw_labels(frame, panes[0]);
        self.draw_preview(frame, panes[1]);

        self.draw_scope_status(frame, chunks[2]);
        self.draw_footer(frame, chunks[3]);

        if self.confirm.is_some() {
            self.draw_confirm(frame);
        }
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let header = Line::from(vec![
            Span::styled(
                " himitsu ",
                Style::default()
                    .fg(theme::on_accent())
                    .bg(theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled("envs", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                format!("{} labels", self.label_count()),
                Style::default().fg(theme::muted()),
            ),
        ]);
        frame.render_widget(Paragraph::new(header), area);
    }

    fn label_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r, Row::Label { .. }))
            .count()
    }

    fn draw_labels(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .title(" labels ")
            .title_style(Style::default().fg(theme::border_label()));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        if self.rows.is_empty() {
            let msg = "  no env presets defined";
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    msg,
                    Style::default().fg(theme::muted()),
                ))),
                inner,
            );
            return;
        }

        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|r| match r {
                Row::Header { scope } => ListItem::new(Line::from(Span::styled(
                    format!(
                        "■ {}",
                        match scope {
                            Scope::Project => "Project",
                            Scope::Global => "Global",
                        }
                    ),
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD),
                ))),
                Row::Label { label, .. } => {
                    ListItem::new(Line::from(vec![Span::raw("  "), Span::raw(label.clone())]))
                }
            })
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme::accent())
                .fg(theme::on_accent())
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, inner, &mut self.list_state);
    }

    fn draw_preview(&self, frame: &mut Frame<'_>, area: Rect) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .title(" preview ")
            .title_style(Style::default().fg(theme::border_label()));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let Some((label, scope)) = self.selected_label_scope() else {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "  select a label",
                    Style::default().fg(theme::muted()),
                ))),
                inner,
            );
            return;
        };

        // Build a single-entry map so the resolver can operate without the
        // full on-disk context. We already hold the entries in memory.
        let mut envs = BTreeMap::new();
        if let Some(entries) = self.entries.get(&(scope_key(scope), label.to_string())) {
            envs.insert(label.to_string(), entries.clone());
        }

        let lines: Vec<Line> = match env_resolver::resolve(&envs, label, &self.available_secrets) {
            Ok(node) => render_node(&node, 0),
            Err(e) => vec![Line::from(Span::styled(
                format!("  error: {e}"),
                Style::default().fg(theme::danger()),
            ))],
        };
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_scope_status(&self, frame: &mut Frame<'_>, area: Rect) {
        let (text, color) = match self.selected_label_scope() {
            Some((_, Scope::Project)) => (
                format!(
                    "scope: project ({})",
                    self.project_config_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| ".himitsu.yaml".to_string())
                ),
                theme::accent(),
            ),
            Some((_, Scope::Global)) => ("scope: global".to_string(), theme::accent()),
            None => ("scope: —".to_string(), theme::muted()),
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, Style::default().fg(color)))),
            area,
        );
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let footer = Style::default().fg(theme::footer_text());
        let line = Line::from(vec![
            Span::styled("↑/↓ / j/k", Style::default().fg(theme::accent())),
            Span::styled(" navigate    ", footer),
            Span::styled("d", Style::default().fg(theme::accent())),
            Span::styled(" delete    ", footer),
            Span::styled("?", Style::default().fg(theme::accent())),
            Span::styled(" help    ", footer),
            Span::styled("esc", Style::default().fg(theme::accent())),
            Span::styled(" back", footer),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_confirm(&self, frame: &mut Frame<'_>) {
        let Some(pending) = self.confirm.as_ref() else {
            return;
        };
        let area = centered_rect(50, 20, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            " confirm delete ",
            Style::default()
                .fg(theme::border_label())
                .add_modifier(Modifier::BOLD),
        ));
        let scope_str = match pending.scope {
            Scope::Project => "project",
            Scope::Global => "global",
        };
        let text = vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("  Delete "),
                Span::styled(
                    format!("`{}`", pending.label),
                    Style::default()
                        .fg(theme::warning())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" from {scope_str} scope?")),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("[y]", Style::default().fg(theme::danger())),
                Span::raw(" yes    "),
                Span::styled("[N]", Style::default().fg(theme::accent())),
                Span::raw(" cancel"),
            ]),
        ];
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(text), inner);
    }
}

/// Render an `EnvNode` tree as an indented list of `Line`s.
///
/// Branches show their key; leaves show `key = secret_path`. A purely empty
/// root branch (no children) renders a single dim `(empty)` line so the
/// preview pane is never blank on an unresolved wildcard.
fn render_node(node: &EnvNode, depth: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    match node {
        EnvNode::Leaf { secret_path } => {
            out.push(Line::from(vec![
                Span::raw("  ".repeat(depth + 1)),
                Span::styled("→ ", Style::default().fg(theme::muted())),
                Span::raw(secret_path.clone()),
            ]));
        }
        EnvNode::Branch(children) => {
            if depth == 0 && children.is_empty() {
                out.push(Line::from(Span::styled(
                    "  (empty)",
                    Style::default().fg(theme::muted()),
                )));
                return out;
            }
            for (key, child) in children {
                match child {
                    EnvNode::Leaf { secret_path } => {
                        out.push(Line::from(vec![
                            Span::raw("  ".repeat(depth + 1)),
                            Span::styled(
                                key.clone(),
                                Style::default()
                                    .fg(theme::accent())
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(" = ", Style::default().fg(theme::muted())),
                            Span::raw(secret_path.clone()),
                        ]));
                    }
                    EnvNode::Branch(_) => {
                        out.push(Line::from(vec![
                            Span::raw("  ".repeat(depth + 1)),
                            Span::styled(
                                format!("{key}/"),
                                Style::default()
                                    .fg(theme::warning())
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                        out.extend(render_node(child, depth + 1));
                    }
                }
            }
        }
    }
    out
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let v = Layout::default()
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
        .split(v[1])[1]
}

fn scope_key(scope: Scope) -> u8 {
    match scope {
        Scope::Project => 0,
        Scope::Global => 1,
    }
}

fn clone_ctx(ctx: &Context) -> Context {
    Context {
        data_dir: ctx.data_dir.clone(),
        state_dir: ctx.state_dir.clone(),
        store: ctx.store.clone(),
        recipients_path: ctx.recipients_path.clone(),
    }
}

// ── Help overlay integration ─────────────────────────────────────────────

impl EnvsView {
    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("↑/↓ / j/k", "navigate labels"),
            ("d", "delete selected env (confirm y/N)"),
            ("?", "toggle this help"),
            ("esc / q", "back to search"),
            ("ctrl-c", "quit"),
        ]
    }

    pub fn help_title() -> &'static str {
        "envs · keys"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    use tempfile::TempDir;

    // envs_mut tests serialize HIMITSU_CONFIG because it's process-global.
    // Use the same lock here so our fixtures don't stomp on their runs or
    // each other — see `crate::config::envs_mut::HIMITSU_CONFIG_TEST_GUARD`.
    use crate::config::envs_mut::HIMITSU_CONFIG_TEST_GUARD as ENV_GUARD;

    struct Home {
        _guard: std::sync::MutexGuard<'static, ()>,
        _tmp: TempDir,
        path: PathBuf,
        _orig_cwd: PathBuf,
    }

    impl Home {
        fn new() -> Self {
            let guard = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            std::env::set_var("HIMITSU_CONFIG", tmp.path().join("config.yaml"));
            let path = tmp.path().to_path_buf();
            let orig_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            Self {
                _guard: guard,
                _tmp: tmp,
                path,
                _orig_cwd: orig_cwd,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            // Restore cwd first so other tests don't inherit a deleted dir.
            let _ = std::env::set_current_dir(&self._orig_cwd);
            std::env::remove_var("HIMITSU_CONFIG");
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctx_in(store: &std::path::Path) -> Context {
        Context {
            data_dir: PathBuf::new(),
            state_dir: PathBuf::new(),
            store: store.to_path_buf(),
            recipients_path: None,
        }
    }

    /// Seed a project config + a global config with known labels so the view
    /// has a deterministic fixture to render.
    fn seed_two_project_one_global(home: &Home) -> PathBuf {
        // Project config: two labels (`dev`, `prod`).
        let proj = home.path.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let proj_cfg = proj.join(".himitsu.yaml");
        std::fs::write(
            &proj_cfg,
            "envs:\n  dev:\n    - dev/API_KEY\n  prod:\n    - prod/API_KEY\n",
        )
        .unwrap();

        // Global config: one label (`shared`).
        let global_cfg = crate::config::config_path();
        std::fs::create_dir_all(global_cfg.parent().unwrap()).unwrap();
        std::fs::write(&global_cfg, "envs:\n  shared:\n    - shared/TOKEN\n").unwrap();

        // Chdir into the project so `read(Auto, cwd)` finds the project cfg.
        std::env::set_current_dir(&proj).unwrap();
        proj
    }

    #[test]
    fn renders_project_and_global_sections() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let view = EnvsView::new(&ctx_in(&empty_store));

        // Expect: [Header(Project), Label(dev), Label(prod), Header(Global), Label(shared)]
        assert_eq!(view.rows.len(), 5);
        assert!(matches!(
            &view.rows[0],
            Row::Header {
                scope: Scope::Project
            }
        ));
        assert!(
            matches!(&view.rows[1], Row::Label { label, scope: Scope::Project } if label == "dev")
        );
        assert!(
            matches!(&view.rows[2], Row::Label { label, scope: Scope::Project } if label == "prod")
        );
        assert!(matches!(
            &view.rows[3],
            Row::Header {
                scope: Scope::Global
            }
        ));
        assert!(
            matches!(&view.rows[4], Row::Label { label, scope: Scope::Global } if label == "shared")
        );

        // First selectable is the project `dev` label at row 1.
        assert_eq!(view.list_state.selected(), Some(1));
        assert_eq!(view.label_count(), 3);

        // Preview contents: entries for `dev` were loaded into the entries map.
        let dev = view
            .entries
            .get(&(scope_key(Scope::Project), "dev".to_string()))
            .expect("dev entries present");
        assert_eq!(dev.len(), 1);
    }

    #[test]
    fn d_then_y_invokes_delete_and_reloads() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));

        // Selection starts at `dev` (row 1). Press `d` → confirm modal opens.
        assert!(view.confirm.is_none());
        let act = view.on_key(key(KeyCode::Char('d')), &km);
        assert!(matches!(act, EnvsAction::None));
        let pending = view.confirm.as_ref().expect("confirm modal should be open");
        assert_eq!(pending.label, "dev");
        assert_eq!(pending.scope, Scope::Project);

        // Press `y` → delete fires, view reloads, Deleted action carries
        // the (label, scope) pair.
        let act = view.on_key(key(KeyCode::Char('y')), &km);
        match act {
            EnvsAction::Deleted { label, scope } => {
                assert_eq!(label, "dev");
                assert_eq!(scope, Scope::Project);
            }
            other => panic!("expected Deleted, got {other:?}"),
        }

        // After reload, `dev` should be gone but `prod` + `shared` remain.
        let labels: Vec<&str> = view
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Label { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert!(!labels.contains(&"dev"));
        assert!(labels.contains(&"prod"));
        assert!(labels.contains(&"shared"));
    }

    #[test]
    fn d_then_n_cancels_delete() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));
        view.on_key(key(KeyCode::Char('d')), &km);
        assert!(view.confirm.is_some());
        let act = view.on_key(key(KeyCode::Char('n')), &km);
        assert!(matches!(act, EnvsAction::None));
        assert!(view.confirm.is_none());

        // dev is still present on disk.
        let labels: Vec<&str> = view
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Label { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"dev"));
    }

    #[test]
    fn esc_emits_back() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));
        assert!(matches!(
            view.on_key(key(KeyCode::Esc), &km),
            EnvsAction::Back
        ));
    }

    #[test]
    fn navigation_skips_headers() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));
        // Walk the entire row list twice; every landing must be a Label.
        for _ in 0..view.rows.len() * 2 {
            view.on_key(key(KeyCode::Down), &km);
            let sel = view.list_state.selected().unwrap();
            assert!(
                matches!(view.rows[sel], Row::Label { .. }),
                "Down landed on non-label row {sel}"
            );
        }
    }
}
