//! New-secret form: in-TUI creation of a secret without shelling out.
//!
//! The form walks through a fixed sequence of fields:
//!
//! 1. **Path** — full secret path (e.g. `prod/API_KEY`). Slashes are allowed
//!    and purely organisational; they show up as folder headers in the
//!    search view.
//! 2. **Value** — multi-line buffer. `Enter` inserts a newline.
//! 3. **Description** — human-readable note.
//! 4. **URL** — associated website or API.
//! 5. **TOTP** — `otpauth://` URI or base32 secret (validated).
//! 6. **Env key** — default env-var name (validated).
//! 7. **Expires at** — `never`, relative duration (`30d`/`6mo`/`1y`), or an
//!    RFC 3339 timestamp.
//!
//! Tab / Shift-Tab move between fields with wrap-around. `Ctrl+S` or `Ctrl+W`
//! submits from any field. Submission encrypts via [`crate::crypto::age`]
//! and writes through [`crate::remote::store::write_secret`], reusing the
//! exact same code path that `himitsu set` uses. Validation leans on the
//! `pub(crate)` helpers in [`crate::cli::set`] and [`crate::cli::duration`]
//! so the TUI and CLI stay in lockstep.
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

use crate::cli::duration::{self, ExpiresAt};
use crate::cli::set::{validate_env_key, validate_totp};
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
    Description,
    Url,
    Totp,
    EnvKey,
    ExpiresAt,
}

impl Step {
    /// All steps in display/tab order. Used for Tab / Shift-Tab cycling
    /// and for tests asserting the cycle visits every field.
    const ORDER: [Step; 7] = [
        Step::Path,
        Step::Value,
        Step::Description,
        Step::Url,
        Step::Totp,
        Step::EnvKey,
        Step::ExpiresAt,
    ];

