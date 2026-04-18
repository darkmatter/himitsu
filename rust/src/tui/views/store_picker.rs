//! Store picker — overlay for switching the dashboard's active store without
//! restarting the TUI.
//!
//! # Scope note (US-013)
//!
//! himitsu's config layer DOES support multiple stores: they live under
//! `state_dir()/stores/<org>/<repo>` (see [`crate::config::stores_dir`]), and
//! [`crate::config::resolve_store`] already knows how to pick between them.
//! This picker exposes that list in the TUI and also lets the user type an
//! arbitrary path to a store on disk (e.g. a checkout outside the managed
//! stores dir, or a `~`-prefixed path).
//!
//! The picker does NOT persist the choice to `config.yaml` — the switch is
//! in-memory for the current TUI session only. Persisting "last used store"
//! or a full multi-context UX is a separate, larger effort and is out of
//! scope for this bead (see US-013 notes).
//!
//! The picker is a self-contained state machine, hosted by `SearchView`
//! as an optional overlay. Tests exercise the state machine directly without
//! spinning up a real terminal — see the `tests` module below.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

/// Outcome of forwarding a key event to the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorePickerOutcome {
    /// Picker is still open, no action needed.
    Pending,
    /// User pressed Esc — close the picker with no store change.
    Cancelled,
    /// User selected or typed a valid store path — close the picker and
    /// apply the switch.
    Selected(PathBuf),
}

/// Which side of the picker has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    /// Focus is on the list of known store checkouts.
    List,
    /// Focus is on the free-form path input.
    Input,
}

/// Store picker overlay state.
pub struct StorePicker {
    /// Paths discovered under `stores_dir()`, shown as the quick-pick list.
    entries: Vec<StoreEntry>,
    /// Selection state for `entries`.
    list_state: ListState,
    /// Current free-form path input buffer.
    input: String,
    /// Which pane is focused.
    focus: Focus,
    /// Last validation error, if any — cleared on the next key press.
    error: Option<String>,
    /// The store path currently in use (for the "current" marker).
    current: PathBuf,
}

/// An entry in the quick-pick list — a known store checkout.
#[derive(Debug, Clone)]
struct StoreEntry {
    /// Display slug (e.g. `acme/secrets`) relative to `stores_dir()`.
    slug: String,
    /// Absolute path to the store checkout.
    path: PathBuf,
}

impl StorePicker {
    /// Build a new picker, enumerating known store checkouts under
    /// `stores_dir`. The caller-supplied `current` path is rendered with a
    /// marker in the list so the user can see what they're replacing.
    pub fn new(stores_dir: &Path, current: PathBuf) -> Self {
        let entries = enumerate_stores(stores_dir);
        let mut list_state = ListState::default();
        if !entries.is_empty() {
            // If the current store is one of the enumerated entries, start
            // the cursor on it; otherwise start at the top.
            let start = entries.iter().position(|e| e.path == current).unwrap_or(0);
            list_state.select(Some(start));
        }
        // When there are no managed stores, start with focus on the input —
        // otherwise the user would have to press Tab before they could type.
        let focus = if entries.is_empty() {
            Focus::Input
        } else {
            Focus::List
        };
        Self {
            entries,
            list_state,
            input: String::new(),
            focus,
            error: None,
            current,
        }
    }

