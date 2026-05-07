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
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};

use super::{render_distributed_footer, standard_canvas};

use crate::tui::theme;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use chrono::Utc;

use crate::cli::search::{humanize_age_compact, parse_ts, search_core, SearchResult};
use crate::cli::Context;
use crate::crypto::{age, secret_value};
use crate::remote::store;
use crate::tui::icons;
use crate::tui::keymap::{KeyAction, KeyMap};
use crate::tui::views::command_palette::{Command, CommandPalette, CommandPaletteOutcome};
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
    /// User picked "add remote" from the command palette — open the
    /// protobuf-driven add-remote form.
    AddRemote,
    /// User requested the envs view (Shift+E) — browse/delete preset envs.
    OpenEnvs,
    /// User picked a new active store via the embedded picker overlay.
    SwitchStore(PathBuf),
    /// User picked "show help" from the command palette — the router
    /// should open the contextual help overlay just like pressing `?`.
    ShowHelp,
    /// User pressed Esc / Ctrl-C — root view, so quit the app.
    Quit,
    /// User pressed Ctrl+Y and we successfully copied the selected secret
    /// value to the clipboard. Carries the secret path for the toast.
    Copied(String),
    /// Ctrl+Y attempted but failed (no selection / decrypt error / no
    /// clipboard backend). Carries a human-readable error string.
    CopyFailed(String),
    /// User triggered sync from the command palette. Carries a result
    /// message for the toast.
    Synced(String),
    /// User triggered rekey from the command palette.
    Rekeyed(String),
    /// User triggered join from the command palette.
    Joined(String),
    /// A palette command failed. Carries the error for the toast.
    CommandFailed(String),
    /// A palette command without a TUI form yet — surface the equivalent
    /// CLI invocation as an info toast.
    CommandHint(String),
}

/// A row in the rendered results list. `Store` headers group secrets by
/// origin (`org/repo` slug or local path) and are never selectable;
/// navigation steps over them. `FoldedGroup` rows appear only in folded mode,
/// one per top-level path prefix shared by ≥ 2 secrets — they collapse the
/// group's leaves into a single selectable row that expands when the user
/// unfolds.
#[derive(Debug, Clone)]
enum Row {
    Store {
        name: String,
        count: usize,
    },
    FoldedGroup {
        /// Top-level path segment shared by the collapsed leaves.
        prefix: String,
        /// Number of leaves under this prefix.
        count: usize,
        /// Indentation depth (matches what its children would have if
        /// expanded). 0 in single-store mode, 1 under a `Store` header.
        indent: usize,
    },
    Secret {
        result: SearchResult,
        /// Indentation depth in list-item cells (2 spaces per level).
        /// 0 in single-store mode, 1 under a `Store` header.
        indent: usize,
        /// Top-level path segment when this secret shares a prefix with
        /// ≥ 1 sibling in the same store. The renderer paints this segment
        /// with a subtle accent so the visual grouping survives without a
        /// separate header row. `None` for singletons.
        shared_prefix: Option<String>,
    },
}

/// Health status of the active store's git checkout, computed once at view
/// construction. Displayed as a compact indicator in the header bar.
#[derive(Debug, Clone)]
enum StoreHealth {
    /// Store checkout is up to date with its remote tracking branch.
    Synced,
    /// Local checkout is behind its remote by N commit(s).
    Behind(u32),
    /// Working tree has uncommitted local changes.
    Dirty,
    /// Both behind remote AND has local changes.
    BehindAndDirty(u32),
    /// Store directory is not a git repo.
    NotGit,
    /// Git repo exists but has no remote configured.
    NoRemote,
    /// Git repo has a remote but the tracking branch doesn't exist yet
    /// (e.g. never pushed).
    NotPushed,
    /// User's own age key is not in the store's recipient list.
    NotRecipient,
    /// Could not determine status for some other reason.
    Unknown,
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
    /// Embedded command-palette overlay. When `Some`, it intercepts every
    /// key just like the store picker. Mutually exclusive with `picker`
    /// because both are modal popups.
    palette: Option<CommandPalette>,
    /// Health of the active store's git checkout, checked once at startup.
    store_health: StoreHealth,
    /// Whether to render the STORE column in the results table. Off by
    /// default — most users work in a single store at a time, so the
    /// column is dead weight. Toggled via the command palette
    /// ("toggle store column"). When the table groups results by store
    /// (multi-store searches) the column is hidden regardless because the
    /// store name is already in a group header row.
    show_store_column: bool,
    /// Map of secret path → list of env labels that reference it. Built
    /// once at view-construction time from the project + global configs.
    /// Used to render the ENVS column in the results table.
    env_index: std::collections::HashMap<String, Vec<String>>,
    /// When true, multi-leaf top-level prefix groups collapse to a single
    /// `FoldedGroup` row. Toggled with Tab. Singleton paths render the same
    /// in both states. Default: unfolded.
    folded: bool,
}

impl SearchView {
    pub fn new(ctx: &Context) -> Self {
        let ctx_owned = Context {
            data_dir: ctx.data_dir.clone(),
            state_dir: ctx.state_dir.clone(),
            store: ctx.store.clone(),
            recipients_path: ctx.recipients_path.clone(),
        };
        let store_health = check_store_health(&ctx_owned);
        let env_index = build_env_index();
        let mut view = Self {
            query: String::new(),
            results: Vec::new(),
            rows: Vec::new(),
            list_state: ListState::default(),
            ctx: ctx_owned,
            picker: None,
            palette: None,
            store_health,
            show_store_column: false,
            env_index,
            folded: false,
        };
        view.refresh_results();
        view
    }

