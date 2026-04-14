//! New-secret form: in-TUI creation of a secret without shelling out.
//!
//! Two-step state machine:
//!
//! 1. **Path** — full secret path (e.g. `prod/API_KEY`). Slashes are allowed
//!    and purely organisational — they show up as folder headers in the
//!    search view.
//! 2. **Value** — multi-line buffer. `Enter` inserts a newline; `Ctrl+S` or
//!    `Ctrl+W` submits the form from any step.
//!
//! Submission encrypts via [`crate::crypto::age`] and writes through
//! [`crate::remote::store::write_secret`], reusing the exact same code path
//! that `himitsu set` uses. No subprocesses are spawned.
//!
//! On success the outer app router refreshes search; on failure the view
//! surfaces the error in its status line and stays open so the user can
//! correct the input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::cli::Context;
use crate::crypto::{age, secret_value};
use crate::proto::SecretValue;
use crate::remote::store;

/// Outcome of handling a key — routed by [`crate::tui::app::App`].
#[derive(Debug, Clone)]
pub enum NewSecretAction {
    None,
    /// User cancelled (Esc). Return to search without creating anything.
    Cancel,
    /// Ctrl-C quit.
    Quit,
    /// Secret was created successfully. Carries the full path so the caller
    /// can refresh search and surface a confirmation.
    Created(String),
    /// Submission failed but the form should stay open so the user can
    /// edit. Carries the error message to show in the status line.
    Failed(String),
}

/// Which field currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Path,
    Value,
}

pub struct NewSecretView {
    step: Step,
    path: String,
    value: String,
    status: Option<String>,
    ctx: Context,
}

impl NewSecretView {
    pub fn new(ctx: &Context) -> Self {
        Self {
            step: Step::Path,
            path: String::new(),
            value: String::new(),
            status: None,
            ctx: ctx.clone(),
        }
    }

    #[cfg(test)]
    pub fn step(&self) -> Step {
        self.step
    }

    #[cfg(test)]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[cfg(test)]
    pub fn value(&self) -> &str {
        &self.value
    }

    #[cfg(test)]
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn on_key(&mut self, key: KeyEvent) -> NewSecretAction {
        // Global escape hatches.
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
        ) {
            return NewSecretAction::Quit;
        }
        if matches!(key.code, KeyCode::Esc) {
            return NewSecretAction::Cancel;
        }

