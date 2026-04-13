//! New-secret form: in-TUI creation of a secret without shelling out.
//!
//! Three-step state machine:
//!
//! 1. **Env** — path segment (defaults to the dashboard's currently selected
//!    env, still editable).
//! 2. **Path** — secret key within the env (e.g. `API_KEY`). Combined with
//!    the env it forms `<env>/<path>`.
//! 3. **Value** — multi-line buffer. `Enter` inserts a newline, `Ctrl+S`
//!    submits the form.
//!
//! Submission encrypts via [`crate::crypto::age`] and writes through
//! [`crate::remote::store::write_secret`], reusing the exact same code path
//! that `himitsu set` uses. No subprocesses are spawned.
//!
//! On success the outer app router refreshes the dashboard; on failure the
//! view surfaces the error in its status line and stays open so the user
//! can correct the input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::cli::Context;
use crate::crypto::age;
use crate::remote::store;

/// Outcome of handling a key — routed by [`crate::tui::app::App`].
#[derive(Debug, Clone)]
pub enum NewSecretAction {
    None,
    /// User cancelled (Esc). Return to the dashboard without creating anything.
    Cancel,
    /// Ctrl-C quit.
    Quit,
    /// Secret was created successfully. Carries the full path (`env/key`)
    /// so the dashboard can select it after refresh.
    Created(String),
    /// Submission failed but the form should stay open so the user can
    /// edit. Carries the error message to show in the status line.
    Failed(String),
}

/// Which field currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Env,
    Path,
    Value,
}

pub struct NewSecretView {
    step: Step,
    env: String,
    path: String,
    value: String,
    status: Option<String>,
    ctx: Context,
}

impl NewSecretView {
    pub fn new(ctx: &Context, default_env: Option<String>) -> Self {
        Self {
            step: Step::Env,
            env: default_env.unwrap_or_default(),
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
    pub fn env(&self) -> &str {
        &self.env
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

        // Ctrl+S submits from any step (common pattern for multi-line forms).
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('s'), KeyModifiers::CONTROL)
        ) {
            return self.submit();
        }

        match self.step {
            Step::Env => self.handle_line_key(key, true),
            Step::Path => self.handle_line_key(key, false),
            Step::Value => self.handle_value_key(key),
        }
    }

