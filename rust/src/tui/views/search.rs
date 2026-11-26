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

use crate::tui::layout::{
    FOOTER_HEIGHT, HEADER_HEIGHT, HEADER_LEFT_MIN_WIDTH, SEARCH_INPUT_HEIGHT, SPACER_HEIGHT,
};
use crate::tui::theme;
use ratatui::Frame;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use chrono::Utc;

use crate::cli::Context;
use crate::cli::search::{
    SearchResult, collect_candidates_in, collect_stores, filter_candidates, humanize_age_compact,
    parse_ts,
};
use crate::crypto::{age, secret_value};
use crate::remote::store;
use crate::tui::keymap::{KeyAction, KeyMap};
use crate::tui::model::path_folding::{Row, build_rows, prefix_of, split_shared_prefix};
use crate::tui::model::result_sort::{SearchColumn, SortDirection, SortState};
use crate::tui::views::command_palette::{Command, CommandPalette, CommandPaletteOutcome};
use crate::tui::views::store_picker::{StorePicker, StorePickerOutcome};
use crate::tui::widgets::store_health::{StoreHealth, check_store_health, render_health_pill};

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
    OpenOutputs,
    /// User picked "list recipients" from the command palette — open the
    /// recipient list view.
    OpenRecipientList,
    /// User picked "add recipient" from the command palette — open the
    /// add-recipient form.
    OpenRecipientAdd,
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

const SEARCH_COLUMN_MAX_WIDTH: usize = 32;
const TRUNCATION_MARKER: &str = "..";

pub struct SearchView {
    query: String,
    results: Vec<SearchResult>,
    rows: Vec<Row>,
    list_state: ListState,
    /// Operations use the selected checkout; All restores the invocation target.
    ctx: Context,
    invocation_store: PathBuf,
    invocation_recipients: Option<String>,
    /// Embedded store-picker overlay. When `Some`, it intercepts every key.
    picker: Option<StorePicker>,
    /// Embedded command-palette overlay. When `Some`, it intercepts every
    /// key just like the store picker. Mutually exclusive with `picker`
    /// because both are modal popups.
    palette: Option<CommandPalette>,
    health: StoreHealth,
    /// Whether to render the STORE column in the results table. Off by
    /// default — most users work in a single store at a time, so the
    /// column is dead weight. Toggled via the command palette
    /// ("toggle store column"). When the table groups results by store
    /// (multi-store searches) the column is hidden regardless because the
    /// store name is already in a group header row.
    show_store_column: bool,
    /// Active tag filters selected from result-row tag chips. AND semantics,
    /// matching CLI `search --tag`.
    tag_filters: Vec<String>,
    /// Column currently selected by the table-header cursor. Tab and
    /// Shift+Tab move this focus without touching the row selection.
    selected_column: SearchColumn,
    /// Current result ordering. Ctrl+O sorts by the selected column and
    /// repeats on the same column toggle ascending/descending direction.
    sort_state: SortState,
    /// When true, multi-leaf top-level prefix groups collapse to a single
    /// `FoldedGroup` row. Ctrl+- collapses and Ctrl++ expands. Singleton
    /// paths render the same in both states. Default: unfolded.
    folded: bool,
    /// Cached decrypted candidates. Refreshed only by [`recollect`] (store
    /// change, join, explicit refresh) — NOT on every query keystroke. The
    /// query fuzzy-filter runs against this cache via [`filter_candidates`].
    candidates: Vec<SearchResult>,
    /// Whether the query has changed since the last [`refresh_results`] and
    /// the app router should re-filter before the next draw. Set by
    /// [`mark_search_dirty`]; consumed via [`take_search_dirty`].
    search_dirty: bool,
    /// The same checkout inventory drives search and scope selection, even
    /// when a checkout has no secrets. None selects All.
    stores: Vec<(String, PathBuf)>,
    selected: Option<usize>,
    /// Project store slug and its optional recipients directory override.
    project: Option<(String, Option<String>)>,
    /// Resolved concrete theme names per mode (`[user, project, all]`).
    /// Pre-resolved once at configuration time so cycling modes does not
    /// re-roll a new random pick each press.
    theme_names: [String; 3],
}

impl SearchView {
    pub fn new(ctx: &Context) -> Self {
        let mut stores = collect_stores(ctx).unwrap_or_default();
        stores.sort_unstable_by(|a, b| (a.1 != ctx.store, &a.0).cmp(&(b.1 != ctx.store, &b.0)));
        let project = ctx
            .project_config()
            .ok()
            .flatten()
            .and_then(|(cfg, _)| cfg.default_store.map(|slug| (slug, cfg.recipients_path)));
        let mut active = ctx.clone();
        if let Some((slug, dir)) = &project
            && active.recipients_path.is_none()
            && stores
                .iter()
                .any(|(name, path)| name == slug && path == &ctx.store)
        {
            active.recipients_path.clone_from(dir);
        }
        let health = check_store_health(&active);
        let mut view = Self {
            query: String::new(),
            results: Vec::new(),
            rows: Vec::new(),
            list_state: ListState::default(),
            invocation_store: active.store.clone(),
            invocation_recipients: active.recipients_path.clone(),
            ctx: active,
            picker: None,
            palette: None,
            health,
            show_store_column: false,
            tag_filters: Vec::new(),
            selected_column: SearchColumn::Path,
            sort_state: SortState {
                column: SearchColumn::Path,
                direction: SortDirection::Asc,
            },
            folded: false,
            search_dirty: false,
            project,
            candidates: Vec::new(),
            stores,
            selected: None,
            theme_names: [
                String::from("himitsu"),
                String::from("himitsu"),
                String::from("himitsu"),
            ],
        };
        view.recollect();
        view
    }

    /// Store the session-resolved per-mode theme names and apply the
    /// current mode's theme. Called by the app router once after
    /// construction; the names are pre-resolved so cycling modes swaps
    /// palettes without re-rolling `random`.
    pub fn set_theme_names(&mut self, names: [String; 3]) {
        self.theme_names = names;
        self.apply_theme();
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

        self.on_results_key(key, keymap, crate::tui::terminal::tab_is_ambiguous())
    }

