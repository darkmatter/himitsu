//! Command palette overlay: a centered, fuzzy-filterable list of actions
//! reachable from the current view.
//!
//! Inspired by the VS Code command palette and `fzf`. Opened with `Ctrl+P`
//! from the search view, it consolidates discoverability of every command
//! the app exposes so individual footers don't need to enumerate them.
//!
//! The palette is intentionally dumb: it owns no state about the views it
//! launches. Each entry resolves to a [`Command`] variant, and the host
//! view (currently `SearchView`) maps the variant onto its existing key
//! handlers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::tui::theme;

/// One command exposed in the palette. Mirrors the visible top-level
/// stateless CLI surface so Ctrl+P stays at parity with `himitsu --help`.
/// Listed in [`COMMANDS`] roughly by usage frequency.
///
/// Commands without a full TUI affordance yet dispatch to a hint toast that
/// names the equivalent CLI invocation — discoverability now, full forms as
/// follow-up work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    // ── Wired in the TUI ─────────────────────────────────────────────
    NewSecret,
    Sync,
    Rekey,
    Join,
    AddRemote,
    SwitchStore,
    ToggleStoreColumn,
    Envs,
    Help,
    Quit,

    // ── CLI parity (hint to CLI invocation) ──────────────────────────
    RecipientLs,
    RecipientAdd,
    RecipientRm,
    RecipientShow,
    RemoteList,
    RemoteRemove,
    RemoteSetDefault,
    ContextShow,
    ContextSet,
    ContextClear,
    Generate,
    Export,
    Check,
    Docs,
    Import,
    Git,
}

impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Command::NewSecret => "new secret",
            Command::Sync => "sync",
            Command::Rekey => "rekey",
            Command::Join => "join store",
            Command::AddRemote => "add remote",
            Command::SwitchStore => "switch store",
            Command::ToggleStoreColumn => "toggle store column",
            Command::Envs => "browse envs",
            Command::Help => "show help",
            Command::Quit => "quit",

            Command::RecipientLs => "list recipients",
            Command::RecipientAdd => "add recipient",
            Command::RecipientRm => "remove recipient",
            Command::RecipientShow => "show recipient",
            Command::RemoteList => "list remotes",
            Command::RemoteRemove => "remove remote",
            Command::RemoteSetDefault => "set default store",
            Command::ContextShow => "show context",
            Command::ContextSet => "set context",
            Command::ContextClear => "clear context",
            Command::Generate => "generate configs",
            Command::Export => "export to SOPS",
            Command::Check => "check stores",
            Command::Docs => "show docs",
            Command::Import => "import secrets",
            Command::Git => "run git",
        }
    }

    pub fn shortcut(&self) -> &'static str {
        match self {
            Command::NewSecret => "ctrl-n",
            Command::SwitchStore => "ctrl-s",
            Command::Envs => "shift-e",
            Command::Help => "?",
            Command::Quit => "esc",
            _ => "",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Command::NewSecret => "Create a new encrypted secret",
            Command::Sync => "Pull from remote and rekey drifted secrets",
            Command::Rekey => "Re-encrypt all secrets for current recipients",
            Command::Join => "Add your age key to this store's recipients",
            Command::AddRemote => "Clone and register a remote git store",
            Command::SwitchStore => "Pick a different remote / checkout",
            Command::ToggleStoreColumn => "Show/hide the STORE column in the results table",
            Command::Envs => "Browse env presets defined in himitsu.yaml",
            Command::Help => "Open the contextual key reference",
            Command::Quit => "Exit the TUI",

            Command::RecipientLs => "List recipients for the current store",
            Command::RecipientAdd => "Register a new recipient (needs name + age key)",
            Command::RecipientRm => "Remove a recipient by name",
            Command::RecipientShow => "Print a recipient's key and description",
            Command::RemoteList => "List all configured stores",
            Command::RemoteRemove => "Remove a store checkout",
            Command::RemoteSetDefault => "Choose the default store for unqualified paths",
            Command::ContextShow => "Show the active store context",
            Command::ContextSet => "Pin a store as the active context",
            Command::ContextClear => "Clear the active context",
            Command::Generate => "Emit config files from himitsu.yaml env presets",
            Command::Export => "Export secrets matching a glob to a SOPS file",
            Command::Check => "Verify checkouts are up to date with remotes",
            Command::Docs => "Render the himitsu README",
            Command::Import => "Import secrets from 1Password or a SOPS file",
            Command::Git => "Run git inside a store checkout",
        }
    }

    /// Equivalent CLI invocation, used by the host view to surface a hint
    /// toast for commands that don't have a full TUI form yet. Returns
    /// `None` for commands that are already wired into the TUI.
    pub fn cli_hint(&self) -> Option<&'static str> {
        match self {
            Command::NewSecret
            | Command::Sync
            | Command::Rekey
            | Command::Join
            | Command::AddRemote
            | Command::SwitchStore
            | Command::ToggleStoreColumn
            | Command::Envs
            | Command::Help
            | Command::Quit => None,

            Command::RecipientLs => Some("himitsu recipient ls"),
            Command::RecipientAdd => Some("himitsu recipient add <name> --age-key <key>"),
            Command::RecipientRm => Some("himitsu recipient rm <name>"),
            Command::RecipientShow => Some("himitsu recipient show <name>"),
            Command::RemoteList => Some("himitsu remote list"),
            Command::RemoteRemove => Some("himitsu remote remove <slug>"),
            Command::RemoteSetDefault => Some("himitsu remote default <slug>"),
            Command::ContextShow => Some("himitsu context"),
            Command::ContextSet => Some("himitsu context remote <slug>"),
            Command::ContextClear => Some("himitsu context clear"),
            Command::Generate => Some("himitsu generate"),
            Command::Export => Some("himitsu export <glob> --to <file>"),
            Command::Check => Some("himitsu check"),
            Command::Docs => Some("himitsu docs"),
            Command::Import => Some("himitsu import --op <ref> <path>"),
            Command::Git => Some("himitsu git -- <args>"),
        }
    }
}