    /// Single-line editor for `env` and `path`. `Enter` advances to the next
    /// step (rejecting empty input), `Backspace` erases, other chars append.
    /// Pressing Shift-Tab or Up returns to the previous step.
    fn handle_line_key(&mut self, key: KeyEvent, is_env_step: bool) -> NewSecretAction {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                if is_env_step {
                    if self.env.trim().is_empty() {
                        self.status = Some("env cannot be empty".into());
                        return NewSecretAction::None;
                    }
                    self.status = None;
                    self.step = Step::Path;
                } else {
                    if self.path.trim().is_empty() {
                        self.status = Some("path cannot be empty".into());
                        return NewSecretAction::None;
                    }
                    self.status = None;
                    self.step = Step::Value;
                }
                NewSecretAction::None
            }
            (KeyCode::Tab, _) => {
                // Tab always advances (same semantics as Enter).
                if is_env_step {
                    self.step = Step::Path;
                } else {
                    self.step = Step::Value;
                }
                NewSecretAction::None
            }
            (KeyCode::BackTab, _) | (KeyCode::Up, _) => {
                if !is_env_step {
                    self.step = Step::Env;
                }
                NewSecretAction::None
            }
            (KeyCode::Backspace, _) => {
                if is_env_step {
                    self.env.pop();
                } else {
                    self.path.pop();
                }
                NewSecretAction::None
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                if is_env_step {
                    self.env.push(c);
                } else {
                    self.path.push(c);
                }
                NewSecretAction::None
            }
            _ => NewSecretAction::None,
        }
    }

    /// Multi-line editor for `value`. `Enter` inserts a newline, `Backspace`
    /// deletes the previous char, `Ctrl+S` submits (handled in `on_key`
    /// before dispatch).
    fn handle_value_key(&mut self, key: KeyEvent) -> NewSecretAction {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                self.value.push('\n');
                NewSecretAction::None
            }
            (KeyCode::BackTab, _) => {
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
        if self.env.trim().is_empty() {
            self.status = Some("env cannot be empty".into());
            self.step = Step::Env;
            return NewSecretAction::None;
        }
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

        let full = format!("{}/{}", self.env.trim(), self.path.trim());

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

        let ciphertext = match age::encrypt(self.value.as_bytes(), &recipients) {
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

        // Best-effort git sync — mirrors the `himitsu set` behaviour.
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
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_env_field(frame, chunks[1]);
        self.draw_path_field(frame, chunks[2]);
        self.draw_value_field(frame, chunks[3]);
        self.draw_footer(frame, chunks[4]);
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

    fn draw_env_field(&self, frame: &mut Frame<'_>, area: Rect) {
        let focused = self.step == Step::Env;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" env ")
            .border_style(Self::border_style(focused));
        let mut text = self.env.clone();
        if focused {
            text.push('_');
        }
        frame.render_widget(Paragraph::new(text).block(block), area);
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
                Span::styled("ctrl-s", Style::default().fg(Color::Cyan)),
                Span::raw(" save  "),
                Span::styled("esc", Style::default().fg(Color::Cyan)),
                Span::raw(" cancel  "),
                Span::styled("ctrl-c", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
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
    fn new_view_defaults_to_env_step_and_prefills_env() {
        let view = NewSecretView::new(&empty_ctx(), Some("prod".into()));
        assert_eq!(view.step(), Step::Env);
        assert_eq!(view.env(), "prod");
        assert_eq!(view.path(), "");
        assert_eq!(view.value(), "");
    }

    #[test]
    fn enter_advances_from_env_to_path_to_value() {
        let mut view = NewSecretView::new(&empty_ctx(), Some("prod".into()));
        assert_eq!(view.step(), Step::Env);
        view.on_key(press(KeyCode::Enter));
        assert_eq!(view.step(), Step::Path);
        typ(&mut view, "API_KEY");
        view.on_key(press(KeyCode::Enter));
        assert_eq!(view.step(), Step::Value);
        assert_eq!(view.path(), "API_KEY");
    }

    #[test]
    fn enter_on_empty_env_is_rejected_with_status() {
        let mut view = NewSecretView::new(&empty_ctx(), None);
        view.on_key(press(KeyCode::Enter));
        assert_eq!(view.step(), Step::Env);
        assert!(view.status().unwrap().contains("env"));
    }

    #[test]
    fn enter_on_empty_path_is_rejected_with_status() {
        let mut view = NewSecretView::new(&empty_ctx(), Some("prod".into()));
        view.on_key(press(KeyCode::Enter));
        assert_eq!(view.step(), Step::Path);
        view.on_key(press(KeyCode::Enter));
        assert_eq!(view.step(), Step::Path);
        assert!(view.status().unwrap().contains("path"));
    }

    #[test]
    fn backspace_erases_last_char_in_current_field() {
        let mut view = NewSecretView::new(&empty_ctx(), None);
        typ(&mut view, "prod");
        view.on_key(press(KeyCode::Backspace));
        assert_eq!(view.env(), "pro");
    }

    #[test]
    fn value_step_treats_enter_as_newline() {
        let mut view = NewSecretView::new(&empty_ctx(), Some("prod".into()));
        view.on_key(press(KeyCode::Enter)); // env -> path
        typ(&mut view, "KEY");
        view.on_key(press(KeyCode::Enter)); // path -> value
        typ(&mut view, "line1");
        view.on_key(press(KeyCode::Enter));
        typ(&mut view, "line2");
        assert_eq!(view.value(), "line1\nline2");
        assert_eq!(view.step(), Step::Value);
    }

    #[test]
    fn shift_tab_goes_back_to_previous_step() {
        let mut view = NewSecretView::new(&empty_ctx(), Some("prod".into()));
        view.on_key(press(KeyCode::Enter)); // -> Path
        view.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(view.step(), Step::Env);
    }

    #[test]
    fn esc_cancels_the_form() {
        let mut view = NewSecretView::new(&empty_ctx(), Some("prod".into()));
        typ(&mut view, "x");
        assert!(matches!(
            view.on_key(press(KeyCode::Esc)),
            NewSecretAction::Cancel
        ));
    }

    #[test]
    fn ctrl_c_quits_from_any_step() {
        let mut view = NewSecretView::new(&empty_ctx(), Some("prod".into()));
        assert!(matches!(view.on_key(ctrl('c')), NewSecretAction::Quit));
    }

    #[test]
    fn submit_with_empty_value_fails_and_refocuses_value_step() {
        let mut view = NewSecretView::new(&empty_ctx(), Some("prod".into()));
        view.on_key(press(KeyCode::Enter));
        typ(&mut view, "KEY");
        view.on_key(press(KeyCode::Enter));
        // Value is empty — Ctrl+S should not submit.
        let out = view.on_key(ctrl('s'));
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Value);
        assert!(view.status().unwrap().contains("value"));
    }

    #[test]
    fn submit_with_missing_env_refocuses_env_step() {
        let mut view = NewSecretView::new(&empty_ctx(), None);
        // Jump forward with Tab even though env is empty — verifies submit()
        // drags focus back.
        view.on_key(press(KeyCode::Tab));
        view.on_key(press(KeyCode::Tab));
        let out = view.on_key(ctrl('s'));
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Env);
    }
}
