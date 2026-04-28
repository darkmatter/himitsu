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

use crate::tui::theme;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use chrono::Utc;

use crate::cli::search::{humanize_age, parse_ts, search_core, SearchResult};
use crate::cli::Context;
use crate::crypto::{age, secret_value};
use crate::remote::store;
use crate::tui::icons;
use crate::tui::keymap::{Bindings, KeyMap};
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
}

/// A row in the rendered results list. Store and Folder rows are visual-only
/// headers — they group the secrets that follow and are never selectable;
/// navigation steps over them. Stores group by origin (`org/repo` slug or
/// local path); folders group adjacent secrets sharing a top-level path
/// prefix within a store.
#[derive(Debug, Clone)]
enum Row {
    Store {
        name: String,
        count: usize,
    },
    Folder {
        name: String,
        count: usize,
    },
    Secret {
        result: SearchResult,
        /// Indentation depth in list-item cells (2 spaces per level). Level
        /// 0 = flat, 1 = under one header (folder or store), 2 = under both.
        indent: usize,
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
}

impl SearchView {
    pub fn new(ctx: &Context) -> Self {
        let ctx_owned = Context {
            data_dir: ctx.data_dir.clone(),
            state_dir: ctx.state_dir.clone(),
            store: ctx.store.clone(),
            recipients_path: ctx.recipients_path.clone(),
        };
        let store_health = check_store_health(&ctx_owned.store);
        let mut view = Self {
            query: String::new(),
            results: Vec::new(),
            rows: Vec::new(),
            list_state: ListState::default(),
            ctx: ctx_owned,
            picker: None,
            palette: None,
            store_health,
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
        if keymap.quit.matches(&key) {
            return SearchAction::Quit;
        }
        if keymap.command_palette.matches(&key) {
            self.palette = Some(CommandPalette::new());
            return SearchAction::None;
        }
        if keymap.new_secret.matches(&key) {
            return SearchAction::NewSecret;
        }
        if keymap.envs.matches(&key) {
            return SearchAction::OpenEnvs;
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

        // Always require a small margin around the center column
        let chunks = Layout::default()
            .constraints(Constraint::from_mins([0]))
            .horizontal_margin(4)
            .vertical_margin(4)
            .split(area);
        let with_margin = chunks[0];
        // Allow resizing within a constraint. When developing, alaways develop against
        // the minimum size.
        let center_column = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Fill(1),
                Constraint::Max(80),
                Constraint::Fill(1),
            ])
            .split(with_margin)[1];
        let center_row = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Max(30), //
                Constraint::Fill(1),
            ])
            .split(center_column)[1];
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
            .split(center_row);

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
        match cmd {
            Command::NewSecret => SearchAction::NewSecret,
            Command::SwitchStore => {
                self.picker = Some(StorePicker::new(
                    &self.ctx.stores_dir(),
                    self.ctx.store.clone(),
                ));
                SearchAction::None
            }
            Command::Envs => SearchAction::OpenEnvs,
            Command::Help => SearchAction::ShowHelp,
            Command::Quit => SearchAction::Quit,
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
        );
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

        let now = Utc::now();

        // Pre-compute humanized timestamps and descriptions for each secret
        // row so column widths account for the rendered text, not raw data.
        let row_data: Vec<Option<(String, String, String)>> = self
            .rows
            .iter()
            .map(|row| match row {
                Row::Secret { result, indent } => {
                    let prefix = "  ".repeat(*indent);
                    let padded_path = format!("{prefix}{}", result.path);
                    let ts = result
                        .updated_at
                        .as_deref()
                        .or(result.created_at.as_deref());
                    let updated = ts
                        .and_then(parse_ts)
                        .map(|t| humanize_age(now, t))
                        .unwrap_or_else(|| "—".to_string());
                    let desc = result.description.clone().unwrap_or_default();
                    Some((padded_path, updated, desc))
                }
                _ => None,
            })
            .collect();

        // Column widths are computed against the widest secret row and the
        // header label itself, so short paths still leave room for "PATH" /
        // "STORE" to read cleanly.
        let path_w = row_data
            .iter()
            .filter_map(|d| d.as_ref().map(|(p, _, _)| p.len()))
            .max()
            .unwrap_or(0)
            .max("PATH".len());
        let updated_w = row_data
            .iter()
            .filter_map(|d| d.as_ref().map(|(_, u, _)| u.len()))
            .max()
            .unwrap_or(0)
            .max("UPDATED".len());
        let desc_w = row_data
            .iter()
            .filter_map(|d| d.as_ref().map(|(_, _, d)| d.len()))
            .max()
            .unwrap_or(0)
            .max("DESCRIPTION".len());
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
                format!("{:<desc_w$}  ", "DESCRIPTION", desc_w = desc_w),
                header_style,
            ),
        ];
        if !has_store_headers {
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
                Row::Folder { name, count } => {
                    let line = Line::from(vec![
                        Span::styled(
                            format!("▸ {name}/"),
                            Style::default()
                                .fg(theme::warning())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(format!("({count})"), Style::default().fg(theme::muted())),
                    ]);
                    ListItem::new(line)
                }
                Row::Secret { result, .. } => {
                    let (padded_path, updated, desc) = data.as_ref().unwrap();
                    let mut spans = vec![
                        Span::raw(format!("{padded_path:<path_w$}  ")),
                        Span::styled(
                            format!("{updated:<updated_w$}  "),
                            Style::default().fg(theme::muted()),
                        ),
                        Span::raw(format!("{desc:<desc_w$}  ")),
                    ];
                    if !has_store_headers {
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
        let line = Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(theme::accent())),
            Span::styled(" navigate    ", footer),
            Span::styled("enter", Style::default().fg(theme::accent())),
            Span::styled(" open    ", footer),
            Span::styled("^n", Style::default().fg(theme::accent())),
            Span::styled(" new    ", footer),
            Span::styled("^y", Style::default().fg(theme::accent())),
            Span::styled(" copy    ", footer),
            Span::styled("^p", Style::default().fg(theme::accent())),
            Span::styled(" commands    ", footer),
            Span::styled("esc", Style::default().fg(theme::accent())),
            Span::styled(" quit", footer),
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

/// Check the git health of a store checkout (offline — no fetch).
///
/// Returns a [`StoreHealth`] summarising whether the checkout is behind its
/// remote tracking branch and/or has uncommitted local changes. The check
/// is intentionally cheap: it only inspects already-fetched refs so it
/// never blocks on the network.
fn check_store_health(store_path: &std::path::Path) -> StoreHealth {
    use crate::git;

    if store_path.as_os_str().is_empty() {
        return StoreHealth::Unknown;
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
        assert!(
            rendered.contains("UPDATED"),
            "missing UPDATED header: {rendered}"
        );
        assert!(
            rendered.contains("DESCRIPTION"),
            "missing DESCRIPTION header: {rendered}"
        );
        assert!(
            rendered.contains("STORE"),
            "missing STORE header: {rendered}"
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
}