/// Outcome of a key press while the palette is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPaletteOutcome {
    /// Stay open; redraw with updated state.
    Pending,
    /// User pressed Esc / Ctrl+C without picking anything.
    Cancelled,
    /// User picked `command`.
    Selected(Command),
}

/// Palette state — the filter buffer and the selection cursor over the
/// filtered subset of [`COMMANDS`].
pub struct CommandPalette {
    query: String,
    filtered: Vec<Command>,
    list_state: ListState,
}

const COMMANDS: &[Command] = &[
    // Wired commands first — these are what most users reach for.
    Command::NewSecret,
    Command::Sync,
    Command::Rekey,
    Command::Join,
    Command::AddRemote,
    Command::SwitchStore,
    Command::ToggleStoreColumn,
    Command::Envs,
    // CLI-parity commands — discoverable here, but selecting them surfaces
    // the equivalent CLI invocation rather than running an in-TUI form.
    Command::RecipientLs,
    Command::RecipientAdd,
    Command::RecipientRm,
    Command::RecipientShow,
    Command::RemoteList,
    Command::RemoteRemove,
    Command::RemoteSetDefault,
    Command::ContextShow,
    Command::ContextSet,
    Command::ContextClear,
    Command::Generate,
    Command::Export,
    Command::Check,
    Command::Import,
    Command::Git,
    Command::Docs,
    // Help and Quit at the end so they aren't where the cursor lands by
    // default but stay one search-keystroke away.
    Command::Help,
    Command::Quit,
];

impl CommandPalette {
    pub fn new() -> Self {
        let mut palette = Self {
            query: String::new(),
            filtered: COMMANDS.to_vec(),
            list_state: ListState::default(),
        };
        palette.list_state.select(Some(0));
        palette
    }

    pub fn on_key(&mut self, key: KeyEvent) -> CommandPaletteOutcome {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                CommandPaletteOutcome::Cancelled
            }
            (KeyCode::Enter, _) => match self.selected_command() {
                Some(cmd) => CommandPaletteOutcome::Selected(cmd),
                None => CommandPaletteOutcome::Pending,
            },
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.move_selection(-1);
                CommandPaletteOutcome::Pending
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.move_selection(1);
                CommandPaletteOutcome::Pending
            }
            (KeyCode::Backspace, _) => {
                self.query.pop();
                self.refilter();
                CommandPaletteOutcome::Pending
            }
            (KeyCode::Char(c), modifiers)
                if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(c);
                self.refilter();
                CommandPaletteOutcome::Pending
            }
            _ => CommandPaletteOutcome::Pending,
        }
    }

    fn selected_command(&self) -> Option<Command> {
        self.list_state
            .selected()
            .and_then(|idx| self.filtered.get(idx).copied())
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.list_state.select(None);
            return;
        }
        let len = self.filtered.len() as isize;
        let current = self.list_state.selected().unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len) as usize;
        self.list_state.select(Some(next));
    }

    /// Recompute [`Self::filtered`] for the current `query`. Uses a simple
    /// case-insensitive substring match across `label` + `description` —
    /// good enough for ~10 commands; revisit if the catalog grows large
    /// enough that a real fuzzy matcher matters.
    fn refilter(&mut self) {
        let q = self.query.to_ascii_lowercase();
        self.filtered = COMMANDS
            .iter()
            .copied()
            .filter(|cmd| {
                if q.is_empty() {
                    return true;
                }
                cmd.label().to_ascii_lowercase().contains(&q)
                    || cmd.description().to_ascii_lowercase().contains(&q)
                    || cmd.shortcut().to_ascii_lowercase().contains(&q)
            })
            .collect();
        if self.filtered.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = centered_rect(60, 50, frame.area());
        frame.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::border()))
            .title(Line::from(theme::brand_chip("command palette")));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // prompt
                Constraint::Length(1), // separator
                Constraint::Min(1),    // results
                Constraint::Length(1), // footer
            ])
            .split(inner);

        // Prompt row: > <query>█
        let prompt = Line::from(vec![
            Span::styled(" > ", Style::default().fg(theme::accent())),
            Span::raw(self.query.clone()),
            Span::styled("█", Style::default().fg(theme::accent())),
        ]);
        frame.render_widget(Paragraph::new(prompt), rows[0]);

        // Separator
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(theme::border()),
            ))),
            rows[1],
        );

        // Results
        if self.filtered.is_empty() {
            let msg = Paragraph::new(Line::from(Span::styled(
                "  no matching commands",
                Style::default().fg(theme::muted()),
            )));
            frame.render_widget(msg, rows[2]);
        } else {
            // Pad shortcuts to the widest entry so the descriptions line up.
            let shortcut_w = self
                .filtered
                .iter()
                .map(|c| c.shortcut().len())
                .max()
                .unwrap_or(0);
            let label_w = self
                .filtered
                .iter()
                .map(|c| c.label().len())
                .max()
                .unwrap_or(0);

            let items: Vec<ListItem> = self
                .filtered
                .iter()
                .map(|cmd| {
                    let line = Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            format!("{:<shortcut_w$}", cmd.shortcut()),
                            Style::default().fg(theme::accent()),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            format!("{:<label_w$}", cmd.label()),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(cmd.description(), Style::default().fg(theme::muted())),
                    ]);
                    ListItem::new(line)
                })
                .collect();
            let list = List::new(items).highlight_style(
                Style::default()
                    .bg(theme::accent())
                    .fg(theme::on_accent())
                    .add_modifier(Modifier::BOLD),
            );
            frame.render_stateful_widget(list, rows[2], &mut self.list_state);
        }

        // Footer
        let footer = Style::default().fg(theme::footer_text());
        let key = Style::default().fg(theme::accent());
        let line = Line::from(vec![
            Span::styled("↑/↓", key),
            Span::styled(" navigate    ", footer),
            Span::styled("enter", key),
            Span::styled(" run    ", footer),
            Span::styled("esc", key),
            Span::styled(" close", footer),
        ]);
        frame.render_widget(Paragraph::new(line), rows[3]);
    }
}