    /// Handle a key event. Returns the updated outcome; the caller inspects
    /// the return value to decide whether to close the overlay.
    pub fn on_key(&mut self, key: KeyEvent) -> StorePickerOutcome {
        // Any real input clears the last error message.
        self.error = None;

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => StorePickerOutcome::Cancelled,

            // Toggle focus between list and input.
            (KeyCode::Tab, _) => {
                self.toggle_focus();
                StorePickerOutcome::Pending
            }

            // Navigation and submit depend on focus.
            (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) if self.focus == Focus::List => {
                self.select_prev();
                StorePickerOutcome::Pending
            }
            (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE)
                if self.focus == Focus::List =>
            {
                self.select_next();
                StorePickerOutcome::Pending
            }

            (KeyCode::Enter, _) => self.submit(),

            // Free-form input editing (only while focused on the input).
            (KeyCode::Backspace, _) if self.focus == Focus::Input => {
                self.input.pop();
                StorePickerOutcome::Pending
            }
            (KeyCode::Char(c), mods)
                if self.focus == Focus::Input && !mods.contains(KeyModifiers::CONTROL) =>
            {
                self.input.push(c);
                StorePickerOutcome::Pending
            }

            _ => StorePickerOutcome::Pending,
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::List if !self.entries.is_empty() => Focus::Input,
            Focus::Input if !self.entries.is_empty() => Focus::List,
            other => other,
        };
    }

    fn select_prev(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = if i == 0 {
            self.entries.len() - 1
        } else {
            i - 1
        };
        self.list_state.select(Some(next));
    }

    fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1) % self.entries.len();
        self.list_state.select(Some(next));
    }

    /// Resolve the user's submission into an `Outcome`. On input focus we
    /// interpret the typed path (expanding `~`); on list focus we use the
    /// selected entry.
    fn submit(&mut self) -> StorePickerOutcome {
        let candidate = match self.focus {
            Focus::List => self
                .list_state
                .selected()
                .and_then(|i| self.entries.get(i))
                .map(|e| e.path.clone()),
            Focus::Input => {
                let trimmed = self.input.trim();
                if trimmed.is_empty() {
                    self.error = Some("path is empty".to_string());
                    return StorePickerOutcome::Pending;
                }
                Some(expand_tilde(trimmed))
            }
        };

        let Some(path) = candidate else {
            self.error = Some("nothing selected".to_string());
            return StorePickerOutcome::Pending;
        };

        match validate_store(&path) {
            Ok(()) => StorePickerOutcome::Selected(path),
            Err(msg) => {
                self.error = Some(msg);
                StorePickerOutcome::Pending
            }
        }
    }

    /// Expose the current input buffer (tests).
    #[cfg(test)]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Expose the current error (tests).
    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Render the picker as a centred overlay. The caller is responsible for
    /// ensuring the underlying view has already been drawn.
    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let full = frame.area();
        let area = centered_rect(60, 60, full);
        frame.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" switch store ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // list
                Constraint::Length(3), // input
                Constraint::Length(2), // status/footer
            ])
            .split(inner);

        self.draw_list(frame, rows[0]);
        self.draw_input(frame, rows[1]);
        self.draw_footer(frame, rows[2]);
    }

    fn draw_list(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let title = if self.focus == Focus::List {
            " stores [focused] "
        } else {
            " stores "
        };
        let block = Block::default().borders(Borders::ALL).title(title);

        if self.entries.is_empty() {
            let msg = Paragraph::new(Line::from(Span::styled(
                "  no managed stores — type a path below",
                Style::default().fg(Color::DarkGray),
            )))
            .block(block);
            frame.render_widget(msg, area);
            return;
        }

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|e| {
                let marker = if e.path == self.current { "• " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(marker, Style::default().fg(Color::Cyan)),
                    Span::raw(e.slug.clone()),
                ]))
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

    fn draw_input(&self, frame: &mut Frame<'_>, area: Rect) {
        let title = if self.focus == Focus::Input {
            " path [focused] "
        } else {
            " path "
        };
        let block = Block::default().borders(Borders::ALL).title(title);
        let display = if self.focus == Focus::Input {
            format!("{}_", self.input)
        } else {
            self.input.clone()
        };
        frame.render_widget(Paragraph::new(display).block(block), area);
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = if let Some(err) = &self.error {
            Line::from(Span::styled(
                format!("error: {err}"),
                Style::default().fg(Color::Red),
            ))
        } else {
            Line::from(vec![
                Span::styled("tab", Style::default().fg(Color::Cyan)),
                Span::raw(" switch focus  "),
                Span::styled("enter", Style::default().fg(Color::Cyan)),
                Span::raw(" select  "),
                Span::styled("esc", Style::default().fg(Color::Cyan)),
                Span::raw(" cancel"),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }
}

/// Enumerate `<stores_dir>/<org>/<repo>` checkouts, sorted by slug.
fn enumerate_stores(stores_dir: &Path) -> Vec<StoreEntry> {
    let mut entries = Vec::new();
    let Ok(read) = std::fs::read_dir(stores_dir) else {
        return entries;
    };
    for org_res in read {
        let Ok(org) = org_res else { continue };
        let Ok(ft) = org.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let Ok(repos) = std::fs::read_dir(org.path()) else {
            continue;
        };
        for repo_res in repos {
            let Ok(repo) = repo_res else { continue };
            let Ok(ft) = repo.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let path = repo.path();
            // Only surface entries that actually look like a store; silently
            // skip anything else so we don't dangle broken options.
            if validate_store(&path).is_err() {
                continue;
            }
            let slug = format!(
                "{}/{}",
                org.file_name().to_string_lossy(),
                repo.file_name().to_string_lossy()
            );
            entries.push(StoreEntry { slug, path });
        }
    }
    entries.sort_by(|a, b| a.slug.cmp(&b.slug));
    entries
}

/// Check that `path` looks like a himitsu store — it must exist, be a
/// directory, and contain a `.himitsu/` subdirectory (the same layout
/// [`crate::remote::store::secrets_dir`] expects).
pub fn validate_store(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    let himitsu = path.join(".himitsu");
    if !himitsu.is_dir() {
        return Err(format!(
            "missing .himitsu/ subdirectory: {}",
            path.display()
        ));
    }
    Ok(())
}

/// Expand a leading `~` or `~/` in a user-supplied path. Anything else is
/// returned verbatim as a `PathBuf`.
fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if input == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(input)
}

/// Compute a centred rectangle covering the given percentages of `r`.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vert[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_char(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Create a directory that looks like a valid himitsu store.
    fn make_store(root: &Path, slug: &str) -> PathBuf {
        let (org, repo) = slug.split_once('/').unwrap();
        let path = root.join(org).join(repo);
        std::fs::create_dir_all(path.join(".himitsu").join("secrets")).unwrap();
        path
    }

    #[test]
    fn enumerate_skips_non_store_dirs() {
        let tmp = tempdir().unwrap();
        make_store(tmp.path(), "acme/secrets");
        // A plain directory that is NOT a store must be skipped.
        std::fs::create_dir_all(tmp.path().join("garbage").join("nope")).unwrap();
        let entries = enumerate_stores(tmp.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].slug, "acme/secrets");
    }

    #[test]
    fn validate_store_accepts_valid_layout() {
        let tmp = tempdir().unwrap();
        let path = make_store(tmp.path(), "acme/secrets");
        assert!(validate_store(&path).is_ok());
    }

    #[test]
    fn validate_store_rejects_missing_path() {
        let err = validate_store(Path::new("/definitely/not/a/real/path/xyz")).unwrap_err();
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn validate_store_rejects_plain_directory() {
        let tmp = tempdir().unwrap();
        let err = validate_store(tmp.path()).unwrap_err();
        assert!(err.contains(".himitsu"));
    }

    #[test]
    fn picker_cancels_on_esc() {
        let tmp = tempdir().unwrap();
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        assert_eq!(
            picker.on_key(press(KeyCode::Esc)),
            StorePickerOutcome::Cancelled
        );
    }

    #[test]
    fn picker_selects_list_entry_on_enter() {
        let tmp = tempdir().unwrap();
        let store = make_store(tmp.path(), "acme/secrets");
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        match picker.on_key(press(KeyCode::Enter)) {
            StorePickerOutcome::Selected(p) => assert_eq!(p, store),
            other => panic!("expected Selected, got {other:?}"),
        }
    }

    #[test]
    fn picker_navigates_list_with_arrows() {
        let tmp = tempdir().unwrap();
        make_store(tmp.path(), "a/one");
        make_store(tmp.path(), "b/two");
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        assert_eq!(picker.list_state.selected(), Some(0));
        picker.on_key(press(KeyCode::Down));
        assert_eq!(picker.list_state.selected(), Some(1));
        picker.on_key(press(KeyCode::Down));
        // Wraps.
        assert_eq!(picker.list_state.selected(), Some(0));
        picker.on_key(press(KeyCode::Up));
        assert_eq!(picker.list_state.selected(), Some(1));
    }

    #[test]
    fn picker_toggles_focus_on_tab() {
        let tmp = tempdir().unwrap();
        make_store(tmp.path(), "acme/secrets");
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        assert_eq!(picker.focus, Focus::List);
        picker.on_key(press(KeyCode::Tab));
        assert_eq!(picker.focus, Focus::Input);
        picker.on_key(press(KeyCode::Tab));
        assert_eq!(picker.focus, Focus::List);
    }

    #[test]
    fn picker_accepts_typed_valid_path() {
        let tmp = tempdir().unwrap();
        let store = make_store(tmp.path(), "acme/secrets");
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        // Switch to input focus and type the path.
        picker.on_key(press(KeyCode::Tab));
        for c in store.to_string_lossy().chars() {
            picker.on_key(type_char(c));
        }
        assert_eq!(picker.input(), store.to_string_lossy().as_ref());
        match picker.on_key(press(KeyCode::Enter)) {
            StorePickerOutcome::Selected(p) => assert_eq!(p, store),
            other => panic!("expected Selected, got {other:?}"),
        }
    }

    #[test]
    fn picker_reports_error_for_invalid_typed_path() {
        let tmp = tempdir().unwrap();
        // Start with empty stores dir so picker begins on input focus
        // via the post-construction fixup.
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        // No entries → focus starts on Input.
        assert_eq!(picker.focus, Focus::Input);
        for c in "/nope/not/a/store".chars() {
            picker.on_key(type_char(c));
        }
        let outcome = picker.on_key(press(KeyCode::Enter));
        assert_eq!(outcome, StorePickerOutcome::Pending);
        assert!(picker.error().is_some());
    }

    #[test]
    fn picker_backspace_edits_input() {
        let tmp = tempdir().unwrap();
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        // No entries → starts on Input.
        assert_eq!(picker.focus, Focus::Input);
        picker.on_key(type_char('a'));
        picker.on_key(type_char('b'));
        picker.on_key(type_char('c'));
        assert_eq!(picker.input(), "abc");
        picker.on_key(press(KeyCode::Backspace));
        assert_eq!(picker.input(), "ab");
    }

    #[test]
    fn picker_empty_input_shows_error() {
        let tmp = tempdir().unwrap();
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        assert_eq!(picker.focus, Focus::Input);
        let outcome = picker.on_key(press(KeyCode::Enter));
        assert_eq!(outcome, StorePickerOutcome::Pending);
        assert_eq!(picker.error(), Some("path is empty"));
    }

    #[test]
    fn picker_error_clears_on_next_keypress() {
        let tmp = tempdir().unwrap();
        let mut picker = StorePicker::new(tmp.path(), PathBuf::new());
        picker.on_key(press(KeyCode::Enter)); // empty → error
        assert!(picker.error().is_some());
        picker.on_key(type_char('a'));
        assert!(picker.error().is_none());
    }
}