    fn index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|s| *s == self)
            .expect("step is always in ORDER")
    }

    fn next(self) -> Step {
        let i = self.index();
        Self::ORDER[(i + 1) % Self::ORDER.len()]
    }

    fn prev(self) -> Step {
        let i = self.index();
        Self::ORDER[(i + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

pub struct NewSecretView {
    step: Step,
    path: String,
    value: String,
    description: String,
    url: String,
    totp: String,
    env_key: String,
    expires_at: String,
    status: Option<String>,
    ctx: Context,
}

impl NewSecretView {
    pub fn new(ctx: &Context) -> Self {
        Self {
            step: Step::Path,
            path: String::new(),
            value: String::new(),
            description: String::new(),
            url: String::new(),
            totp: String::new(),
            env_key: String::new(),
            expires_at: String::new(),
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

    /// Mutable accessor to the buffer that backs the currently focused step.
    /// `Value` is multi-line so it lives in its own helper; every other field
    /// routes through this single-line path.
    fn field_buffer_mut(&mut self, step: Step) -> Option<&mut String> {
        match step {
            Step::Path => Some(&mut self.path),
            Step::Value => None,
            Step::Description => Some(&mut self.description),
            Step::Url => Some(&mut self.url),
            Step::Totp => Some(&mut self.totp),
            Step::EnvKey => Some(&mut self.env_key),
            Step::ExpiresAt => Some(&mut self.expires_at),
        }
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

        // Shift-Tab wraps backward from any step.
        if matches!(key.code, KeyCode::BackTab) {
            self.move_to(self.step.prev());
            return NewSecretAction::None;
        }

        match self.step {
            Step::Value => self.handle_value_key(key),
            _ => self.handle_single_line_key(key),
        }
    }

    /// Single-line editor used by every field except `Value`. `Tab` / `Enter`
    /// advances to the next field (running field-local validation first);
    /// `Backspace` erases; printable chars append.
    fn handle_single_line_key(&mut self, key: KeyEvent) -> NewSecretAction {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) | (KeyCode::Tab, _) => {
                if let Err(msg) = self.validate_current_field() {
                    self.status = Some(msg);
                    return NewSecretAction::None;
                }
                self.status = None;
                self.move_to(self.step.next());
                NewSecretAction::None
            }
            (KeyCode::Up, _) => {
                if let Err(msg) = self.validate_current_field() {
                    self.status = Some(msg);
                    return NewSecretAction::None;
                }
                self.status = None;
                self.move_to(self.step.prev());
                NewSecretAction::None
            }
            (KeyCode::Down, _) => {
                if let Err(msg) = self.validate_current_field() {
                    self.status = Some(msg);
                    return NewSecretAction::None;
                }
                self.status = None;
                self.move_to(self.step.next());
                NewSecretAction::None
            }
            (KeyCode::Backspace, _) => {
                if let Some(buf) = self.field_buffer_mut(self.step) {
                    buf.pop();
                }
                NewSecretAction::None
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                if let Some(buf) = self.field_buffer_mut(self.step) {
                    buf.push(c);
                }
                NewSecretAction::None
            }
            _ => NewSecretAction::None,
        }
    }

    /// Multi-line editor for `Value`. `Enter` inserts a newline; `Tab` moves
    /// to the next field; `Backspace` erases; `Ctrl+S` / `Ctrl+W` submit
    /// (handled in `on_key` before dispatch).
    fn handle_value_key(&mut self, key: KeyEvent) -> NewSecretAction {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                self.value.push('\n');
                NewSecretAction::None
            }
            (KeyCode::Tab, _) => {
                self.move_to(self.step.next());
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

    fn move_to(&mut self, step: Step) {
        self.step = step;
    }

    /// Validate the field the user is about to leave. Empty optional fields
    /// are fine — we only complain about required inputs (path) or
    /// syntactically bad values.
    fn validate_current_field(&self) -> Result<(), String> {
        match self.step {
            Step::Path => {
                if self.path.trim().is_empty() {
                    return Err("path cannot be empty".into());
                }
                Ok(())
            }
            Step::Value => Ok(()),
            Step::Description | Step::Url => Ok(()),
            Step::Totp => {
                if self.totp.trim().is_empty() {
                    return Ok(());
                }
                validate_totp(&self.totp).map_err(|e| format!("{e}"))
            }
            Step::EnvKey => {
                if self.env_key.trim().is_empty() {
                    return Ok(());
                }
                validate_env_key(&self.env_key).map_err(|e| format!("{e}"))
            }
            Step::ExpiresAt => {
                if self.expires_at.trim().is_empty() {
                    return Ok(());
                }
                duration::parse(&self.expires_at)
                    .map(|_| ())
                    .map_err(|e| format!("{e}"))
            }
        }
    }

    /// Build a `SecretValue` populated with every entered field. Mirrors
    /// `cli::set::run`: empty optional fields become empty strings.
    fn build_secret_value(&self) -> Result<SecretValue, String> {
        let expires_at_ts = if self.expires_at.trim().is_empty() {
            None
        } else {
            match duration::parse(&self.expires_at).map_err(|e| format!("{e}"))? {
                ExpiresAt::Never => None,
                ExpiresAt::At(dt) => Some(duration::to_proto_timestamp(dt)),
            }
        };

        Ok(SecretValue {
            data: self.value.as_bytes().to_vec(),
            content_type: String::new(),
            annotations: Default::default(),
            totp: self.totp.clone(),
            url: self.url.clone(),
            expires_at: expires_at_ts,
            description: self.description.clone(),
            env_key: self.env_key.clone(),
        })
    }

    /// Run every field validator before we attempt to encrypt, pulling focus
    /// back to the offending field if something is wrong. Reuses the same
    /// `validate_current_field` path so save-time and leave-time checks stay
    /// in sync.
    fn validate_all(&mut self) -> Result<(), NewSecretAction> {
        if self.path.trim().is_empty() {
            self.status = Some("path cannot be empty".into());
            self.step = Step::Path;
            return Err(NewSecretAction::None);
        }
        if self.value.is_empty() {
            self.status = Some("value cannot be empty".into());
            self.step = Step::Value;
            return Err(NewSecretAction::None);
        }

        for step in [Step::Totp, Step::EnvKey, Step::ExpiresAt] {
            let saved = self.step;
            self.step = step;
            let check = self.validate_current_field();
            self.step = saved;
            if let Err(msg) = check {
                self.status = Some(msg);
                self.step = step;
                return Err(NewSecretAction::None);
            }
        }

        Ok(())
    }

    /// Validate every field, encrypt, and persist. On success returns
    /// `Created(..)`; on failure leaves the form untouched and returns
    /// either `None` (validation, so the user keeps editing) or
    /// `Failed(..)` (underlying crypto/store error).
    fn submit(&mut self) -> NewSecretAction {
        if let Err(action) = self.validate_all() {
            return action;
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

        let sv = match self.build_secret_value() {
            Ok(sv) => sv,
            Err(msg) => {
                self.status = Some(msg.clone());
                return NewSecretAction::Failed(msg);
            }
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
                Constraint::Length(1), // header
                Constraint::Length(3), // path
                Constraint::Min(3),    // value
                Constraint::Length(3), // description
                Constraint::Length(3), // url
                Constraint::Length(3), // totp
                Constraint::Length(3), // env_key
                Constraint::Length(3), // expires_at
                Constraint::Length(1), // footer
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_single_line(frame, chunks[1], Step::Path, " path ", &self.path);
        self.draw_value_field(frame, chunks[2]);
        self.draw_single_line(
            frame,
            chunks[3],
            Step::Description,
            " description ",
            &self.description,
        );
        self.draw_single_line(frame, chunks[4], Step::Url, " url ", &self.url);
        self.draw_single_line(frame, chunks[5], Step::Totp, " totp ", &self.totp);
        self.draw_single_line(frame, chunks[6], Step::EnvKey, " env_key ", &self.env_key);
        self.draw_single_line(
            frame,
            chunks[7],
            Step::ExpiresAt,
            " expires_at ",
            &self.expires_at,
        );
        self.draw_footer(frame, chunks[8]);
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

    fn draw_single_line(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        step: Step,
        title: &str,
        content: &str,
    ) {
        let focused = self.step == step;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title.to_string())
            .border_style(Self::border_style(focused));
        let mut text = content.to_string();
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
                Span::styled("tab", Style::default().fg(Color::Cyan)),
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
            ("tab / enter", "next field (wraps)"),
            ("shift-tab", "previous field (wraps)"),
            ("enter (value)", "insert newline"),
            ("ctrl-s / ctrl-w", "save from any field"),
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

    fn back_tab() -> KeyEvent {
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
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

    #[test]
    fn tab_cycle_visits_every_field_and_wraps_to_path() {
        // hm-r4i: cycling forward must hit every metadata field and wrap.
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        // Value is multi-line, so skip past it explicitly.
        let expected = [
            Step::Path,
            Step::Value,
            Step::Description,
            Step::Url,
            Step::Totp,
            Step::EnvKey,
            Step::ExpiresAt,
            Step::Path, // wrap-around
        ];
        let mut seen = vec![view.step()];
        for _ in 0..expected.len() - 1 {
            view.on_key(press(KeyCode::Tab));
            seen.push(view.step());
        }
        assert_eq!(seen, expected);
    }

    #[test]
    fn shift_tab_wraps_backward_from_path_to_expires_at() {
        let mut view = NewSecretView::new(&empty_ctx());
        assert_eq!(view.step(), Step::Path);
        view.on_key(back_tab());
        assert_eq!(view.step(), Step::ExpiresAt);
        view.on_key(back_tab());
        assert_eq!(view.step(), Step::EnvKey);
    }

    #[test]
    fn full_metadata_roundtrip_populates_secret_value() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/API_KEY");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "hunter2");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "the prod api key");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "https://api.example.com");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "API_KEY");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "30d");

        let sv = view.build_secret_value().expect("valid build");
        assert_eq!(sv.data, b"hunter2");
        assert_eq!(sv.description, "the prod api key");
        assert_eq!(sv.url, "https://api.example.com");
        assert_eq!(sv.totp, "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP");
        assert_eq!(sv.env_key, "API_KEY");
        assert!(sv.expires_at.is_some());
    }

    #[test]
    fn empty_optional_fields_stay_empty_in_secret_value() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "value");

        let sv = view.build_secret_value().expect("valid build");
        assert_eq!(sv.description, "");
        assert_eq!(sv.url, "");
        assert_eq!(sv.totp, "");
        assert_eq!(sv.env_key, "");
        assert!(sv.expires_at.is_none());
    }

    #[test]
    fn invalid_totp_blocks_submit_and_keeps_focus() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "value");
        // Walk to the TOTP field.
        view.on_key(press(KeyCode::Tab)); // value -> description
        view.on_key(press(KeyCode::Tab)); // -> url
        view.on_key(press(KeyCode::Tab)); // -> totp
        assert_eq!(view.step(), Step::Totp);
        typ(&mut view, "short!!!");
        let out = view.on_key(ctrl('s'));
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Totp);
        assert!(view.status().is_some());
    }

    #[test]
    fn invalid_env_key_blocks_submit_and_keeps_focus() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "value");
        view.on_key(press(KeyCode::Tab)); // -> description
        view.on_key(press(KeyCode::Tab)); // -> url
        view.on_key(press(KeyCode::Tab)); // -> totp
        view.on_key(press(KeyCode::Tab)); // -> env_key
        assert_eq!(view.step(), Step::EnvKey);
        typ(&mut view, "1BAD");
        let out = view.on_key(ctrl('s'));
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::EnvKey);
        assert!(view.status().unwrap().contains("letter or underscore"));
    }

    #[test]
    fn invalid_expires_at_blocks_submit_and_keeps_focus() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "value");
        view.on_key(press(KeyCode::Tab)); // -> description
        view.on_key(press(KeyCode::Tab)); // -> url
        view.on_key(press(KeyCode::Tab)); // -> totp
        view.on_key(press(KeyCode::Tab)); // -> env_key
        view.on_key(press(KeyCode::Tab)); // -> expires_at
        assert_eq!(view.step(), Step::ExpiresAt);
        typ(&mut view, "not-a-duration");
        let out = view.on_key(ctrl('s'));
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::ExpiresAt);
        assert!(view.status().is_some());
    }

    #[test]
    fn expires_at_never_keyword_clears_to_none() {
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab));
        typ(&mut view, "value");
        view.on_key(press(KeyCode::Tab)); // description
        view.on_key(press(KeyCode::Tab)); // url
        view.on_key(press(KeyCode::Tab)); // totp
        view.on_key(press(KeyCode::Tab)); // env_key
        view.on_key(press(KeyCode::Tab)); // expires_at
        typ(&mut view, "never");
        let sv = view.build_secret_value().expect("valid build");
        assert!(sv.expires_at.is_none());
    }
}