    pub fn on_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> SearchAction {
        // Palette overlay swallows every key while open.
        if let Some(palette) = self.palette.as_mut() {
            match palette.on_key(key) {
                CommandPaletteOutcome::Pending => return SearchAction::None,
                CommandPaletteOutcome::Cancelled => {
                    self.palette = None;
                    return SearchAction::None;
                }
                CommandPaletteOutcome::Selected(cmd) => {
                    self.palette = None;
                    return self.dispatch_command(cmd);
                }
            }
        }

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
        // All matches route through `dispatch_action` so leader-key chord
        // completions (resolved at the App layer) take the same code path.
        if let Some(action) = match_keymap_action(keymap, &key) {
            if let Some(outcome) = self.dispatch_action(action) {
                return outcome;
            }
        }

        match (key.code, key.modifiers) {
            (KeyCode::Tab, _) => {
                self.toggle_fold();
                SearchAction::None
            }
            (KeyCode::Enter, _) => {
                // On a folded group, Enter expands the entire view (1-level
                // unfold) and lands the cursor on the first leaf of the
                // group the user just opened.
                if let Some(prefix) = self.selected_folded_prefix() {
                    self.unfold_to_prefix(&prefix);
                    return SearchAction::None;
                }
                match self.selected_result().cloned() {
                    Some(r) => SearchAction::OpenViewer(r),
                    None => SearchAction::None,
                }
            }
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

    /// Run a [`KeyAction`] against the search view. Returns `None` for
    /// actions this view doesn't own (so the caller can fall through to
    /// raw-key handling), `Some(SearchAction::None)` for actions that are
    /// consumed but produce no router work (overlay opens, etc.), and
    /// other variants for outcomes the router needs to surface.
    ///
    /// Used both by the single-key matcher in `on_key` and by the leader-
    /// key dispatcher in `App::on_key` when a multi-step chord completes.
    pub fn dispatch_action(&mut self, action: KeyAction) -> Option<SearchAction> {
        match action {
            KeyAction::Quit => Some(SearchAction::Quit),
            KeyAction::CommandPalette => {
                self.palette = Some(CommandPalette::new());
                Some(SearchAction::None)
            }
            KeyAction::NewSecret => Some(SearchAction::NewSecret),
            KeyAction::Envs => Some(SearchAction::OpenEnvs),
            KeyAction::SwitchStore => {
                self.picker = Some(StorePicker::new(
                    &self.ctx.stores_dir(),
                    self.ctx.store.clone(),
                ));
                Some(SearchAction::None)
            }
            KeyAction::CopySelected => Some(self.copy_selected_to_clipboard()),
            KeyAction::CopyRefSelected => Some(self.copy_selected_ref_to_clipboard()),
            _ => None,
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

    /// Copy `himitsu read <ref>` for the selected row to the clipboard. No
    /// decryption — just the path, suitable for pasting into a terminal,
    /// pull request, or chat message without putting plaintext on the
    /// clipboard. The ref is qualified with `-r <store>` when the active
    /// store differs from the selected row's store, so the command works
    /// from a different shell.
    fn copy_selected_ref_to_clipboard(&mut self) -> SearchAction {
        let Some(result) = self.selected_result().cloned() else {
            return SearchAction::CopyFailed("no selection to copy".to_string());
        };
        let active_label = crate::cli::search::store_label(&self.ctx.store, &self.ctx);
        let cmd = format_read_command(&result.store, &result.path, &active_label);
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(cmd.clone())) {
            Ok(()) => SearchAction::Copied(format!("$ {cmd}")),
            Err(e) => SearchAction::CopyFailed(format!("clipboard unavailable: {e}")),
        }
    }

    fn refresh_results(&mut self) {
        // Pass an empty tag filter; the TUI handles tag chips/filtering in a
        // separate worker so this view always asks for everything.
        self.results = search_core(&self.ctx, &self.query, &[]).unwrap_or_default();
        self.rows = build_rows(&self.results, self.folded);
        self.list_state.select(self.first_selectable());
    }

    fn toggle_fold(&mut self) {
        // Remember the prefix or path under the cursor so we can re-anchor
        // the selection after rebuilding rows. Otherwise the cursor would
        // jump to the first selectable line on every toggle.
        let anchor = self.list_state.selected().and_then(|i| self.rows.get(i)).map(|row| match row {
            Row::Secret { result, .. } => SelectionAnchor::Path(result.path.clone(), result.store.clone()),
            Row::FoldedGroup { prefix, .. } => SelectionAnchor::Prefix(prefix.clone()),
            Row::Store { name, .. } => SelectionAnchor::Store(name.clone()),
        });

        self.folded = !self.folded;
        self.rows = build_rows(&self.results, self.folded);
        self.list_state.select(self.reanchor(anchor));
    }

    /// Expand the view if currently folded and place the cursor on the first
    /// leaf belonging to `prefix`. No-op when already unfolded.
    fn unfold_to_prefix(&mut self, prefix: &str) {
        if !self.folded {
            return;
        }
        self.folded = false;
        self.rows = build_rows(&self.results, self.folded);
        let target = self.rows.iter().position(|row| match row {
            Row::Secret { result, shared_prefix, .. } => {
                shared_prefix.as_deref() == Some(prefix)
                    || prefix_of(&result.path) == prefix
            }
            _ => false,
        });
        self.list_state.select(target.or_else(|| self.first_selectable()));
    }

    fn selected_result(&self) -> Option<&SearchResult> {
        self.list_state
            .selected()
            .and_then(|i| self.rows.get(i))
            .and_then(|row| match row {
                Row::Secret { result, .. } => Some(result),
                Row::FoldedGroup { .. } | Row::Store { .. } => None,
            })
    }

    fn selected_folded_prefix(&self) -> Option<String> {
        self.list_state
            .selected()
            .and_then(|i| self.rows.get(i))
            .and_then(|row| match row {
                Row::FoldedGroup { prefix, .. } => Some(prefix.clone()),
                _ => None,
            })
    }

    fn is_selectable(&self, i: usize) -> bool {
        matches!(
            self.rows.get(i),
            Some(Row::Secret { .. }) | Some(Row::FoldedGroup { .. })
        )
    }

    fn reanchor(&self, anchor: Option<SelectionAnchor>) -> Option<usize> {
        let anchor = anchor?;
        for (i, row) in self.rows.iter().enumerate() {
            let hit = match (&anchor, row) {
                (SelectionAnchor::Path(p, s), Row::Secret { result, .. }) => {
                    &result.path == p && &result.store == s
                }
                (SelectionAnchor::Path(p, _), Row::FoldedGroup { prefix, .. }) => {
                    prefix_of(p) == prefix.as_str()
                }
                (SelectionAnchor::Prefix(prefix), Row::FoldedGroup { prefix: p, .. }) => prefix == p,
                (SelectionAnchor::Prefix(prefix), Row::Secret { shared_prefix, result, .. }) => {
                    shared_prefix.as_deref() == Some(prefix.as_str())
                        || prefix_of(&result.path) == prefix
                }
                (SelectionAnchor::Store(s), Row::Store { name, .. }) => s == name,
                _ => false,
            };
            if hit && self.is_selectable(i) {
                return Some(i);
            }
        }
        self.first_selectable()
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
        let area = standard_canvas(frame.area());
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header (brand + view name + health)
                Constraint::Length(1), // -- spacer --
                Constraint::Length(3), // search-input
                Constraint::Min(1),    // results
                Constraint::Length(0), // -- spacer --
                Constraint::Length(1), // footer
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_input(frame, chunks[2]);
        self.draw_results(frame, chunks[3]);
        self.draw_footer(frame, chunks[5]);

        // Render the picker / palette overlays last so they sit on top of
        // the rest of the chrome.
        if let Some(picker) = self.picker.as_mut() {
            picker.draw(frame);
        }
        if let Some(palette) = self.palette.as_mut() {
            palette.draw(frame);
        }
    }

    /// Translate a [`Command`] picked from the palette into a
    /// [`SearchAction`] the router already knows how to handle. For
    /// commands that need the next view to open via state mutation
    /// (currently just SwitchStore), we install the picker here and
    /// return [`SearchAction::None`] so the next frame draws the picker.
    fn dispatch_command(&mut self, cmd: Command) -> SearchAction {
        // Wired commands first; any variant with a CLI-only path falls
        // through to the hint toast at the bottom.
        match cmd {
            Command::NewSecret => return SearchAction::NewSecret,
            Command::Sync => return self.run_sync(),
            Command::Rekey => return self.run_rekey(),
            Command::Join => return self.run_join(),
            Command::AddRemote => return SearchAction::AddRemote,
            Command::SwitchStore => {
                self.picker = Some(StorePicker::new(
                    &self.ctx.stores_dir(),
                    self.ctx.store.clone(),
                ));
                return SearchAction::None;
            }
            Command::ToggleStoreColumn => {
                self.show_store_column = !self.show_store_column;
                return SearchAction::None;
            }
            Command::Envs => return SearchAction::OpenEnvs,
            Command::Help => return SearchAction::ShowHelp,
            Command::Quit => return SearchAction::Quit,
            _ => {}
        }

        match cmd.cli_hint() {
            Some(hint) => SearchAction::CommandHint(format!("run from CLI: {hint}")),
            None => SearchAction::None,
        }
    }

    fn run_sync(&mut self) -> SearchAction {
        use crate::cli::rekey;

        if let Err(e) = crate::git::pull(&self.ctx.store) {
            return SearchAction::CommandFailed(format!("sync pull failed: {e}"));
        }
        match rekey::rekey_store(&self.ctx, None) {
            Ok(n) => {
                self.refresh_results();
                self.store_health = check_store_health(&self.ctx);
                SearchAction::Synced(format!("pulled, {n} secret(s) rekeyed"))
            }
            Err(e) => SearchAction::CommandFailed(format!("sync rekey failed: {e}")),
        }
    }

    fn run_rekey(&self) -> SearchAction {
        use crate::cli::rekey;
        match rekey::rekey_store(&self.ctx, None) {
            Ok(n) => {
                let recipients = crate::crypto::age::collect_recipients(
                    &self.ctx.store,
                    self.ctx.recipients_path.as_deref(),
                )
                .map(|r| r.len())
                .unwrap_or(0);
                SearchAction::Rekeyed(format!(
                    "{n} secret(s) rekeyed for {recipients} recipient(s)"
                ))
            }
            Err(e) => SearchAction::CommandFailed(format!("rekey failed: {e}")),
        }
    }

    fn run_join(&mut self) -> SearchAction {
        use crate::cli::join::{self, JoinArgs};

        if join::is_self_recipient(&self.ctx) {
            return SearchAction::Joined("already a recipient".into());
        }

        match join::run(
            JoinArgs {
                name: None,
                no_push: false,
            },
            &self.ctx,
        ) {
            Ok(()) => {
                self.ctx.commit_and_push("himitsu: join");
                self.store_health = check_store_health(&self.ctx);
                SearchAction::Joined("joined as recipient".into())
            }
            Err(e) => SearchAction::CommandFailed(format!("join failed: {e}")),
        }
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let (health_label, health_color) = match &self.store_health {
            StoreHealth::Synced => ("synced".to_string(), theme::success()),
            StoreHealth::Behind(n) => (format!("{n} behind remote"), theme::warning()),
            StoreHealth::Dirty => ("uncommitted changes".to_string(), theme::danger()),
            StoreHealth::BehindAndDirty(n) => (format!("{n} behind + dirty"), theme::danger()),
            StoreHealth::NotGit => ("not a git repo".to_string(), theme::warning()),
            StoreHealth::NoRemote => (
                "no remote — run: himitsu remote add".to_string(),
                theme::warning(),
            ),
            StoreHealth::NotPushed => (
                "not pushed — run: himitsu git push -u origin main".to_string(),
                theme::warning(),
            ),
            StoreHealth::NotRecipient => (
                "not a recipient — run: himitsu join".to_string(),
                theme::warning(),
            ),
            StoreHealth::Unknown => ("unknown".to_string(), theme::muted()),
        };

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(20),
                Constraint::Length((health_label.len() as u16).saturating_add(4)),
            ])
            .split(area);

        // Left: brand chip + active view name. The chip carries the project's
        // namesake kanji (秘 = "secret", first half of 秘密 / himitsu).
        let mut left_spans = theme::brand_chip("秘 himitsu");
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(
            "search",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(Line::from(left_spans)), cols[0]);

        // Right: store health indicator, right-aligned within the second
        // column. The healthy steady-state (`Synced`) renders as a quiet
        // colored dot + label on the default background — we don't want a
        // bright green pill screaming at the user when nothing is wrong.
        // Every other state still uses the colored pill so problems remain
        // visually loud.
        let right_spans = if matches!(self.store_health, StoreHealth::Synced) {
            vec![
                Span::styled(icons::health(), Style::default().fg(health_color)),
                Span::raw(" "),
                Span::styled(health_label, Style::default().fg(health_color)),
            ]
        } else {
            theme::pill_with(
                format!("{} {health_label}", icons::health()),
                health_color,
                theme::on_accent(),
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right),
            cols[1],
        );
    }

    fn draw_input(&self, frame: &mut Frame<'_>, area: Rect) {
        let count = self.results.len();
        let count_label = format!(" {count} result{} ", if count == 1 { "" } else { "s" });
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme::border()))
            .title(" query ")
            .title_style(Style::default().fg(theme::border_label()))
            .title_top(
                Line::from(Span::styled(
                    count_label,
                    Style::default().fg(theme::muted()),
                ))
                .right_aligned(),
            );
        let text = Line::from(vec![
            Span::raw(&self.query),
            Span::styled("█", Style::default().fg(theme::accent())),
        ]);
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_results(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::new().fg(theme::border()))
            .title(" results ")
            .title_style(Style::default().fg(theme::border_label()));
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
                Style::default().fg(theme::muted()),
            )));
            frame.render_widget(p, inner);
            return;
        }

        // When multi-store grouping is active the store name is already in
        // the header row and we drop the redundant per-row store column.
        let has_store_headers = self.rows.iter().any(|r| matches!(r, Row::Store { .. }));
        let show_store = self.show_store_column && !has_store_headers;

        let now = Utc::now();

        // Pre-compute the rendered cells for each secret row so column
        // widths account for the rendered text, not raw data.
        struct SecretCells {
            indent: usize,
            parent_dim: String, // path prefix up to (but not including) the basename
            basename: String,
            updated: String,
            desc: String,
            envs: String,
            tags: Option<Vec<String>>,
        }
        let row_data: Vec<Option<SecretCells>> = self
            .rows
            .iter()
            .map(|row| match row {
                Row::Secret { result, indent, .. } => {
                    let (parent, name) = split_path_basename(&result.path);
                    let ts = result
                        .updated_at
                        .as_deref()
                        .or(result.created_at.as_deref());
                    let updated = ts
                        .and_then(parse_ts)
                        .map(|t| humanize_age_compact(now, t))
                        .unwrap_or_else(|| "—".to_string());
                    let desc = result.description.clone().unwrap_or_default();
                    let envs = self
                        .env_index
                        .get(&result.path)
                        .map(|labels| labels.join(", "))
                        .unwrap_or_default();
                    Some(SecretCells {
                        indent: *indent,
                        parent_dim: parent.to_string(),
                        basename: name.to_string(),
                        updated,
                        desc,
                        envs,
                        tags: result.tags.clone(),
                    })
                }
                _ => None,
            })
            .collect();

        // Column widths are computed against the widest secret row and the
        // header label itself.
        let path_w = row_data
            .iter()
            .filter_map(|d| {
                d.as_ref()
                    .map(|c| c.indent * 2 + c.parent_dim.len() + c.basename.len())
            })
            .max()
            .unwrap_or(0)
            .max("PATH".len());
        let updated_w = row_data
            .iter()
            .filter_map(|d| d.as_ref().map(|c| c.updated.len()))
            .max()
            .unwrap_or(0)
            .max("UPDATED".len());
        let envs_w = row_data
            .iter()
            .filter_map(|d| d.as_ref().map(|c| c.envs.len()))
            .max()
            .unwrap_or(0)
            .max("ENVS".len());
        let desc_w = row_data
            .iter()
            .filter_map(|d| d.as_ref().map(|c| desc_cell_width(&c.desc, c.tags.as_deref())))
            .max()
            .unwrap_or(0)
            .max("DESCRIPTION".len());
        let store_w = if show_store {
            self.rows
                .iter()
                .filter_map(|row| match row {
                    Row::Secret { result, .. } => Some(result.store.len()),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
                .max("STORE".len())
        } else {
            0
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        let header_style = Style::default()
            .fg(theme::muted())
            .add_modifier(Modifier::BOLD);
        let mut header_spans = vec![
            Span::styled(
                format!("{:<path_w$}  ", "PATH", path_w = path_w),
                header_style,
            ),
            Span::styled(
                format!("{:<updated_w$}  ", "UPDATED", updated_w = updated_w),
                header_style,
            ),
            Span::styled(
                format!("{:<envs_w$}  ", "ENVS", envs_w = envs_w),
                header_style,
            ),
            Span::styled(
                format!("{:<desc_w$}  ", "DESCRIPTION", desc_w = desc_w),
                header_style,
            ),
        ];
        if show_store {
            header_spans.push(Span::styled(
                format!("{:<store_w$}", "STORE", store_w = store_w),
                header_style,
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(header_spans)), chunks[0]);

        let items: Vec<ListItem> = self
            .rows
            .iter()
            .zip(row_data.iter())
            .map(|(row, data)| match row {
                Row::Store { name, count } => {
                    let line = Line::from(vec![
                        Span::styled(
                            format!("■ {name}"),
                            Style::default()
                                .fg(theme::accent())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(format!("({count})"), Style::default().fg(theme::muted())),
                    ]);
                    ListItem::new(line)
                }
                Row::FoldedGroup { prefix, count, indent } => {
                    let pad_indent = "  ".repeat(*indent);
                    let line = Line::from(vec![
                        Span::raw(pad_indent),
                        Span::styled(
                            format!("▸ {prefix}/"),
                            Style::default()
                                .fg(theme::accent())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(format!("({count})"), Style::default().fg(theme::muted())),
                    ]);
                    ListItem::new(line)
                }
                Row::Secret { result, shared_prefix, .. } => {
                    let cells = data.as_ref().unwrap();
                    // Compose the path cell. The parent prefix is split into
                    // a "shared" segment (top-level path slice when this leaf
                    // belongs to a multi-leaf group) painted in a subtle
                    // accent — replacing the old folder header — and the
                    // remaining parent path which stays dimmed.
                    let indent = "  ".repeat(cells.indent);
                    let (shared_seg, rest_seg) =
                        split_shared_prefix(&cells.parent_dim, shared_prefix.as_deref());
                    let consumed = indent.len() + cells.parent_dim.len() + cells.basename.len();
                    let pad = path_w.saturating_sub(consumed);
                    let chips = format_tag_chips(cells.tags.as_deref());
                    let chips_w = tag_chips_width(cells.tags.as_deref());
                    let consumed_desc = desc_cell_width(&cells.desc, cells.tags.as_deref());
                    let desc_pad = desc_w.saturating_sub(consumed_desc);
                    let needs_sep = !cells.desc.is_empty() && chips_w > 0;
                    let mut spans = vec![
                        Span::raw(indent),
                        Span::styled(
                            shared_seg.to_string(),
                            Style::default()
                                .fg(theme::accent())
                                .add_modifier(Modifier::DIM),
                        ),
                        Span::styled(
                            rest_seg.to_string(),
                            Style::default().fg(theme::path_dim()),
                        ),
                        Span::raw(cells.basename.clone()),
                        Span::raw(format!("{:<pad$}  ", "", pad = pad)),
                        Span::styled(
                            format!("{:<updated_w$}  ", cells.updated, updated_w = updated_w),
                            Style::default().fg(theme::muted()),
                        ),
                        Span::styled(
                            format!("{:<envs_w$}  ", cells.envs, envs_w = envs_w),
                            Style::default().fg(theme::accent()),
                        ),
                        Span::raw(cells.desc.clone()),
                    ];
                    if needs_sep {
                        spans.push(Span::raw(" "));
                    }
                    spans.extend(chips);
                    spans.push(Span::raw(format!("{:<pad$}  ", "", pad = desc_pad)));
                    if show_store {
                        spans.push(Span::styled(
                            format!("{:<store_w$}", result.store, store_w = store_w),
                            Style::default().fg(theme::accent()),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                }
            })
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme::accent())
                .fg(theme::on_accent())
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, chunks[1], &mut self.list_state);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        // The footer keeps only the most-used bindings. Anything else —
        // envs, help, switch-store, and any future commands — is reachable
        // through the command palette (Ctrl+P), so the row stays short and
        // doesn't need to grow as the catalog of commands does.
        let footer = Style::default().fg(theme::footer_text());
        render_distributed_footer(
            frame,
            area,
            vec![
                Line::from(vec![
                    Span::styled("↑/↓", Style::default().fg(theme::accent())),
                    Span::styled(" navigate", footer),
                ]),
                Line::from(vec![
                    Span::styled("enter", Style::default().fg(theme::accent())),
                    Span::styled(" open", footer),
                ]),
                Line::from(vec![
                    Span::styled("^n", Style::default().fg(theme::accent())),
                    Span::styled(" new", footer),
                ]),
                Line::from(vec![
                    Span::styled("^y", Style::default().fg(theme::accent())),
                    Span::styled(" copy", footer),
                ]),
                Line::from(vec![
                    Span::styled("^p", Style::default().fg(theme::accent())),
                    Span::styled(" commands", footer),
                ]),
                Line::from(vec![
                    Span::styled("esc", Style::default().fg(theme::accent())),
                    Span::styled(" quit", footer),
                ]),
            ],
        );
    }
}

/// Render a secret's tag list as a sequence of small `[tag]` spans suitable
/// for inlining into a table row. Returns an empty `Vec` when there are no
/// tags to show — either because the secret could not be decrypted (`None`)
/// or because it decrypted but carried no tags (`Some(&[])`); both cases
/// render the same way, mirroring how `description = None` is currently
/// handled (no chip, no `?` placeholder).
///
/// Each chip renders as `[<label>] ` with a trailing space so consecutive
/// chips space themselves naturally without the caller having to interleave
/// separators. The trailing space on the final chip is purely cosmetic and
/// is accounted for by [`tag_chips_width`] so column padding stays correct.
fn format_tag_chips(tags: Option<&[String]>) -> Vec<Span<'static>> {
    let Some(tags) = tags else {
        return Vec::new();
    };
    if tags.is_empty() {
        return Vec::new();
    }
    let style = Style::default().fg(theme::muted());
    tags.iter()
        .map(|tag| Span::styled(format!("[{tag}] "), style))
        .collect()
}

/// Cell-width of the rendering produced by [`format_tag_chips`], used to
/// account for chips when computing the description column width. Each chip
/// renders as `[<label>] ` (label length + 3 cells: brackets + trailing
/// space). Returns 0 when there are no chips.
fn tag_chips_width(tags: Option<&[String]>) -> usize {
    let Some(tags) = tags else {
        return 0;
    };
    tags.iter().map(|t| t.len() + 3).sum()
}

/// Combined width of a description cell: the description text, an optional
/// single-cell separator when both description and chips are present, and
/// the inline tag chips. Used for both the column-width calculation and
/// the per-row right-padding so they can't drift apart.
fn desc_cell_width(desc: &str, tags: Option<&[String]>) -> usize {
    let chips_w = tag_chips_width(tags);
    let sep = if !desc.is_empty() && chips_w > 0 { 1 } else { 0 };
    desc.len() + sep + chips_w
}

/// Split a slash-delimited secret path into `(parent_with_slash, basename)`.
/// `"foo/bar/baz"` becomes `("foo/bar/", "baz")`. A path without slashes
/// returns an empty parent. Kept here as a free function so the table
/// renderer can dim the parent prefix without re-walking the path.
fn split_path_basename(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(idx) => path.split_at(idx + 1),
        None => ("", path),
    }
}

/// Build a `secret_path → [env labels]` map from the project + global
/// configs. Glob entries (`prefix/*`) match every secret whose path starts
/// with `prefix/`; aliases and singles match their explicit path. Keeps
/// the result sorted within each entry so render order stays stable.
fn build_env_index() -> std::collections::HashMap<String, Vec<String>> {
    use crate::config::{self, EnvEntry};
    use std::collections::{BTreeSet, HashMap};

    let mut by_path: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut record = |path: String, label: &str| {
        by_path.entry(path).or_default().insert(label.to_string());
    };
    let mut walk = |envs: &std::collections::BTreeMap<String, Vec<EnvEntry>>| {
        for (label, entries) in envs {
            for entry in entries {
                match entry {
                    EnvEntry::Single(path) => record(path.clone(), label),
                    EnvEntry::Alias { path, .. } => record(path.clone(), label),
                    // Glob/Tag: skip — we don't have a way to expand against
                    // the result set here without more plumbing (Glob would
                    // need the full path list, Tag needs decryption). The
                    // label still shows up against any explicit Single/Alias
                    // references.
                    EnvEntry::Glob(_)
                    | EnvEntry::Tag(_)
                    | EnvEntry::AliasTag { .. } => {}
                }
            }
        }
    };

    if let Ok(global) = config::Config::load(&config::config_path()) {
        walk(&global.envs);
    }
    if let Some((project, _)) = config::load_project_config() {
        walk(&project.envs);
    }

    by_path
        .into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

/// Search view's keymap action priority. Quit comes first so a user who
/// rebinds an action to a printable character still has an escape hatch.
/// `CopyRefSelected` is listed before `CopySelected` because their
/// default bindings overlap on the bare `y` key: shift-insensitive
/// matching means a `y` binding would otherwise claim `Shift+Y` first.
const SEARCH_ACTION_PRIORITY: &[KeyAction] = &[
    KeyAction::Quit,
    KeyAction::CommandPalette,
    KeyAction::NewSecret,
    KeyAction::Envs,
    KeyAction::SwitchStore,
    KeyAction::CopyRefSelected,
    KeyAction::CopySelected,
];

fn match_keymap_action(keymap: &KeyMap, key: &crossterm::event::KeyEvent) -> Option<KeyAction> {
    keymap.action_for_key_in(key, SEARCH_ACTION_PRIORITY)
}

/// Build the `himitsu read <ref>` command for a given (store, path) pair.
///
/// `active_label` is the canonical label of the currently-active store,
/// produced by [`crate::cli::search::store_label`] — same function that
/// populates `SearchResult.store`. Comparing labels directly means
/// `same_store` is decided in one place and stays consistent whether the
/// active store is set by slug or by absolute path.
fn format_read_command(row_store: &str, secret_path: &str, active_label: &str) -> String {
    if row_store == active_label {
        format!("himitsu read {secret_path}")
    } else {
        format!("himitsu -r {row_store} read {secret_path}")
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

/// Check the git health of a store checkout (offline — no fetch).
///
/// Returns a [`StoreHealth`] summarising whether the checkout is behind its
/// remote tracking branch and/or has uncommitted local changes. Also checks
/// whether the user's own age key is in the store's recipient list —
/// [`StoreHealth::NotRecipient`] takes priority over git health because the
/// store is unusable without it.
fn check_store_health(ctx: &Context) -> StoreHealth {
    use crate::git;

    let store_path = &ctx.store;

    if let Some(override_health) = store_health_override() {
        return override_health;
    }

    if store_path.as_os_str().is_empty() {
        return StoreHealth::Unknown;
    }

    // Recipient membership check — takes priority because the store is
    // unusable (can't decrypt) if you're not a recipient.
    if !crate::cli::join::is_self_recipient(ctx) {
        return StoreHealth::NotRecipient;
    }

    if !store_path.join(".git").exists() {
        return StoreHealth::NotGit;
    }

    // Current branch name
    let branch = match git::run(&["rev-parse", "--abbrev-ref", "HEAD"], store_path) {
        Ok(b) => b.trim().to_string(),
        Err(_) => return StoreHealth::Unknown,
    };

    // Check if any remote is configured at all
    let has_remote = git::run(&["remote"], store_path)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !has_remote {
        return StoreHealth::NoRemote;
    }

    // Check remote tracking branch exists
    let remote_ref = format!("origin/{branch}");
    if git::run(&["rev-parse", "--verify", &remote_ref], store_path).is_err() {
        return StoreHealth::NotPushed;
    }

    // Behind count
    let behind: u32 = git::run(
        &["rev-list", "--count", &format!("HEAD..{remote_ref}")],
        store_path,
    )
    .ok()
    .and_then(|s| s.trim().parse().ok())
    .unwrap_or(0);

    // Dirty working tree
    let dirty = git::run(&["status", "--short"], store_path)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    match (behind > 0, dirty) {
        (true, true) => StoreHealth::BehindAndDirty(behind),
        (true, false) => StoreHealth::Behind(behind),
        (false, true) => StoreHealth::Dirty,
        (false, false) => StoreHealth::Synced,
    }
}

fn store_health_override() -> Option<StoreHealth> {
    let raw = std::env::var("HIMITSU_TUI_STORE_HEALTH").ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "synced" => Some(StoreHealth::Synced),
        "no-remote" | "no_remote" => Some(StoreHealth::NoRemote),
        "not-pushed" | "not_pushed" => Some(StoreHealth::NotPushed),
        "not-git" | "not_git" => Some(StoreHealth::NotGit),
        "not-recipient" | "not_recipient" => Some(StoreHealth::NotRecipient),
        "dirty" => Some(StoreHealth::Dirty),
        "unknown" => Some(StoreHealth::Unknown),
        _ => None,
    }
}

/// Tracks which row was selected before a fold/unfold toggle so the cursor
/// can re-anchor on the most semantically equivalent row in the rebuilt list.
#[derive(Debug, Clone)]
enum SelectionAnchor {
    /// A specific secret leaf, identified by `(path, store)`.
    Path(String, String),
    /// A folded group, identified by its top-level prefix.
    Prefix(String),
    /// A store header (kept selectable-adjacent when only stores remain).
    Store(String),
}

/// Top-level path segment of a secret's path, used for prefix grouping.
fn prefix_of(path: &str) -> &str {
    match path.split_once('/') {
        Some((head, _)) => head,
        None => path,
    }
}

/// Group a flat list of results into rows.
///
/// When results span **multiple stores**, rows are partitioned per-store with
/// a `Store` header row per bucket; within each bucket we apply path-prefix
/// grouping. When only one store is present we fall back to the single-store
/// layout (no store header).
///
/// A "group" is any top-level path segment that contains ≥ 2 leaves. In
/// folded mode each such group collapses to a single `FoldedGroup` row; in
/// unfolded mode the leaves render inline with their shared prefix tagged so
/// the renderer can paint it in a subtle accent. Singletons render the same
/// in both modes. Within each section entries sort alphabetically so layout
/// is stable regardless of input order.
fn build_rows(results: &[SearchResult], folded: bool) -> Vec<Row> {
    use std::collections::BTreeMap;

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
        append_prefix_grouped_rows(&mut rows, bucket, multi_store, folded);
    }
    rows
}

/// Append `bucket` rows to `rows` applying path-prefix grouping.
///
/// `under_store_header` adds one level of indent so each store's children
/// visually nest. `folded` collapses ≥ 2-leaf groups into `FoldedGroup` rows.
fn append_prefix_grouped_rows(
    rows: &mut Vec<Row>,
    bucket: Vec<SearchResult>,
    under_store_header: bool,
    folded: bool,
) {
    use std::collections::HashMap;

    let store_indent: usize = if under_store_header { 1 } else { 0 };

    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<SearchResult>> = HashMap::new();
    for r in bucket {
        let prefix = prefix_of(&r.path).to_string();
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

    for (prefix, mut items) in folders {
        if folded {
            rows.push(Row::FoldedGroup {
                prefix,
                count: items.len(),
                indent: store_indent,
            });
            continue;
        }
        items.sort_by(|a, b| a.path.cmp(&b.path));
        let shared = Some(prefix);
        for result in items {
            rows.push(Row::Secret {
                result,
                indent: store_indent,
                shared_prefix: shared.clone(),
            });
        }
    }
    for (_, items) in singles {
        for result in items {
            rows.push(Row::Secret {
                result,
                indent: store_indent,
                shared_prefix: None,
            });
        }
    }
}

/// Split `parent` (the slash-terminated path prefix in front of a secret's
/// basename) into a leading "shared" segment and the remainder. The shared
/// segment is `"<prefix>/"` when the leaf is part of a multi-leaf group;
/// otherwise the entire parent stays in the second slot for the dimmed
/// renderer to draw as before.
fn split_shared_prefix<'a>(parent: &'a str, shared: Option<&str>) -> (&'a str, &'a str) {
    let Some(prefix) = shared else {
        return ("", parent);
    };
    let head = format!("{prefix}/");
    if parent.starts_with(&head) {
        parent.split_at(head.len())
    } else {
        ("", parent)
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
            ("tab", "fold / unfold groups"),
            ("backspace", "delete char"),
            ("ctrl-p", "open command palette"),
            ("ctrl-n", "new secret"),
            ("ctrl-s", "switch store"),
            ("ctrl-y", "copy selection to clipboard"),
            ("shift-e", "browse env presets"),
            ("?", "toggle this help"),
            ("esc / ctrl-c", "quit"),
        ]
    }

    pub fn help_title() -> &'static str {
        "search · keys"
    }
}

#[cfg(test)]
mod read_command_tests {
    use super::*;

    #[test]
    fn same_store_emits_bare_path() {
        let cmd = format_read_command("acme/secrets", "prod/API_KEY", "acme/secrets");
        assert_eq!(cmd, "himitsu read prod/API_KEY");
    }

    #[test]
    fn cross_store_qualifies_with_remote_flag() {
        let cmd = format_read_command("acme/infra", "prod/SHARED_KEY", "acme/secrets");
        assert_eq!(cmd, "himitsu -r acme/infra read prod/SHARED_KEY");
    }

    #[test]
    fn full_path_label_round_trips_too() {
        // When the active store is set by absolute path, store_label
        // returns the same path string — equality still says "same store".
        let cmd = format_read_command("/tmp/x/.himitsu", "p", "/tmp/x/.himitsu");
        assert_eq!(cmd, "himitsu read p");
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
        // Default unfolded: 3 leaves, no header row.
        assert_eq!(view.rows.len(), 3);
        match &view.rows[0] {
            Row::Secret {
                result,
                shared_prefix,
                ..
            } => {
                assert_eq!(result.path, "prod/API_KEY");
                assert_eq!(shared_prefix.as_deref(), Some("prod"));
            }
            other => panic!("expected Secret at row 0, got {other:?}"),
        }
        assert_eq!(view.list_state.selected(), Some(0));
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
                Row::FoldedGroup { .. } => "folded",
                Row::Secret { shared_prefix, .. } if shared_prefix.is_some() => "grouped",
                Row::Secret { .. } => "leaf",
            })
            .collect();
        assert_eq!(kinds, vec!["grouped", "grouped", "leaf"]);
        match &view.rows[2] {
            Row::Secret { result, .. } => assert_eq!(result.path, "staging/API_KEY"),
            _ => panic!("expected secret leaf at row 2"),
        }
    }

    #[test]
    fn tab_folds_groups_into_single_row() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        // Unfolded baseline: 3 leaves (2 grouped + 1 single).
        assert_eq!(view.rows.len(), 3);

        view.on_key(key(KeyCode::Tab), &km);
        // Folded: prod group collapses to a single FoldedGroup row + the
        // staging singleton stays as a Secret. 2 rows total.
        assert_eq!(view.rows.len(), 2);
        match &view.rows[0] {
            Row::FoldedGroup { prefix, count, .. } => {
                assert_eq!(prefix, "prod");
                assert_eq!(*count, 2);
            }
            other => panic!("expected FoldedGroup at row 0, got {other:?}"),
        }
        match &view.rows[1] {
            Row::Secret { result, .. } => assert_eq!(result.path, "staging/API_KEY"),
            other => panic!("expected Secret at row 1, got {other:?}"),
        }

        view.on_key(key(KeyCode::Tab), &km);
        assert_eq!(view.rows.len(), 3);
    }

    #[test]
    fn folded_group_is_selectable_and_enter_unfolds() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);

        view.on_key(key(KeyCode::Tab), &km);
        assert_eq!(view.list_state.selected(), Some(0));
        assert!(view.selected_folded_prefix().is_some());

        let action = view.on_key(key(KeyCode::Enter), &km);
        assert!(matches!(action, SearchAction::None));
        assert!(!view.folded);
        match view.rows.get(view.list_state.selected().unwrap()) {
            Some(Row::Secret { result, .. }) => assert_eq!(result.path, "prod/API_KEY"),
            other => panic!("expected first prod leaf selected, got {other:?}"),
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
        assert!(matches!(
            view.on_key(key(KeyCode::Esc), &km),
            SearchAction::Quit
        ));
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
        // Unfolded layout: 3 secret rows, no header row. First selectable
        // is row 0; Up wraps to the last row.
        assert_eq!(view.list_state.selected(), Some(0));
        view.on_key(key(KeyCode::Up), &km);
        assert_eq!(view.list_state.selected(), Some(2));
        view.on_key(key(KeyCode::Down), &km);
        assert_eq!(view.list_state.selected(), Some(0));
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

        let render = |view: &mut SearchView| -> String {
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
            rendered
        };

        // Default render: PATH / UPDATED / ENVS / DESCRIPTION are always
        // shown; STORE is hidden until the user toggles it on via the
        // command palette.
        let rendered = render(&mut view);
        assert!(rendered.contains("PATH"), "missing PATH header: {rendered}");
        assert!(
            rendered.contains("UPDATED"),
            "missing UPDATED header: {rendered}"
        );
        assert!(rendered.contains("ENVS"), "missing ENVS header: {rendered}");
        assert!(
            rendered.contains("DESCRIPTION"),
            "missing DESCRIPTION header: {rendered}"
        );
        assert!(
            !rendered.contains("STORE"),
            "STORE header should be hidden by default: {rendered}"
        );

        // Toggling the column on through the dispatch path (same code the
        // command palette runs) makes STORE appear.
        view.dispatch_command(Command::ToggleStoreColumn);
        let rendered = render(&mut view);
        assert!(
            rendered.contains("STORE"),
            "STORE header should appear after toggle: {rendered}"
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

        // Next: alpha's two children (no folder header row anymore). Both
        // tagged with the shared "prod" prefix and indented under the store.
        match &view.rows[1] {
            Row::Secret {
                result,
                indent,
                shared_prefix,
            } => {
                assert_eq!(result.store, "acme/alpha");
                assert_eq!(*indent, 1, "alpha child indent under store header");
                assert_eq!(shared_prefix.as_deref(), Some("prod"));
            }
            other => panic!("row 1 expected Secret, got {other:?}"),
        }

        // Beta store header appears later; its secret is indented under it.
        let beta_idx = view
            .rows
            .iter()
            .position(|r| matches!(r, Row::Store { name, .. } if name == "acme/beta"))
            .expect("acme/beta store header missing");
        match &view.rows[beta_idx + 1] {
            Row::Secret { result, indent, .. } => {
                assert_eq!(result.store, "acme/beta");
                assert_eq!(*indent, 1);
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
        use secrecy::ExposeSecret;
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data");
        let state_dir = dir.path().join("state");
        let store = state_dir.join("stores/acme/prod");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/secrets")).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/recipients")).unwrap();

        let identity = ::age::x25519::Identity::generate();
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
            matches!(
                action,
                SearchAction::Copied(_) | SearchAction::CopyFailed(_)
            ),
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

    /// Concatenate the rendered text of a span list. Used by the chip tests
    /// so they can assert on the visible output without caring about styling.
    fn spans_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn format_tag_chips_renders_each_label() {
        let tags = vec!["pci".to_string(), "stripe".to_string()];
        let spans = format_tag_chips(Some(&tags));
        let rendered = spans_text(&spans);
        assert!(
            rendered.contains("[pci]"),
            "first chip missing: {rendered:?}"
        );
        assert!(
            rendered.contains("[stripe]"),
            "second chip missing: {rendered:?}"
        );
    }

    #[test]
    fn format_tag_chips_returns_empty_when_tags_none() {
        let spans = format_tag_chips(None);
        assert!(
            spans.is_empty(),
            "expected no chips for None, got {spans:?}"
        );
    }

    #[test]
    fn format_tag_chips_returns_empty_when_tag_list_empty() {
        let tags: Vec<String> = Vec::new();
        let spans = format_tag_chips(Some(&tags));
        assert!(
            spans.is_empty(),
            "expected no chips for empty list, got {spans:?}"
        );
    }
}
