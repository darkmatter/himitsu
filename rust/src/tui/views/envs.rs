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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};

use super::{render_distributed_footer, standard_canvas};

use crate::tui::theme;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::cli::Context;
use crate::config::env_cache::Scope;
use crate::config::env_resolver::{self, EnvNode};
use crate::config::envs_mut::{self, ScopeHint};
use crate::config::{validate_env_label, EnvEntry};
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
    /// An env label was created or replaced.
    Created { label: String, scope: Scope },
    /// A create attempt failed; carries the message to surface as a toast.
    CreateFailed(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateFocus {
    Label,
    EntryKind,
    AliasKey,
    Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Single,
    Glob,
    Alias,
}

#[derive(Debug, Clone)]
enum EditorMode {
    Create,
    Edit {
        original_label: String,
        original_scope: Scope,
    },
}

impl EntryKind {
    fn next(self) -> Self {
        match self {
            Self::Single => Self::Glob,
            Self::Glob => Self::Alias,
            Self::Alias => Self::Single,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Single => Self::Alias,
            Self::Glob => Self::Single,
            Self::Alias => Self::Glob,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Glob => "Glob",
            Self::Alias => "Alias",
        }
    }
}

#[derive(Debug, Clone)]
struct CreateEditor {
    label: String,
    kind: EntryKind,
    alias_key: String,
    path: String,
    focus: CreateFocus,
    scope_hint: ScopeHint,
    mode: EditorMode,
}

impl Default for CreateEditor {
    fn default() -> Self {
        Self {
            label: String::new(),
            kind: EntryKind::Single,
            alias_key: String::new(),
            path: String::new(),
            focus: CreateFocus::Label,
            scope_hint: ScopeHint::Auto,
            mode: EditorMode::Create,
        }
    }
}

impl CreateEditor {
    fn from_existing(
        label: &str,
        scope: Scope,
        entries: &[EnvEntry],
    ) -> std::result::Result<Self, String> {
        if entries.len() != 1 {
            return Err(format!(
                "edit not yet supported for multi-entry envs ({} entries)",
                entries.len()
            ));
        }

        let mut editor = Self {
            label: label.to_string(),
            scope_hint: match scope {
                Scope::Project => ScopeHint::Project,
                Scope::Global => ScopeHint::Global,
            },
            mode: EditorMode::Edit {
                original_label: label.to_string(),
                original_scope: scope,
            },
            ..Default::default()
        };

        match &entries[0] {
            EnvEntry::Single(path) => {
                editor.kind = EntryKind::Single;
                editor.path = path.clone();
            }
            EnvEntry::Glob(prefix) => {
                editor.kind = EntryKind::Glob;
                editor.path = format!("{prefix}/*");
            }
            EnvEntry::Alias { key, path } => {
                editor.kind = EntryKind::Alias;
                editor.alias_key = key.clone();
                editor.path = path.clone();
            }
        }

        Ok(editor)
    }

    fn is_dirty(&self) -> bool {
        !self.label.is_empty() || !self.alias_key.is_empty() || !self.path.is_empty()
    }

    fn next_focus(&mut self) {
        self.focus = match (self.focus, self.kind) {
            (CreateFocus::Label, _) => CreateFocus::EntryKind,
            (CreateFocus::EntryKind, EntryKind::Alias) => CreateFocus::AliasKey,
            (CreateFocus::EntryKind, _) => CreateFocus::Path,
            (CreateFocus::AliasKey, _) => CreateFocus::Path,
            (CreateFocus::Path, _) => CreateFocus::Label,
        };
    }

    fn previous_focus(&mut self) {
        self.focus = match (self.focus, self.kind) {
            (CreateFocus::Label, _) => CreateFocus::Path,
            (CreateFocus::EntryKind, _) => CreateFocus::Label,
            (CreateFocus::AliasKey, _) => CreateFocus::EntryKind,
            (CreateFocus::Path, EntryKind::Alias) => CreateFocus::AliasKey,
            (CreateFocus::Path, _) => CreateFocus::EntryKind,
        };
    }

    fn input_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            CreateFocus::Label => Some(&mut self.label),
            CreateFocus::AliasKey => Some(&mut self.alias_key),
            CreateFocus::Path => Some(&mut self.path),
            CreateFocus::EntryKind => None,
        }
    }

    fn toggle_scope(&mut self) {
        self.scope_hint = match self.scope_hint {
            ScopeHint::Auto | ScopeHint::Project => ScopeHint::Global,
            ScopeHint::Global => ScopeHint::Auto,
        };
    }

    fn scope_label(&self) -> &'static str {
        match self.scope_hint {
            ScopeHint::Auto => "auto",
            ScopeHint::Project => "project",
            ScopeHint::Global => "global",
        }
    }

    fn title(&self) -> &'static str {
        match self.mode {
            EditorMode::Create => " new env ",
            EditorMode::Edit { .. } => " edit env ",
        }
    }

    fn validation_error(&self) -> Option<String> {
        if let Err(e) = validate_env_label(&self.label) {
            return Some(e.to_string());
        }
        match self.kind {
            EntryKind::Single => {
                if self.path.trim().is_empty() {
                    Some("secret path is required".into())
                } else {
                    None
                }
            }
            EntryKind::Glob => {
                if self.path.trim().is_empty() {
                    Some("glob prefix is required".into())
                } else {
                    None
                }
            }
            EntryKind::Alias => {
                if self.alias_key.trim().is_empty() {
                    Some("alias key is required".into())
                } else if self.path.trim().is_empty() {
                    Some("alias path is required".into())
                } else {
                    None
                }
            }
        }
    }

    fn entries(&self) -> Vec<EnvEntry> {
        let path = self.path.trim().trim_end_matches("/*").to_string();
        match self.kind {
            EntryKind::Single => vec![EnvEntry::Single(self.path.trim().to_string())],
            EntryKind::Glob => vec![EnvEntry::Glob(path)],
            EntryKind::Alias => vec![EnvEntry::Alias {
                key: self.alias_key.trim().to_string(),
                path: self.path.trim().to_string(),
            }],
        }
    }
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
    create: Option<CreateEditor>,
    confirm_cancel_create: bool,
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
            create: None,
            confirm_cancel_create: false,
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
        if self.confirm_cancel_create {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_cancel_create = false;
                    self.create = None;
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Enter => {
                    self.confirm_cancel_create = false;
                }
                _ => {}
            }
            return EnvsAction::None;
        }

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

        if self.create.is_some() {
            return self.handle_create_key(key, keymap);
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
            (KeyCode::Char('n'), _) => {
                self.create = Some(CreateEditor::default());
                EnvsAction::None
            }
            (KeyCode::Char('e'), _) => self.open_edit_selected(),
            _ => EnvsAction::None,
        }
    }

    fn open_edit_selected(&mut self) -> EnvsAction {
        let Some((label, scope)) = self
            .selected_label_scope()
            .map(|(label, scope)| (label.to_string(), scope))
        else {
            return EnvsAction::None;
        };
        let entries = self
            .entries
            .get(&(scope_key(scope), label.clone()))
            .cloned()
            .unwrap_or_default();
        match CreateEditor::from_existing(&label, scope, &entries) {
            Ok(editor) => {
                self.create = Some(editor);
                EnvsAction::None
            }
            Err(msg) => EnvsAction::CreateFailed(msg),
        }
    }

    fn handle_create_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> EnvsAction {
        if keymap.quit.matches(&key) && key.code != KeyCode::Esc {
            return EnvsAction::Quit;
        }

        if matches!(key.code, KeyCode::Char('s')) && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.perform_create();
        }

        let editor = self.create.as_mut().expect("create editor exists");
        match key.code {
            KeyCode::Esc => {
                if editor.is_dirty() {
                    self.confirm_cancel_create = true;
                } else {
                    self.create = None;
                }
                EnvsAction::None
            }
            KeyCode::Tab => {
                editor.next_focus();
                EnvsAction::None
            }
            KeyCode::BackTab => {
                editor.previous_focus();
                EnvsAction::None
            }
            KeyCode::Enter => {
                if editor.focus == CreateFocus::Label {
                    editor.next_focus();
                    EnvsAction::None
                } else {
                    self.perform_create()
                }
            }
            KeyCode::Left => {
                if editor.focus == CreateFocus::EntryKind {
                    editor.kind = editor.kind.previous();
                }
                EnvsAction::None
            }
            KeyCode::Right => {
                if editor.focus == CreateFocus::EntryKind {
                    editor.kind = editor.kind.next();
                }
                EnvsAction::None
            }
            KeyCode::Backspace => {
                if let Some(input) = editor.input_mut() {
                    input.pop();
                }
                EnvsAction::None
            }
            KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                editor.toggle_scope();
                EnvsAction::None
            }
            KeyCode::Char(c) => {
                if let Some(input) = editor.input_mut() {
                    input.push(c);
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

    fn perform_create(&mut self) -> EnvsAction {
        let Some(editor) = self.create.clone() else {
            return EnvsAction::None;
        };
        if let Some(msg) = editor.validation_error() {
            return EnvsAction::CreateFailed(format!("create failed: {msg}"));
        }

        let label = editor.label.trim().to_string();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match envs_mut::upsert(&label, editor.entries(), editor.scope_hint, &cwd) {
            Ok(resolved) => {
                if let EditorMode::Edit {
                    original_label,
                    original_scope,
                } = &editor.mode
                {
                    if original_label != &label || *original_scope != resolved.scope {
                        let original_hint = match original_scope {
                            Scope::Project => ScopeHint::Project,
                            Scope::Global => ScopeHint::Global,
                        };
                        if let Err(e) = envs_mut::delete(original_label, original_hint, &cwd) {
                            return EnvsAction::CreateFailed(format!(
                                "save failed while removing old label: {e}"
                            ));
                        }
                    }
                }
                self.create = None;
                self.confirm_cancel_create = false;
                self.reload();
                if let Some(i) = self.row_index_for(&label, resolved.scope) {
                    self.list_state.select(Some(i));
                }
                EnvsAction::Created {
                    label,
                    scope: resolved.scope,
                }
            }
            Err(e) => EnvsAction::CreateFailed(format!("create failed: {e}")),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = standard_canvas(frame.area());
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
            .constraints([Constraint::Max(20), Constraint::Min(1)])
            .split(chunks[1]);
        self.draw_labels(frame, panes[0]);
        if self.create.is_some() {
            let editor_panes = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
                .split(panes[1]);
            self.draw_create_editor(frame, editor_panes[0]);
            self.draw_live_preview(frame, editor_panes[1]);
        } else {
            self.draw_preview(frame, panes[1]);
        }

        self.draw_scope_status(frame, chunks[2]);
        self.draw_footer(frame, chunks[3]);

        if self.confirm.is_some() {
            self.draw_confirm(frame);
        }
        if self.confirm_cancel_create {
            self.draw_cancel_create_confirm(frame);
        }
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut spans = theme::brand_chip("秘 himitsu");
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "envs",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{} labels", self.label_count()),
            Style::default().fg(theme::muted()),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
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

        if self.label_count() == 0 {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "  No labels - create one with 'n'",
                    Style::default().fg(theme::muted()),
                ))),
                inner,
            );
            return;
        }

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
        if let Some(editor) = self.create.as_ref() {
            let text = format!(
                "creating: scope {} (ctrl-g toggles auto/global)",
                editor.scope_label()
            );
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    text,
                    Style::default().fg(theme::accent()),
                ))),
                area,
            );
            return;
        }

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
        let items = if self.create.is_some() {
            vec![
                Line::from(vec![
                    Span::styled("tab", Style::default().fg(theme::accent())),
                    Span::styled(" next field", footer),
                ]),
                Line::from(vec![
                    Span::styled("←/→", Style::default().fg(theme::accent())),
                    Span::styled(" kind", footer),
                ]),
                Line::from(vec![
                    Span::styled("ctrl-s", Style::default().fg(theme::accent())),
                    Span::styled(" save", footer),
                ]),
                Line::from(vec![
                    Span::styled("esc", Style::default().fg(theme::accent())),
                    Span::styled(" cancel", footer),
                ]),
            ]
        } else {
            vec![
                Line::from(vec![
                    Span::styled("↑/↓ / j/k", Style::default().fg(theme::accent())),
                    Span::styled(" navigate", footer),
                ]),
                Line::from(vec![
                    Span::styled("n", Style::default().fg(theme::accent())),
                    Span::styled(" new", footer),
                ]),
                Line::from(vec![
                    Span::styled("d", Style::default().fg(theme::accent())),
                    Span::styled(" delete", footer),
                ]),
                Line::from(vec![
                    Span::styled("?", Style::default().fg(theme::accent())),
                    Span::styled(" help", footer),
                ]),
                Line::from(vec![
                    Span::styled("esc", Style::default().fg(theme::accent())),
                    Span::styled(" back", footer),
                ]),
            ]
        };
        render_distributed_footer(frame, area, items);
    }

    fn draw_create_editor(&self, frame: &mut Frame<'_>, area: Rect) {
        let Some(editor) = self.create.as_ref() else {
            return;
        };

        let outer = Block::default()
            .borders(Borders::ALL)
            .title(editor.title())
            .title_style(Style::default().fg(theme::border_label()));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let focus_style = Style::default()
            .fg(theme::accent())
            .add_modifier(Modifier::BOLD);
        let label_style = |focus: bool| if focus { focus_style } else { Style::default() };
        let focus_prefix = "✦";
        let prefix = "✧";
        let label_span = |text: &'static str, focus: bool| {
            let marker = if focus { focus_prefix } else { prefix };
            Span::styled(format!("{marker} {text}"), label_style(focus))
        };
        let input_line = |value: &str, placeholder: &'static str| {
            if value.is_empty() {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(placeholder, Style::default().fg(theme::muted())),
                ])
            } else {
                Line::from(format!("  {value}"))
            }
        };

        let mut lines = vec![
            Line::from(label_span("Label", editor.focus == CreateFocus::Label)),
            input_line(&editor.label, "env label"),
            Line::from(Span::styled(
                "  e.g. dev or prod/* (segments: letters, numbers, _, -)",
                Style::default().fg(theme::muted()),
            )),
            Line::from(""),
            Line::from(vec![
                label_span("Entry kind", editor.focus == CreateFocus::EntryKind),
                Span::raw(": "),
                Span::styled(editor.kind.label(), Style::default().fg(theme::warning())),
            ]),
        ];

        if editor.kind == EntryKind::Alias {
            lines.extend([
                Line::from(""),
                Line::from(label_span(
                    "Alias key",
                    editor.focus == CreateFocus::AliasKey,
                )),
                input_line(&editor.alias_key, "alias key"),
            ]);
        }

        lines.extend([
            Line::from(""),
            Line::from(label_span(
                match editor.kind {
                    EntryKind::Single => "Secret path",
                    EntryKind::Glob => "Glob prefix",
                    EntryKind::Alias => "Alias path",
                },
                editor.focus == CreateFocus::Path,
            )),
            input_line(
                &editor.path,
                match editor.kind {
                    EntryKind::Single => "secret path",
                    EntryKind::Glob => "glob prefix",
                    EntryKind::Alias => "alias path",
                },
            ),
            Line::from(""),
        ]);

        if let Some(err) = editor.validation_error() {
            lines.push(Line::from(Span::styled(
                format!("  {err}"),
                Style::default().fg(theme::danger()),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  ready to save",
                Style::default().fg(theme::success()),
            )));
        }

        frame.render_widget(Paragraph::new(lines), inner);

        if let Some((input, row)) = match editor.focus {
            CreateFocus::Label => Some((editor.label.as_str(), 1)),
            CreateFocus::AliasKey if editor.kind == EntryKind::Alias => {
                Some((editor.alias_key.as_str(), 7))
            }
            CreateFocus::Path => Some((
                editor.path.as_str(),
                if editor.kind == EntryKind::Alias {
                    10
                } else {
                    7
                },
            )),
            CreateFocus::EntryKind | CreateFocus::AliasKey => None,
        } {
            if inner.width > 2 && row < inner.height {
                let input_width = input.chars().count() as u16;
                let x = inner
                    .x
                    .saturating_add(2)
                    .saturating_add(input_width)
                    .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
                frame.set_cursor_position((x, inner.y.saturating_add(row)));
            }
        }
    }

    fn draw_live_preview(&self, frame: &mut Frame<'_>, area: Rect) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .title(" live preview ")
            .title_style(Style::default().fg(theme::border_label()));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let Some(editor) = self.create.as_ref() else {
            return;
        };
        if let Some(err) = editor.validation_error() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("  {err}"),
                    Style::default().fg(theme::danger()),
                ))),
                inner,
            );
            return;
        }

        let label = editor.label.trim().to_string();
        let mut envs = BTreeMap::new();
        envs.insert(label.clone(), editor.entries());
        let lines = match env_resolver::resolve(&envs, &label, &self.available_secrets) {
            Ok(node) => render_node(&node, 0),
            Err(e) => vec![Line::from(Span::styled(
                format!("  error: {e}"),
                Style::default().fg(theme::danger()),
            ))],
        };
        frame.render_widget(Paragraph::new(lines), inner);
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

    fn draw_cancel_create_confirm(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(50, 20, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            " discard new env? ",
            Style::default()
                .fg(theme::border_label())
                .add_modifier(Modifier::BOLD),
        ));
        let text = vec![
            Line::from(""),
            Line::from("  Discard the new env form?"),
            Line::from(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("[y]", Style::default().fg(theme::danger())),
                Span::raw(" discard    "),
                Span::styled("[N]", Style::default().fg(theme::accent())),
                Span::raw(" keep editing"),
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
            ("n", "create a new env"),
            ("e", "edit selected env"),
            ("d", "delete selected env (confirm y/N)"),
            ("ctrl-s", "save new env while editing"),
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

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
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
    fn n_opens_create_editor() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));

        let act = view.on_key(key(KeyCode::Char('n')), &km);
        assert!(matches!(act, EnvsAction::None));
        assert!(view.create.is_some());
    }

    #[test]
    fn create_single_entry_saves_and_reloads() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));
        view.on_key(key(KeyCode::Char('n')), &km);
        view.create.as_mut().unwrap().label = "stage".into();
        view.create.as_mut().unwrap().path = "stage/API_KEY".into();

        let act = view.on_key(ctrl('s'), &km);
        match act {
            EnvsAction::Created { label, scope } => {
                assert_eq!(label, "stage");
                assert_eq!(scope, Scope::Project);
            }
            other => panic!("expected Created, got {other:?}"),
        }

        assert!(view.create.is_none());
        let entries = view
            .entries
            .get(&(scope_key(Scope::Project), "stage".to_string()))
            .expect("stage entries present");
        assert!(matches!(&entries[0], EnvEntry::Single(path) if path == "stage/API_KEY"));
    }

    #[test]
    fn create_escape_prompts_when_dirty() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));
        view.on_key(key(KeyCode::Char('n')), &km);
        view.create.as_mut().unwrap().label = "draft".into();

        let act = view.on_key(key(KeyCode::Esc), &km);
        assert!(matches!(act, EnvsAction::None));
        assert!(view.confirm_cancel_create);
        assert!(view.create.is_some());

        view.on_key(key(KeyCode::Char('y')), &km);
        assert!(view.create.is_none());
        assert!(!view.confirm_cancel_create);
    }

    #[test]
    fn edit_existing_env_replaces_entries() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));
        let act = view.on_key(key(KeyCode::Char('e')), &km);
        assert!(matches!(act, EnvsAction::None));

        let editor = view.create.as_mut().expect("editor open");
        assert!(matches!(editor.mode, EditorMode::Edit { .. }));
        assert_eq!(editor.label, "dev");
        editor.path = "dev/NEW_API".into();

        let act = view.on_key(ctrl('s'), &km);
        match act {
            EnvsAction::Created { label, scope } => {
                assert_eq!(label, "dev");
                assert_eq!(scope, Scope::Project);
            }
            other => panic!("expected Created, got {other:?}"),
        }

        let entries = view
            .entries
            .get(&(scope_key(Scope::Project), "dev".to_string()))
            .expect("dev entries present");
        assert!(matches!(&entries[0], EnvEntry::Single(path) if path == "dev/NEW_API"));
    }

    #[test]
    fn edit_label_change_relocates_row() {
        let home = Home::new();
        let _proj = seed_two_project_one_global(&home);
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));
        view.on_key(key(KeyCode::Char('e')), &km);

        let editor = view.create.as_mut().expect("editor open");
        editor.label = "stage".into();
        editor.path = "stage/API_KEY".into();

        let act = view.on_key(ctrl('s'), &km);
        assert!(matches!(act, EnvsAction::Created { .. }));

        let labels: Vec<&str> = view
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Label { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert!(!labels.contains(&"dev"));
        assert!(labels.contains(&"stage"));
        assert_eq!(view.selected_label_scope(), Some(("stage", Scope::Project)));
    }

    #[test]
    fn edit_multi_entry_env_returns_error() {
        let home = Home::new();
        let proj = seed_two_project_one_global(&home);
        std::fs::write(
            proj.join(".himitsu.yaml"),
            "envs:\n  multi:\n    - dev/API_KEY\n    - dev/DB_PASS\n",
        )
        .unwrap();
        let empty_store = home.path.join("empty-store");
        std::fs::create_dir_all(&empty_store).unwrap();

        let km = KeyMap::default();
        let mut view = EnvsView::new(&ctx_in(&empty_store));
        let act = view.on_key(key(KeyCode::Char('e')), &km);
        match act {
            EnvsAction::CreateFailed(msg) => assert!(msg.contains("multi-entry")),
            other => panic!("expected CreateFailed, got {other:?}"),
        }
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