/// Build a centered rectangle within `area`.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
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
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn esc_cancels() {
        let mut p = CommandPalette::new();
        assert_eq!(
            p.on_key(press(KeyCode::Esc)),
            CommandPaletteOutcome::Cancelled
        );
    }

    #[test]
    fn enter_returns_selected_command() {
        let mut p = CommandPalette::new();
        // Default selection is the first command.
        assert_eq!(
            p.on_key(press(KeyCode::Enter)),
            CommandPaletteOutcome::Selected(Command::NewSecret),
        );
    }

    #[test]
    fn typing_filters_the_list() {
        let mut p = CommandPalette::new();
        // "browse" only appears in Envs (label "browse envs"); narrower
        // queries like "env" now also match Generate's description.
        for ch in "browse".chars() {
            p.on_key(press(KeyCode::Char(ch)));
        }
        assert_eq!(p.filtered, vec![Command::Envs]);
        assert_eq!(
            p.on_key(press(KeyCode::Enter)),
            CommandPaletteOutcome::Selected(Command::Envs),
        );
    }

    #[test]
    fn down_arrow_advances_selection() {
        let mut p = CommandPalette::new();
        p.on_key(press(KeyCode::Down));
        // Second command in the list (after NewSecret).
        assert_eq!(
            p.on_key(press(KeyCode::Enter)),
            CommandPaletteOutcome::Selected(Command::Sync),
        );
    }

    #[test]
    fn add_remote_filters_by_keyword() {
        let mut p = CommandPalette::new();
        for ch in "remote".chars() {
            p.on_key(press(KeyCode::Char(ch)));
        }
        assert!(p.filtered.contains(&Command::AddRemote));
        // "remote" also matches "add remote" — just confirm it's in the filtered set.
        assert!(p.filtered.contains(&Command::AddRemote));
    }

    #[test]
    fn cli_hint_set_matches_unwired_commands() {
        // Every command in the catalog either dispatches to a wired TUI
        // action (cli_hint == None) or surfaces a CLI invocation. Catch
        // accidental drift if a new variant gets added without either.
        for cmd in COMMANDS {
            let hint = cmd.cli_hint();
            match cmd {
                Command::NewSecret
                | Command::Sync
                | Command::Rekey
                | Command::Join
                | Command::AddRemote
                | Command::SwitchStore
                | Command::ToggleStoreColumn
                | Command::Envs
                | Command::Help
                | Command::Quit => assert!(hint.is_none(), "{:?} should be wired", cmd),
                _ => {
                    let h = hint.expect("CLI-parity command must have a hint");
                    assert!(
                        h.starts_with("himitsu "),
                        "{:?} hint should start with `himitsu `: {}",
                        cmd,
                        h
                    );
                }
            }
        }
    }

    #[test]
    fn empty_filter_disables_enter() {
        let mut p = CommandPalette::new();
        for ch in "zzz".chars() {
            p.on_key(press(KeyCode::Char(ch)));
        }
        assert!(p.filtered.is_empty());
        assert_eq!(
            p.on_key(press(KeyCode::Enter)),
            CommandPaletteOutcome::Pending,
        );
    }
}