    fn on_results_key(
        &mut self,
        key: KeyEvent,
        keymap: &KeyMap,
        ambiguous_tab: bool,
    ) -> SearchAction {
        // Configurable action bindings take precedence over the fall-through
        // text-editing keys below. Quit is checked first so a user who rebinds
        // new_secret to a printable character still has an escape hatch.
        // All matches route through `dispatch_action` so leader-key chord
        // completions (resolved at the App layer) take the same code path.
        let action = match_keymap_action(keymap, &key).or_else(|| {
            // Legacy 0x09 is both Tab and Ctrl+I. Try the user's Ctrl+I
            // binding only when no explicit Tab binding claimed the event.
            if ambiguous_tab && key.code == KeyCode::Tab && key.modifiers.is_empty() {
                let ctrl_i = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL);
                match_keymap_action(keymap, &ctrl_i)
            } else {
                None
            }
        });
        if let Some(action) = action
            && let Some(outcome) = self.dispatch_action(action, keymap)
        {
            return outcome;
        }

        // Tag refine and column sort are keymap-driven actions (RefineTag
        // / SortColumn) routed through `dispatch_action` above —
        // rebindable like everything else, no hardcoded chords here.

        match (key.code, key.modifiers) {
            (KeyCode::BackTab, _) | (KeyCode::Left, KeyModifiers::NONE) => {
                self.select_prev_column();
                SearchAction::None
            }
            (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => {
                self.select_prev_column();
                SearchAction::None
            }
            (KeyCode::Tab, _) | (KeyCode::Right, KeyModifiers::NONE) => {
                self.select_next_column();
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
                let changed = self.query.pop().is_some()
                    || (self.query.is_empty() && self.tag_filters.pop().is_some());
                if changed {
                    self.mark_search_dirty();
                }
                SearchAction::None
            }
            (KeyCode::Char(ch), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.query.push(ch);
                self.mark_search_dirty();
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
    pub fn dispatch_action(&mut self, action: KeyAction, keymap: &KeyMap) -> Option<SearchAction> {
        match action {
            KeyAction::Quit => Some(SearchAction::Quit),
            KeyAction::CommandPalette => {
                self.palette = Some(CommandPalette::new(keymap));
                Some(SearchAction::None)
            }
            KeyAction::NewSecret => Some(SearchAction::NewSecret),
            KeyAction::Outputs => Some(SearchAction::OpenOutputs),
            KeyAction::SwitchStore => {
                self.picker = Some(StorePicker::new(
                    &self.ctx.stores_dir(),
                    self.active_context().store.clone(),
                ));
                Some(SearchAction::None)
            }
            KeyAction::CycleStore => {
                self.cycle_store();
                Some(SearchAction::None)
            }
            KeyAction::CopySelected => Some(self.copy_selected_to_clipboard()),
            KeyAction::CopyRefSelected => Some(self.copy_selected_ref_to_clipboard()),
            KeyAction::CollapsePaths => {
                self.set_folded(true);
                Some(SearchAction::None)
            }
            KeyAction::ExpandPaths => {
                self.set_folded(false);
                Some(SearchAction::None)
            }
            KeyAction::RefineTag => Some(self.refine_to_selected_tag()),
            KeyAction::SortColumn => {
                self.sort_by_selected_column();
                Some(SearchAction::None)
            }
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
        let active_label = crate::cli::search::store_label(&self.invocation_store, &self.ctx);
        let cmd = format_read_command(&result.store, &result.path, &active_label);
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(cmd.clone())) {
            Ok(()) => SearchAction::Copied(format!("$ {cmd}")),
            Err(e) => SearchAction::CopyFailed(format!("clipboard unavailable: {e}")),
        }
    }

    /// Re-filter the cached candidates against the current query + tags.
    /// Fast (<1ms) — safe to call on every keystroke. The slow decryption
    /// happens in [`recollect`], called only on store changes / explicit
    /// refresh.
    pub(crate) fn refresh_results(&mut self) {
        let mut results = filter_candidates(&self.candidates, &self.query, &self.tag_filters);
        if let Some(store) = self.selected_store() {
            results.retain(|r| r.store == store);
        }
        self.results = results;
        self.rows = build_rows(&self.results, self.folded, self.sort_state);
        self.normalize_selected_column();
        self.list_state.select(self.first_selectable());
    }

    /// Re-collect + decrypt all secrets from every known store, then
    /// re-filter. Slow (~1s for 100+ secrets). Call only when the
    /// underlying secret set may have changed: store switch, join,
    /// new-secret save, explicit refresh.
    pub(crate) fn recollect(&mut self) {
        self.candidates = collect_candidates_in(&self.ctx, &self.stores);
        self.refresh_results();
    }

    pub(crate) fn selected_store(&self) -> Option<&str> {
        self.selected.map(|index| self.stores[index].0.as_str())
    }

    pub(crate) fn active_context(&self) -> &Context {
        &self.ctx
    }

    pub(crate) fn active_health(&self) -> &StoreHealth {
        &self.health
    }

    fn is_project_scope(&self) -> bool {
        self.project
            .as_ref()
            .is_some_and(|(slug, _)| Some(slug.as_str()) == self.selected_store())
    }

    fn apply_theme(&self) {
        let index = if self.selected.is_none() {
            2
        } else {
            usize::from(self.is_project_scope())
        };
        let _ = theme::set_theme(&self.theme_names[index]);
    }

    pub(crate) fn restore_store(&mut self, path: &std::path::Path) -> bool {
        let Some(index) = self.stores.iter().position(|(_, p)| p == path) else {
            return false;
        };
        self.select_store(Some(index));
        true
    }

    fn select_store(&mut self, index: Option<usize>) {
        self.selected = index;
        if let Some(i) = index {
            let (slug, path) = &self.stores[i];
            self.ctx.store.clone_from(path);
            let project = self.project.as_ref().filter(|(project, _)| project == slug);
            if path == &self.invocation_store {
                self.ctx
                    .recipients_path
                    .clone_from(&self.invocation_recipients);
            } else {
                self.ctx.recipients_path = store::load_store_config(path)
                    .ok()
                    .and_then(|cfg| cfg.recipients_path)
                    .or_else(|| project.and_then(|(_, dir)| dir.clone()));
            }
        } else {
            self.ctx.store.clone_from(&self.invocation_store);
            self.ctx
                .recipients_path
                .clone_from(&self.invocation_recipients);
        }
        self.health = check_store_health(self.active_context());
        self.apply_theme();
        self.refresh_results();
    }

    fn cycle_store(&mut self) {
        if !self.stores.is_empty() {
            let next = self.selected.map_or(0, |i| i + 1);
            self.select_store((next < self.stores.len()).then_some(next));
        }
    }

    /// The selected checkout, or the invocation's store in All.
    /// Used to decide whether a secret's store cell gets the accent color
    /// (default store) or a distinct `store_color` (non-default store).
    fn default_store_slug(&self) -> String {
        if let Some(ds) = self.selected_store() {
            return ds.to_owned();
        }
        crate::cli::search::store_label(&self.ctx.store, &self.ctx)
    }

    fn mark_search_dirty(&mut self) {
        self.search_dirty = true;
    }

    pub(crate) fn take_search_dirty(&mut self) -> bool {
        let search_dirty = self.search_dirty;
        self.search_dirty = false;
        search_dirty
    }

    fn refine_to_selected_tag(&mut self) -> SearchAction {
        let Some(tag) = self
            .selected_result()
            .and_then(|r| r.tags.as_ref())
            .and_then(|tags| tags.first())
            .cloned()
        else {
            return SearchAction::CommandHint("selected result has no tag chip".into());
        };
        if !self.tag_filters.iter().any(|existing| existing == &tag) {
            self.tag_filters.push(tag.clone());
            self.refresh_results();
        }
        SearchAction::CommandHint(format!("filtering by tag:{tag}"))
    }

    fn set_folded(&mut self, folded: bool) {
        if self.folded == folded {
            return;
        }

        // Remember the prefix or path under the cursor so we can re-anchor
        // the selection after rebuilding rows. Otherwise the cursor would
        // jump to the first selectable line on every toggle.
        let anchor = self.selected_anchor();

        self.folded = folded;
        self.rows = build_rows(&self.results, self.folded, self.sort_state);
        self.list_state.select(self.reanchor(anchor));
    }

    fn selected_anchor(&self) -> Option<SelectionAnchor> {
        self.list_state
            .selected()
            .and_then(|i| self.rows.get(i))
            .map(|row| match row {
                Row::Secret { result, .. } => {
                    SelectionAnchor::Path(result.path.clone(), result.store.clone())
                }
                Row::FoldedGroup { prefix, .. } => SelectionAnchor::Prefix(prefix.clone()),
                Row::Store { name, .. } => SelectionAnchor::Store(name.clone()),
            })
    }

    fn sort_by_selected_column(&mut self) {
        self.normalize_selected_column();
        self.sort_state = if self.sort_state.column == self.selected_column {
            SortState {
                column: self.selected_column,
                direction: self.sort_state.direction.toggled(),
            }
        } else {
            SortState {
                column: self.selected_column,
                direction: SortDirection::Asc,
            }
        };
        let anchor = self.selected_anchor();
        self.rows = build_rows(&self.results, self.folded, self.sort_state);
        self.list_state.select(self.reanchor(anchor));
    }

    fn select_next_column(&mut self) {
        self.move_selected_column(1);
    }

    fn select_prev_column(&mut self) {
        self.move_selected_column(-1);
    }

    fn move_selected_column(&mut self, delta: isize) {
        let columns = self.visible_columns();
        let Some(current) = columns.iter().position(|c| *c == self.selected_column) else {
            self.selected_column = SearchColumn::Path;
            return;
        };
        let len = columns.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        self.selected_column = columns[next];
    }

    fn visible_columns(&self) -> Vec<SearchColumn> {
        let mut columns = SearchColumn::base_columns().to_vec();
        if self.store_column_visible() {
            columns.push(SearchColumn::Store);
        }
        columns
    }

    fn store_column_visible(&self) -> bool {
        self.show_store_column && !self.rows.iter().any(|r| matches!(r, Row::Store { .. }))
    }

    fn normalize_selected_column(&mut self) {
        if !self.visible_columns().contains(&self.selected_column) {
            self.selected_column = SearchColumn::Path;
        }
    }

    /// Expand the view if currently folded and place the cursor on the first
    /// leaf belonging to `prefix`. No-op when already unfolded.
    fn unfold_to_prefix(&mut self, prefix: &str) {
        if !self.folded {
            return;
        }
        self.folded = false;
        self.rows = build_rows(&self.results, self.folded, self.sort_state);
        let target = self.rows.iter().position(|row| match row {
            Row::Secret {
                result,
                shared_prefix,
                ..
            } => shared_prefix.as_deref() == Some(prefix) || prefix_of(&result.path) == prefix,
            _ => false,
        });
        self.list_state
            .select(target.or_else(|| self.first_selectable()));
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
                (SelectionAnchor::Prefix(prefix), Row::FoldedGroup { prefix: p, .. }) => {
                    prefix == p
                }
                (
                    SelectionAnchor::Prefix(prefix),
                    Row::Secret {
                        shared_prefix,
                        result,
                        ..
                    },
                ) => {
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
                Constraint::Length(HEADER_HEIGHT), // header (brand + view name + health)
                Constraint::Length(1),             // -- spacer --
                Constraint::Length(SEARCH_INPUT_HEIGHT), // search-input
                Constraint::Min(1),                // results
                Constraint::Length(SPACER_HEIGHT), // -- spacer --
                Constraint::Length(FOOTER_HEIGHT), // footer
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_input(frame, chunks[2]);
        self.draw_results(frame, chunks[3]);
        self.draw_footer(frame, chunks[5]);
        if self.picker.is_none() && self.palette.is_none() {
            self.draw_selected_description(frame);
        }
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
                    self.active_context().store.clone(),
                ));
                return SearchAction::None;
            }
            Command::ToggleStoreColumn => {
                self.show_store_column = !self.show_store_column;
                self.normalize_selected_column();
                return SearchAction::None;
            }
            Command::Outputs => return SearchAction::OpenOutputs,
            Command::RecipientLs => return SearchAction::OpenRecipientList,
            Command::RecipientAdd => return SearchAction::OpenRecipientAdd,
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
        use crate::cli::store_ops;

        if let Err(e) = crate::git::pull(&self.active_context().store) {
            return SearchAction::CommandFailed(format!("sync pull failed: {e}"));
        }
        match store_ops::rekey(self.active_context(), None) {
            Ok(n) => {
                self.recollect();
                self.health = check_store_health(self.active_context());
                SearchAction::Synced(format!("pulled, {n} secret(s) rekeyed"))
            }
            Err(e) => SearchAction::CommandFailed(format!("sync rekey failed: {e}")),
        }
    }

    fn run_rekey(&self) -> SearchAction {
        use crate::cli::store_ops;
        let ctx = self.active_context();
        match store_ops::rekey(ctx, None) {
            Ok(n) => {
                let recipients = crate::crypto::age::collect_recipients(
                    &ctx.store,
                    ctx.recipients_path.as_deref(),
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

    pub(crate) fn run_join(&mut self) -> SearchAction {
        use crate::cli::join::JoinOutcome;
        use crate::cli::store_ops;

        // The silent core is idempotent and never prints (printing would
        // corrupt ratatui); the chain commits and pushes on success.
        match store_ops::join(self.active_context()) {
            Ok(outcome) => {
                self.health = check_store_health(self.active_context());
                let message = match outcome {
                    JoinOutcome::AlreadyRecipient => "already a recipient",
                    JoinOutcome::Joined(_) => "joined as recipient",
                };
                SearchAction::Joined(message.into())
            }
            Err(e) => SearchAction::CommandFailed(format!("join failed: {e}")),
        }
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let role = if self.is_project_scope() {
            "project"
        } else if self.ctx.store == self.invocation_store {
            "user"
        } else {
            "store"
        };
        let pill = render_health_pill(role, &self.health);

        let right_width = span_width(&pill) as u16;

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(HEADER_LEFT_MIN_WIDTH),
                Constraint::Length(right_width),
            ])
            .split(area);

        // Left: brand chip + active view name + store filter indicator.
        let mut left_spans = theme::brand_chip("秘 himitsu");
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(
            "search",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        let scope = self.selected_store().unwrap_or("All");
        left_spans.push(Span::raw("  "));
        left_spans.extend(theme::pill_with(
            format!("▸ {scope}"),
            theme::store_color(scope),
            theme::on_accent(),
        ));
        frame.render_widget(Paragraph::new(Line::from(left_spans)), cols[0]);

        // Right: the active status pill, right-aligned.
        frame.render_widget(
            Paragraph::new(Line::from(pill)).alignment(Alignment::Right),
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
        let mut spans = Vec::new();
        for tag in &self.tag_filters {
            spans.extend(theme::pill_with(
                format!("tag:{tag}"),
                theme::accent(),
                theme::on_accent(),
            ));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::raw(&self.query));
        spans.push(Span::styled("█", Style::default().fg(theme::accent())));
        let text = Line::from(spans);
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn header_label(&self, column: SearchColumn) -> String {
        let mut label = column.label().to_string();
        if self.sort_state.column == column {
            label.push(self.sort_state.direction.marker());
        }
        if self.selected_column == column {
            format!("[{label}]")
        } else {
            label
        }
    }

    fn header_style(&self, column: SearchColumn, base: Style) -> Style {
        if self.selected_column == column {
            base.fg(theme::accent()).add_modifier(Modifier::UNDERLINED)
        } else {
            base
        }
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
        let path_label = self.header_label(SearchColumn::Path);
        let updated_label = self.header_label(SearchColumn::Updated);
        let tags_label = self.header_label(SearchColumn::Tags);
        let store_label = self.header_label(SearchColumn::Store);

        // Pre-compute the rendered cells for each secret row so column
        // widths account for the rendered text, not raw data.
        struct SecretCells {
            indent: usize,
            parent_dim: String,
            basename: String,
            path_display: String,
            path_truncated: bool,
            updated: String,
            tags_display: String,
            tags_truncated: bool,
            store: String,
            tags: Option<Vec<String>>,
        }
        let row_data: Vec<Option<SecretCells>> = self
            .rows
            .iter()
            .map(|row| match row {
                Row::Secret { result, indent, .. } => {
                    let (parent, name) = split_path_basename(&result.path);
                    let path_budget = SEARCH_COLUMN_MAX_WIDTH.saturating_sub(*indent * 2).max(1);
                    let path_display = truncate_middle(&result.path, path_budget);
                    let path_truncated = char_count(&result.path) > char_count(&path_display);
                    let ts = result
                        .updated_at
                        .as_deref()
                        .or(result.created_at.as_deref());
                    let updated_raw = ts
                        .and_then(parse_ts)
                        .map(|t| humanize_age_compact(now, t))
                        .unwrap_or_else(|| "—".to_string());
                    let updated = truncate_middle(&updated_raw, SEARCH_COLUMN_MAX_WIDTH);
                    let tags_text = tag_chips_text(result.tags.as_deref());
                    let tags_display = truncate_middle(&tags_text, SEARCH_COLUMN_MAX_WIDTH);
                    let tags_truncated = char_count(&tags_text) > char_count(&tags_display);
                    let tags = result.tags.clone();
                    let store = truncate_middle(&result.store, SEARCH_COLUMN_MAX_WIDTH);
                    Some(SecretCells {
                        indent: *indent,
                        parent_dim: parent.to_string(),
                        basename: name.to_string(),
                        path_display,
                        path_truncated,
                        updated,
                        tags_display,
                        tags_truncated,
                        store,
                        tags,
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
                    .map(|c| c.indent * 2 + char_count(&c.path_display))
            })
            .max()
            .unwrap_or(0)
            .max(path_label.len());
        let updated_w = row_data
            .iter()
            .filter_map(|d| d.as_ref().map(|c| char_count(&c.updated)))
            .max()
            .unwrap_or(0)
            .max(updated_label.len());
        let tags_w = row_data
            .iter()
            .filter_map(|d| d.as_ref().map(|c| char_count(&c.tags_display)))
            .max()
            .unwrap_or(0)
            .max(tags_label.len());
        let store_w = if show_store {
            row_data
                .iter()
                .filter_map(|d| d.as_ref().map(|c| char_count(&c.store)))
                .max()
                .unwrap_or(0)
                .max(store_label.len())
        } else {
            0
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(HEADER_HEIGHT), Constraint::Min(1)])
            .split(inner);

        let header_style = Style::default()
            .fg(theme::muted())
            .add_modifier(Modifier::BOLD);
        let mut header_spans = vec![
            Span::styled(
                format!("{:<path_w$}  ", path_label, path_w = path_w),
                self.header_style(SearchColumn::Path, header_style),
            ),
            Span::styled(
                format!("{:<updated_w$}  ", updated_label, updated_w = updated_w),
                self.header_style(SearchColumn::Updated, header_style),
            ),
            Span::styled(
                format!("{:<tags_w$}  ", tags_label, tags_w = tags_w),
                self.header_style(SearchColumn::Tags, header_style),
            ),
        ];
        if show_store {
            header_spans.push(Span::styled(
                format!("{:<store_w$}", store_label, store_w = store_w),
                self.header_style(SearchColumn::Store, header_style),
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
                Row::FoldedGroup {
                    prefix,
                    count,
                    indent,
                } => {
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
                Row::Secret { shared_prefix, .. } => {
                    let cells = data.as_ref().unwrap();
                    let indent = "  ".repeat(cells.indent);
                    let consumed = indent.len() + char_count(&cells.path_display);
                    let pad = path_w.saturating_sub(consumed);
                    let mut spans = vec![Span::raw(indent)];
                    if cells.path_truncated {
                        spans.push(Span::raw(cells.path_display.clone()));
                    } else {
                        let (shared_seg, rest_seg) =
                            split_shared_prefix(&cells.parent_dim, shared_prefix.as_deref());
                        spans.push(Span::styled(
                            shared_seg.to_string(),
                            Style::default()
                                .fg(theme::accent())
                                .add_modifier(Modifier::DIM),
                        ));
                        spans.push(Span::styled(
                            rest_seg.to_string(),
                            Style::default().fg(theme::path_dim()),
                        ));
                        spans.push(Span::raw(cells.basename.clone()));
                    }
                    spans.extend([
                        Span::raw(format!("{:<pad$}  ", "", pad = pad)),
                        Span::styled(
                            format!("{:<updated_w$}  ", cells.updated, updated_w = updated_w),
                            Style::default().fg(theme::muted()),
                        ),
                    ]);
                    // Colorized per-tag chips in the TAGS column. When the
                    // chip text overflows the column budget, fall back to the
                    // plain truncated string so the rendered width always
                    // matches `tags_display` (which sized the column).
                    let tag_consumed = char_count(&cells.tags_display);
                    let tag_pad = tags_w.saturating_sub(tag_consumed);
                    if cells.tags_truncated {
                        spans.push(Span::styled(
                            cells.tags_display.clone(),
                            Style::default().fg(theme::accent()),
                        ));
                    } else {
                        spans.extend(format_colored_tag_chips(cells.tags.as_deref()));
                    }
                    spans.push(Span::raw(format!("{:<tag_pad$}  ", "", tag_pad = tag_pad)));
                    if show_store {
                        let store_style = if cells.store == self.default_store_slug() {
                            Style::default().fg(theme::accent())
                        } else {
                            Style::default().fg(theme::store_color(&cells.store))
                        };
                        spans.push(Span::styled(
                            format!("{:<store_w$}", cells.store, store_w = store_w),
                            store_style,
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
                    Span::styled(" nav", footer),
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
                    Span::styled("^i", Style::default().fg(theme::accent())),
                    Span::styled(" store", footer),
                ]),
                Line::from(vec![
                    Span::styled("esc", Style::default().fg(theme::accent())),
                    Span::styled(" quit", footer),
                ]),
            ],
        );
    }

    fn draw_selected_description(&self, frame: &mut Frame<'_>) {
        let Some(desc) = self
            .selected_result()
            .and_then(|result| result.description.as_deref())
            .filter(|desc| !desc.is_empty())
        else {
            return;
        };

        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let strip = Rect {
            x: area.x,
            y: area.y + area.height - 1,
            width: area.width,
            height: 1,
        };
        let display = truncate_middle(desc, area.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                display,
                Style::default().fg(theme::muted()),
            )))
            .alignment(Alignment::Right),
            strip,
        );
    }
}

/// Render a secret's tag list as a sequence of colorized `[tag]` spans
/// suitable for the TAGS column in the search table. Each tag gets a
/// deterministic per-tag color via [`theme::tag_color`] so different tags
/// are visually distinguishable at a glance.
///
/// Returns an empty `Vec` when there are no tags to show. The rendered
/// text is exactly [`tag_chips_text`] — `[a] [b]` with single-space
/// separators and no trailing space — so column width math stays exact.
fn format_colored_tag_chips(tags: Option<&[String]>) -> Vec<Span<'static>> {
    let Some(tags) = tags else {
        return Vec::new();
    };
    let mut spans = Vec::with_capacity(tags.len() * 2);
    for (i, tag) in tags.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!("[{tag}]"),
            Style::default().fg(theme::tag_color(tag)),
        ));
    }
    spans
}

/// Plain-text form of the tag chip cell (`[a] [b]`), used for column
/// width computation and truncation decisions. Must stay in sync with
/// [`format_colored_tag_chips`].
fn tag_chips_text(tags: Option<&[String]>) -> String {
    let Some(tags) = tags else {
        return String::new();
    };
    tags.iter()
        .map(|t| format!("[{t}]"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let value_chars = char_count(value);
    if value_chars <= max_chars {
        return value.to_string();
    }
    let marker_chars = char_count(TRUNCATION_MARKER);
    if max_chars <= marker_chars {
        return TRUNCATION_MARKER
            .chars()
            .take(max_chars)
            .collect::<String>();
    }

    let keep = max_chars - marker_chars;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let start: String = value.chars().take(head).collect();
    let mut end_chars: Vec<char> = value.chars().rev().take(tail).collect();
    end_chars.reverse();
    let end: String = end_chars.into_iter().collect();
    format!("{start}{TRUNCATION_MARKER}{end}")
}

fn char_count(value: &str) -> usize {
    value.chars().count()
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

/// Search view's keymap action priority. Quit comes first so a user who
/// rebinds an action to a printable character still has an escape hatch.
/// `CopyRefSelected` is listed before `CopySelected` because their
/// default bindings overlap on the bare `y` key: shift-insensitive
/// matching means a `y` binding would otherwise claim `Shift+Y` first.
const SEARCH_ACTION_PRIORITY: &[KeyAction] = &[
    KeyAction::Quit,
    KeyAction::CommandPalette,
    KeyAction::NewSecret,
    KeyAction::Outputs,
    KeyAction::SwitchStore,
    KeyAction::CopyRefSelected,
    KeyAction::CopySelected,
    KeyAction::CollapsePaths,
    KeyAction::ExpandPaths,
    KeyAction::RefineTag,
    KeyAction::SortColumn,
    KeyAction::CycleStore,
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
    let payload = store::read_secret_payload(&result.store_path, &result.path)?;
    let identities = ctx_for_store.load_identities()?;
    let plain = match age::decrypt_with_identities(&payload.ciphertext, &identities) {
        Ok(plain) => plain,
        Err(_) if payload.legacy_proto_envelope => payload.ciphertext,
        Err(err) => return Err(err),
    };
    let decoded =
        secret_value::decode_with_legacy_environment(&plain, payload.legacy_environment.as_deref());
    Ok(String::from_utf8_lossy(&decoded.data).into_owned())
}

fn span_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
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

// ── Help overlay integration (US-012) ─────────────────────────────────
//
// In its own impl block so parallel branches adding new bindings can extend
// `help_entries` without colliding with the main impl.
impl SearchView {
    /// Help rows: static navigation keys plus every rebindable action,
    /// rendered from the LIVE keymap so user rebinds show up here.
    pub fn help_entries(keymap: &KeyMap) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = vec![
            ("type".into(), "filter results".into()),
            ("↑/↓".into(), "navigate".into()),
            ("tab / shift-tab".into(), "select column".into()),
            ("enter".into(), "open selection".into()),
            ("backspace".into(), "delete char".into()),
            ("^i".into(), "cycle header status (info)".into()),
        ];
        rows.extend(crate::tui::keymap::help_rows(
            keymap,
            crate::tui::keymap::Scope::Search,
        ));
        rows.extend(crate::tui::keymap::help_rows(
            keymap,
            crate::tui::keymap::Scope::Global,
        ));
        rows
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
            key_provider: crate::config::KeyProvider::default(),
            project_root: None,
            git: std::sync::Arc::new(crate::git::CliGitAdapter),
            project_config_cell: Default::default(),
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

    fn seeded_sort_store() -> TempDir {
        let dir = TempDir::new().unwrap();
        let store = dir.path().join("store");
        for prefix in ["alpha", "beta", "gamma"] {
            std::fs::create_dir_all(store.join(format!(".himitsu/secrets/{prefix}"))).unwrap();
        }

        let write_secret = |path: &str, created_at: &str| {
            std::fs::write(
                store.join(format!(".himitsu/secrets/{path}.yaml")),
                format!(
                    "value: ENC[age,placeholder]\nhimitsu:\n  created_at: '{created_at}'\n  lastmodified: '{created_at}T00:00:00Z'\n  age: []\n  history: []\n"
                ),
            )
            .unwrap();
        };
        write_secret("alpha/KEY", "2026-01-03");
        write_secret("beta/KEY", "2026-01-01");
        write_secret("gamma/KEY", "2026-01-02");

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
    fn ctrl_t_adds_first_selected_tag_filter() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        view.results = vec![SearchResult {
            store: "local".into(),
            store_path: ctx.store.clone(),
            path: "prod/API_KEY".into(),
            created_at: None,
            updated_at: None,
            description: None,
            tags: Some(vec!["pci".into(), "prod".into()]),
        }];
        view.rows = build_rows(&view.results, false, view.sort_state);
        view.list_state.select(Some(0));

        let action = view
            .dispatch_action(KeyAction::RefineTag, &KeyMap::default())
            .unwrap();
        assert_eq!(view.tag_filters, vec!["pci"]);
        assert!(matches!(action, SearchAction::CommandHint(msg) if msg.contains("tag:pci")));
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
    fn ctrl_minus_collapses_and_ctrl_plus_expands_groups() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        // Unfolded baseline: 3 leaves (2 grouped + 1 single).
        assert_eq!(view.rows.len(), 3);

        view.dispatch_action(KeyAction::CollapsePaths, &km);
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

        view.dispatch_action(KeyAction::ExpandPaths, &km);
        assert_eq!(view.rows.len(), 3);
    }

    #[test]
    fn store_cycle_tab_depends_on_keyboard_protocol() {
        let km = KeyMap::default();
        let dir = seeded_multi_store();
        let ctx = multi_ctx(dir.path());
        let tab = key(KeyCode::Tab);

        let mut enhanced = SearchView::new(&ctx);
        let all = enhanced.results.len();
        enhanced.on_results_key(tab, &km, false);
        assert_eq!(enhanced.results.len(), all);
        assert_eq!(enhanced.selected_column, SearchColumn::Updated);
        enhanced.on_results_key(ctrl('i'), &km, false);
        let filtered = enhanced.results.len();
        assert!(filtered < all, "enhanced Ctrl+I must cycle stores");
        assert!(enhanced.query.is_empty());

        let mut legacy = SearchView::new(&ctx);
        legacy.on_results_key(tab, &km, true);
        assert_eq!(
            legacy.results.len(),
            filtered,
            "legacy 0x09 must also cycle"
        );
        assert_eq!(legacy.selected_column, SearchColumn::Path);
        assert!(legacy.query.is_empty());

        // CSI Z is BackTab + SHIFT. It wraps through every visible column.
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        let columns = legacy.visible_columns();
        for column in columns.iter().rev() {
            legacy.on_results_key(backtab, &km, true);
            assert_eq!(legacy.selected_column, *column);
        }
        legacy.on_results_key(key(KeyCode::Right), &km, true);
        assert_eq!(legacy.selected_column, SearchColumn::Updated);
        legacy.on_results_key(key(KeyCode::Left), &km, true);
        assert_eq!(legacy.selected_column, SearchColumn::Path);
        let shifted_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        legacy.on_results_key(shifted_tab, &km, true);
        assert_eq!(legacy.selected_column, *columns.last().unwrap());
        assert_eq!(legacy.results.len(), filtered);
    }

    #[test]
    fn store_cycle_tab_respects_configured_bindings() {
        let dir = seeded_multi_store();
        let ctx = multi_ctx(dir.path());
        let mut view = SearchView::new(&ctx);
        let all = view.results.len();
        let km: KeyMap = serde_yaml::from_str("cycle_store: ['ctrl+u']").unwrap();
        view.on_results_key(key(KeyCode::Tab), &km, true);
        assert_eq!(view.results.len(), all);
        assert_eq!(view.selected_column, SearchColumn::Updated);
        view.on_results_key(ctrl('u'), &km, true);
        assert!(view.results.len() < all);

        let km: KeyMap = serde_yaml::from_str("new_secret: [tab]\ncodegen: ['shift+tab']").unwrap();
        assert!(matches!(
            view.on_results_key(key(KeyCode::Tab), &km, true),
            SearchAction::NewSecret
        ));
        for code in [KeyCode::Tab, KeyCode::BackTab] {
            let shift_tab = KeyEvent::new(code, KeyModifiers::SHIFT);
            assert!(matches!(
                view.on_results_key(shift_tab, &km, true),
                SearchAction::OpenOutputs
            ));
        }
    }

    #[test]
    fn tab_and_shift_tab_move_the_selected_column_without_folding() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let km = KeyMap::default();
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

        let rendered = render(&mut view);
        assert!(
            rendered.contains("[PATH^]"),
            "PATH should start selected and sorted: {rendered}"
        );
        assert_eq!(view.rows.len(), 3);

        view.on_key(key(KeyCode::Tab), &km);
        assert_eq!(
            view.rows.len(),
            3,
            "Tab should move columns, not fold groups"
        );
        let rendered = render(&mut view);
        assert!(
            rendered.contains("[UPDATED]"),
            "Tab should select the UPDATED column: {rendered}"
        );

        view.on_key(key(KeyCode::BackTab), &km);
        let rendered = render(&mut view);
        assert!(
            rendered.contains("[PATH^]"),
            "Shift+Tab should move back to PATH: {rendered}"
        );
    }

    #[test]
    fn ctrl_o_sorts_by_the_selected_column_and_toggles_direction() {
        let km = KeyMap::default();
        let dir = seeded_sort_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        for result in &mut view.results {
            result.updated_at = Some(
                match result.path.as_str() {
                    "alpha/KEY" => "2026-01-03T00:00:00Z",
                    "beta/KEY" => "2026-01-01T00:00:00Z",
                    "gamma/KEY" => "2026-01-02T00:00:00Z",
                    other => panic!("unexpected path in sort fixture: {other}"),
                }
                .to_string(),
            );
        }
        view.rows = build_rows(&view.results, false, view.sort_state);

        let paths = |view: &SearchView| -> Vec<String> {
            view.rows
                .iter()
                .filter_map(|row| match row {
                    Row::Secret { result, .. } => Some(result.path.clone()),
                    _ => None,
                })
                .collect()
        };

        assert_eq!(paths(&view), vec!["alpha/KEY", "beta/KEY", "gamma/KEY"]);

        view.on_key(key(KeyCode::Tab), &km);
        view.on_key(ctrl('o'), &km);
        assert_eq!(paths(&view), vec!["beta/KEY", "gamma/KEY", "alpha/KEY"]);

        view.on_key(ctrl('o'), &km);
        assert_eq!(paths(&view), vec!["alpha/KEY", "gamma/KEY", "beta/KEY"]);
    }

    #[test]
    fn folded_group_is_selectable_and_enter_unfolds() {
        let km = KeyMap::default();
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);

        view.dispatch_action(KeyAction::CollapsePaths, &km);
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
        view.refresh_results(); // debounce flush
        assert!(
            view.results
                .iter()
                .all(|r| r.path.to_lowercase().contains("dat"))
        );
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
        view.refresh_results(); // debounce flush
        assert_eq!(view.results.len(), 1);
        view.on_key(key(KeyCode::Backspace), &km);
        view.on_key(key(KeyCode::Backspace), &km);
        view.on_key(key(KeyCode::Backspace), &km);
        view.refresh_results(); // debounce flush
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
        view.refresh_results(); // debounce flush
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

    fn render_view(view: &mut SearchView, width: u16, height: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(width, height);
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
    }

    fn set_single_result(view: &mut SearchView, result: SearchResult) {
        view.results = vec![result];
        view.rows = build_rows(&view.results, false, view.sort_state);
        view.list_state.select(Some(0));
    }

    #[test]
    fn selected_description_renders_on_bottom_strip_not_as_expanded_row() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        let desc = "selected database credential for production failover";
        set_single_result(
            &mut view,
            SearchResult {
                store: "local".into(),
                store_path: ctx.store.clone(),
                path: "prod/API_KEY".into(),
                created_at: None,
                updated_at: None,
                description: Some(desc.into()),
                tags: None,
            },
        );

        let rendered = render_view(&mut view, 120, 20);
        let lines: Vec<&str> = rendered.lines().collect();
        let row_idx = lines
            .iter()
            .position(|line| line.contains("prod/API_KEY"))
            .expect("selected row should render");

        assert!(
            !lines[row_idx + 1].contains(desc),
            "description should not expand below the selected row:\n{rendered}"
        );
        assert!(
            lines.last().is_some_and(|line| line.contains(desc)),
            "description should render on the bottom strip:\n{rendered}"
        );
    }

    #[test]
    fn search_columns_cap_long_cells_with_middle_truncation() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        let path = "prod/very-long-secret-name-with-middle-and-baz";
        set_single_result(
            &mut view,
            SearchResult {
                store: "local".into(),
                store_path: ctx.store.clone(),
                path: path.into(),
                created_at: None,
                updated_at: None,
                description: None,
                tags: None,
            },
        );

        let rendered = render_view(&mut view, 120, 20);
        let row = rendered
            .lines()
            .find(|line| line.contains("prod/very-long-") || line.contains(path))
            .expect("secret row should render");

        assert!(
            row.contains("prod/very-long-..-middle-and-baz"),
            "path should be middle-truncated after 32 chars:\n{row}"
        );
        assert!(
            !row.contains(path),
            "row should not contain the raw long path:\n{row}"
        );
    }

    #[test]
    fn tags_cell_caps_long_chip_text_with_middle_truncation() {
        let dir = seeded_store();
        let ctx = make_ctx(&dir.path().join("store"));
        let mut view = SearchView::new(&ctx);
        let long_tag = "very-long-tag-name-exceeding-the-column-budget";
        set_single_result(
            &mut view,
            SearchResult {
                store: "local".into(),
                store_path: ctx.store.clone(),
                path: "prod/API_KEY".into(),
                created_at: None,
                updated_at: None,
                description: None,
                tags: Some(vec![long_tag.to_string()]),
            },
        );

        let rendered = render_view(&mut view, 120, 20);
        let row = rendered
            .lines()
            .find(|line| line.contains("prod/API_KEY"))
            .expect("secret row should render");
        assert!(
            !row.contains(long_tag),
            "row should not contain the raw long tag:\n{row}"
        );
        assert!(
            row.contains(TRUNCATION_MARKER),
            "long tag chip text should be middle-truncated:\n{row}"
        );
    }

    #[test]
    fn column_headers_are_rendered_above_results() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

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

        // Default render: PATH / UPDATED / TAGS are always
        // shown; STORE is hidden until the user toggles it on via the
        // command palette.
        let rendered = render(&mut view);
        assert!(rendered.contains("PATH"), "missing PATH header: {rendered}");
        assert!(
            rendered.contains("UPDATED"),
            "missing UPDATED header: {rendered}"
        );
        assert!(rendered.contains("TAGS"), "missing TAGS header: {rendered}");
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
        assert!(
            !rendered.contains("TAGSSTORE"),
            "TAGS and STORE headers must stay separated: {rendered}"
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
            key_provider: crate::config::KeyProvider::default(),
            project_root: None,
            git: std::sync::Arc::new(crate::git::CliGitAdapter),
            project_config_cell: Default::default(),
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
            key_provider: crate::config::KeyProvider::default(),
            project_root: None,
            git: std::sync::Arc::new(crate::git::CliGitAdapter),
            project_config_cell: Default::default(),
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
        view.refresh_results(); // debounce flush
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
        let action = view.dispatch_action(KeyAction::SwitchStore, &km).unwrap();
        assert!(matches!(action, SearchAction::None));
        assert!(view.picker.is_some());
    }

    /// Concatenate the rendered text of a span list. Used by the chip tests
    /// so they can assert on the visible output without caring about styling.
    fn spans_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn format_colored_tag_chips_renders_each_label() {
        let tags = vec!["pci".to_string(), "stripe".to_string()];
        let spans = format_colored_tag_chips(Some(&tags));
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
    fn format_colored_tag_chips_returns_empty_when_tags_none() {
        let spans = format_colored_tag_chips(None);
        assert!(
            spans.is_empty(),
            "expected no chips for None, got {spans:?}"
        );
    }

    #[test]
    fn format_colored_tag_chips_returns_empty_when_tag_list_empty() {
        let tags: Vec<String> = Vec::new();
        let spans = format_colored_tag_chips(Some(&tags));
        assert!(
            spans.is_empty(),
            "expected no chips for empty list, got {spans:?}"
        );
    }

    #[test]
    fn colored_tag_chips_text_matches_plain_cell_text() {
        let tags = vec!["pci".to_string(), "stripe".to_string(), "a".to_string()];
        let spans = format_colored_tag_chips(Some(&tags));
        assert_eq!(
            spans_text(&spans),
            tag_chips_text(Some(&tags)),
            "rendered chip text must match the width-computation text"
        );
        assert_eq!(tag_chips_text(None), "");
        assert_eq!(spans_text(&format_colored_tag_chips(None)), "");
    }

    #[test]
    fn tag_color_is_deterministic_per_tag() {
        let a1 = theme::tag_color("pci");
        let a2 = theme::tag_color("pci");
        assert_eq!(a1, a2, "same tag must always map to the same color");
        // Not all tags may differ (small palette), but these two known
        // inputs hash to different palette slots.
        assert_ne!(
            theme::tag_color("pci"),
            theme::tag_color("stripe"),
            "distinct tags should usually get distinct colors"
        );
    }

    #[test]
    fn ctrl_i_cycles_store_filter() {
        let km = KeyMap::default();
        let dir = seeded_multi_store();
        let ctx = multi_ctx(dir.path());
        let mut view = SearchView::new(&ctx);

        assert!(view.selected_store().is_none());
        assert_eq!(view.results.len(), 3);

        view.on_key(ctrl('i'), &km);
        assert_eq!(view.active_context().store, ctx.store);
        assert!(
            view.results.is_empty(),
            "the explicit empty checkout is a scope"
        );
        for (slug, count) in [("acme/alpha", 2), ("acme/beta", 1)] {
            view.on_key(ctrl('i'), &km);
            assert_eq!(view.selected_store(), Some(slug));
            assert_eq!(view.results.len(), count);
        }
        view.on_key(ctrl('i'), &km);
        assert!(view.selected_store().is_none());
        assert_eq!(view.results.len(), 3);
    }

    #[test]
    fn draw_header_shows_store_filter_pill_when_active() {
        let dir = seeded_multi_store();
        let ctx = multi_ctx(dir.path());
        let mut view = SearchView::new(&ctx);

        // All shows the invocation store's health, not two statuses.
        let rendered = render_view(&mut view, 140, 20);
        assert!(
            rendered.contains("user:") && !rendered.contains("project:"),
            "expected a single health status:\n{rendered}"
        );
        assert!(
            rendered.contains("▸ All"),
            "expected All as the initial scope:\n{rendered}"
        );

        // Skip past the explicit empty checkout to the first registered store.
        view.on_key(ctrl('i'), &KeyMap::default());
        view.on_key(ctrl('i'), &KeyMap::default());
        let rendered = render_view(&mut view, 160, 20);
        assert!(
            rendered.contains("▸ acme/alpha") && !rendered.contains("user:"),
            "header should show store filter pill when active:\n{rendered}"
        );
    }
}