        // Save from any step. Ctrl+W is the tmux-safe alternative to Ctrl+S,
        // which many users rebind as their tmux prefix.
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('s'), KeyModifiers::CONTROL)
                | (KeyCode::Char('w'), KeyModifiers::CONTROL)
        ) {
            return self.submit();
        }

        match self.step {
            Step::Path => self.handle_path_key(key),
            Step::Value => self.handle_value_key(key),
        }
    }

    /// Single-line editor for `path`. `Enter` / `Tab` advances to the value
    /// step (rejecting empty input), `Backspace` erases, other chars append.
    fn handle_path_key(&mut self, key: KeyEvent) -> NewSecretAction {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) | (KeyCode::Tab, _) => {
                if self.path.trim().is_empty() {
                    self.status = Some("path cannot be empty".into());
                    return NewSecretAction::None;
                }
                self.status = None;
                self.step = Step::Value;
                NewSecretAction::None
            }
            (KeyCode::Backspace, _) => {
                self.path.pop();
                NewSecretAction::None
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.path.push(c);
                NewSecretAction::None
            }
            _ => NewSecretAction::None,
        }
    }

    /// Multi-line editor for `value`. `Enter` inserts a newline; `Shift-Tab`
    /// returns to the path step; `Ctrl+S` / `Ctrl+W` submits (handled in
    /// `on_key` before dispatch).
    fn handle_value_key(&mut self, key: KeyEvent) -> NewSecretAction {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                self.value.push('\n');
                NewSecretAction::None
            }
            (KeyCode::BackTab, _) | (KeyCode::Up, _) => {
                self.step = Step::Path;
                NewSecretAction::None
            }
            (KeyCode::Backspace, _) => {
                self.value.pop();
                NewSecretAction::None
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.value.push(c);
                NewSecretAction::None
            }
            _ => NewSecretAction::None,
        }
    }

    /// Validate and persist the secret. On success returns `Created(..)`;
    /// on failure leaves the form untouched and returns `Failed(..)`.
    fn submit(&mut self) -> NewSecretAction {
        if self.path.trim().is_empty() {
            self.status = Some("path cannot be empty".into());
            self.step = Step::Path;
            return NewSecretAction::None;
        }
        if self.value.is_empty() {
            self.status = Some("value cannot be empty".into());
            self.step = Step::Value;
            return NewSecretAction::None;
        }

        let full = self.path.trim().to_string();

        let recipients = match age::collect_recipients(
            &self.ctx.store,
            self.ctx.recipients_path.as_deref(),
        ) {
            Ok(r) if !r.is_empty() => r,
            Ok(_) => {
                let msg = "no recipients configured for this store".to_string();
                self.status = Some(msg.clone());
                return NewSecretAction::Failed(msg);
            }
            Err(e) => {
                let msg = format!("{e}");
                self.status = Some(msg.clone());
                return NewSecretAction::Failed(msg);
            }
        };

        let sv = SecretValue {
            data: self.value.as_bytes().to_vec(),
            content_type: String::new(),
            annotations: Default::default(),
            totp: String::new(),
            url: String::new(),
            expires_at: None,
            description: String::new(),
            env_key: String::new(),
        };
        let wire = secret_value::encode(&sv);
        let ciphertext = match age::encrypt(&wire, &recipients) {
            Ok(ct) => ct,
            Err(e) => {
                let msg = format!("{e}");
                self.status = Some(msg.clone());
                return NewSecretAction::Failed(msg);
            }
        };

        if let Err(e) = store::write_secret(&self.ctx.store, &full, &ciphertext) {
            let msg = format!("{e}");
            self.status = Some(msg.clone());
            return NewSecretAction::Failed(msg);
        }

        self.ctx.commit_and_push(&format!("himitsu: set {full}"));

        NewSecretAction::Created(full)
    }

    // ── Drawing ────────────────────────────────────────────────────────

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_path_field(frame, chunks[1]);
        self.draw_value_field(frame, chunks[2]);
        self.draw_footer(frame, chunks[3]);
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
            Span::styled("new secret", Style::default().add_modifier(Modifier::BOLD)),
        ]);
        frame.render_widget(Paragraph::new(header), area);
    }

    fn draw_path_field(&self, frame: &mut Frame<'_>, area: Rect) {
        let focused = self.step == Step::Path;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" path ")
            .border_style(Self::border_style(focused));
        let mut text = self.path.clone();
        if focused {
            text.push('_');
        }
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_value_field(&self, frame: &mut Frame<'_>, area: Rect) {
        let focused = self.step == Step::Value;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" value ")
            .border_style(Self::border_style(focused));
        let mut text = self.value.clone();
        if focused {
            text.push('_');
        }
        frame.render_widget(
            Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
            area,
        );
    }

    fn border_style(focused: bool) -> Style {
        if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        }
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = if let Some(msg) = &self.status {
            Line::from(Span::styled(
                msg.clone(),
                Style::default().fg(Color::Red),
            ))
        } else {
            Line::from(vec![
                Span::styled("enter", Style::default().fg(Color::Cyan)),
                Span::raw(" next  "),
                Span::styled("shift-tab", Style::default().fg(Color::Cyan)),
                Span::raw(" prev  "),
                Span::styled("ctrl-s / ctrl-w", Style::default().fg(Color::Cyan)),
                Span::raw(" save  "),
                Span::styled("esc", Style::default().fg(Color::Cyan)),
                Span::raw(" cancel  "),
                Span::styled("ctrl-c", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }

    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("tab / enter", "next field"),
            ("shift-tab / ↑", "previous field"),
            ("ctrl-s / ctrl-w", "save from any step"),
            ("esc / ctrl-c", "cancel"),
            ("?", "toggle this help"),
        ]
    }

    pub fn help_title() -> &'static str {
        "new secret · keys"
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn empty_ctx() -> Context {
        Context {
            data_dir: PathBuf::new(),
            state_dir: PathBuf::new(),
            store: PathBuf::new(),
            recipients_path: None,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typ(view: &mut NewSecretView, s: &str) {
        for c in s.chars() {
            view.on_key(press(KeyCode::Char(c)));
        }
    }

    #[test]
    fn new_view_starts_on_path_step_with_empty_fields() {
        let view = NewSecretView::new(&empty_ctx());
        assert_eq!(view.step(), Step::Path);
        assert_eq!(view.path(), "");
        assert_eq!(view.value(), "");
    }

    #[test]
    fn enter_advances_from_path_to_value() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/API_KEY");
        view.on_key(press(KeyCode::Enter));
        assert_eq!(view.step(), Step::Value);
        assert_eq!(view.path(), "prod/API_KEY");
    }

    #[test]
    fn enter_on_empty_path_is_rejected_with_status() {
        let mut view = NewSecretView::new(&empty_ctx());
        view.on_key(press(KeyCode::Enter));
        assert_eq!(view.step(), Step::Path);
        assert!(view.status().unwrap().contains("path"));
    }

    #[test]
    fn path_accepts_slashes_as_plain_characters() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "some/nested/path");
        assert_eq!(view.path(), "some/nested/path");
    }

    #[test]
    fn backspace_erases_last_char_in_current_field() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod");
        view.on_key(press(KeyCode::Backspace));
        assert_eq!(view.path(), "pro");
    }

    #[test]
    fn value_step_treats_enter_as_newline() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Enter)); // path -> value
        typ(&mut view, "line1");
        view.on_key(press(KeyCode::Enter));
        typ(&mut view, "line2");
        assert_eq!(view.value(), "line1\nline2");
        assert_eq!(view.step(), Step::Value);
    }

    #[test]
    fn shift_tab_goes_back_to_previous_step() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Enter));
        assert_eq!(view.step(), Step::Value);
        view.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(view.step(), Step::Path);
    }

    #[test]
    fn esc_cancels_the_form() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "x");
        assert!(matches!(
            view.on_key(press(KeyCode::Esc)),
            NewSecretAction::Cancel
        ));
    }

    #[test]
    fn ctrl_c_quits_from_any_step() {
        let mut view = NewSecretView::new(&empty_ctx());
        assert!(matches!(view.on_key(ctrl('c')), NewSecretAction::Quit));
    }

    #[test]
    fn submit_with_empty_value_fails_and_refocuses_value_step() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Enter));
        let out = view.on_key(ctrl('s'));
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Value);
        assert!(view.status().unwrap().contains("value"));
    }

    #[test]
    fn submit_with_missing_path_refocuses_path_step() {
        let mut view = NewSecretView::new(&empty_ctx());
        // Jump forward with Tab even though path is empty — submit() should
        // drag focus back to Path.
        view.on_key(press(KeyCode::Tab));
        let out = view.on_key(ctrl('s'));
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Path);
    }

    #[test]
    fn ctrl_w_submits_like_ctrl_s() {
        // hm-y6n: tmux-safe alternative to ctrl+s.
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Enter));
        let out = view.on_key(ctrl('w'));
        // Empty value rejects — confirms the chord reached submit().
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Value);
        assert!(view.status().unwrap().contains("value"));
    }
}
