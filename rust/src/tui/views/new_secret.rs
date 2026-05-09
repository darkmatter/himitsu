//! New-secret form: in-TUI creation of a secret without shelling out.
//!
//! The form walks through a fixed sequence of fields:
//!
//! 1. **Path** — full secret path (e.g. `prod/API_KEY`). Slashes are allowed
//!    and purely organisational; they show up as folder headers in the
//!    search view.
//! 2. **Value** — multi-line buffer. `Enter` inserts a newline.
//! 3. **Description** — human-readable note.
//! 4. **Tags** — comma-separated labels (`pci,stripe`). Each tag must match
//!    the grammar `[A-Za-z0-9_.-]+`, 1-64 chars.
//! 5. **URL** — associated website or API.
//! 6. **TOTP** — `otpauth://` URI or base32 secret (validated).
//! 7. **Env key** — default env-var name (validated).
//! 8. **Expires at** — `never`, relative duration (`30d`/`6mo`/`1y`), or an
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
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};

use super::standard_canvas;

use crate::tui::theme;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::cli::duration::{self, ExpiresAt};
use crate::cli::set::{validate_env_key, validate_totp};
use crate::cli::Context;
use crate::crypto::{age, secret_value, tags as tag_grammar};
use crate::proto::SecretValue;
use crate::remote::store;
use crate::tui::keymap::{Bindings, KeyAction, KeyMap};

/// New-secret form's keymap action priority (excluding NextField, which
/// is dispatched inside the field-specific handlers since it must run
/// the per-field validator before advancing). Cancel comes before save
/// so an explicit Esc binding always wins over any save chord that
/// happens to share its first key.
const FORM_ACTION_PRIORITY: &[KeyAction] = &[
    KeyAction::Cancel,
    KeyAction::SaveSecret,
    KeyAction::PrevField,
];

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
    Tags,
    Url,
    Totp,
    EnvKey,
    ExpiresAt,
    /// Final tab stop — a submit-button row. Pressing Enter here triggers
    /// the same `submit()` path as Ctrl+S/Ctrl+W.
    Submit,
}

impl Step {
    /// All steps in display/tab order. Used for Tab / Shift-Tab cycling
    /// and for tests asserting the cycle visits every field.
    const ORDER: [Step; 9] = [
        Step::Path,
        Step::Value,
        Step::Description,
        Step::Tags,
        Step::Url,
        Step::Totp,
        Step::EnvKey,
        Step::ExpiresAt,
        Step::Submit,
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

/// Buttons in the unsaved-changes confirm dialog. Drawn left-to-right in the
/// declared order; `KeepEditing` is the safe default focus when the modal
/// opens (a stray Esc + Enter shouldn't lose work).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmButton {
    KeepEditing,
    Save,
    Discard,
}

impl ConfirmButton {
    const ORDER: [ConfirmButton; 3] = [
        ConfirmButton::KeepEditing,
        ConfirmButton::Save,
        ConfirmButton::Discard,
    ];

    fn index(self) -> usize {
        Self::ORDER.iter().position(|b| *b == self).unwrap()
    }

    fn next(self) -> Self {
        let i = self.index();
        Self::ORDER[(i + 1) % Self::ORDER.len()]
    }

    fn prev(self) -> Self {
        let i = self.index();
        Self::ORDER[(i + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }

    fn label(self) -> &'static str {
        match self {
            ConfirmButton::KeepEditing => "[ keep editing ]",
            ConfirmButton::Save => "[ save ]",
            ConfirmButton::Discard => "[ discard ]",
        }
    }
}

pub struct NewSecretView {
    step: Step,
    path: String,
    value: String,
    description: String,
    tags: String,
    url: String,
    totp: String,
    env_key: String,
    expires_at: String,
    status: Option<String>,
    /// `Some(focused_button)` while the unsaved-changes modal is up.
    /// `None` means the modal is closed and the form behaves normally.
    confirm_exit: Option<ConfirmButton>,
    ctx: Context,
}

impl NewSecretView {
    pub fn new(ctx: &Context) -> Self {
        Self {
            step: Step::Path,
            path: String::new(),
            value: String::new(),
            description: String::new(),
            tags: String::new(),
            url: String::new(),
            totp: String::new(),
            env_key: String::new(),
            expires_at: String::new(),
            status: None,
            confirm_exit: None,
            ctx: ctx.clone(),
        }
    }

    #[cfg(test)]
    pub fn confirm_exit(&self) -> Option<ConfirmButton> {
        self.confirm_exit
    }

    /// `true` if any field carries content the user might want to save.
    /// Used to decide whether Esc opens the confirm dialog or just bails.
    fn has_unsaved_changes(&self) -> bool {
        !self.path.is_empty()
            || !self.value.is_empty()
            || !self.description.is_empty()
            || !self.tags.is_empty()
            || !self.url.is_empty()
            || !self.totp.is_empty()
            || !self.env_key.is_empty()
            || !self.expires_at.is_empty()
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
    /// routes through this single-line path. The `Submit` step has no
    /// buffer — its handler is keyboard-only.
    fn field_buffer_mut(&mut self, step: Step) -> Option<&mut String> {
        match step {
            Step::Path => Some(&mut self.path),
            Step::Value => None,
            Step::Description => Some(&mut self.description),
            Step::Tags => Some(&mut self.tags),
            Step::Url => Some(&mut self.url),
            Step::Totp => Some(&mut self.totp),
            Step::EnvKey => Some(&mut self.env_key),
            Step::ExpiresAt => Some(&mut self.expires_at),
            Step::Submit => None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> NewSecretAction {
        // Ctrl-C is always a quit; it is hard-coded rather than remappable
        // because users need a reliable panic button even if the configured
        // `quit` binding happens to overlap a printable character the form
        // is trying to capture.
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
        ) {
            return NewSecretAction::Quit;
        }

        // Confirm dialog swallows everything else while active.
        if self.confirm_exit.is_some() {
            return self.handle_confirm_exit_key(key);
        }

        // Resolve cancel / save / prev_field up front so a chord-completed
        // action takes the same path as the bare keystroke. NextField
        // stays inside the field-specific handlers because it interacts
        // with per-field validation.
        if let Some(action) = keymap.action_for_key_in(&key, FORM_ACTION_PRIORITY) {
            if let Some(outcome) = self.dispatch_action(action) {
                return outcome;
            }
        }

        match self.step {
            Step::Value => self.handle_value_key(key, keymap),
            Step::Submit => self.handle_submit_step_key(key, keymap),
            _ => self.handle_single_line_key(key, keymap),
        }
    }

    /// Run a [`KeyAction`] against the new-secret form. Returns `None` for
    /// actions this form doesn't own (e.g. NextField, which is intentionally
    /// scoped to the field-specific handlers below so it interacts with the
    /// per-field validate-then-advance flow). Cancel routes through the
    /// confirm-exit gate so a stray Esc never silently throws away typed
    /// input.
    pub fn dispatch_action(&mut self, action: KeyAction) -> Option<NewSecretAction> {
        match action {
            KeyAction::Cancel => {
                if self.has_unsaved_changes() {
                    self.confirm_exit = Some(ConfirmButton::KeepEditing);
                    Some(NewSecretAction::None)
                } else {
                    Some(NewSecretAction::Cancel)
                }
            }
            KeyAction::SaveSecret => Some(self.submit()),
            KeyAction::PrevField => {
                self.move_to(self.step.prev());
                Some(NewSecretAction::None)
            }
            _ => None,
        }
    }

    /// Submit-step keys: Enter triggers submission; Up/Tab/Down navigate;
    /// every other key is ignored (no buffer to type into).
    fn handle_submit_step_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> NewSecretAction {
        if keymap.next_field.matches(&key) {
            self.move_to(self.step.next());
            return NewSecretAction::None;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => self.submit(),
            (KeyCode::Up, _) => {
                self.move_to(self.step.prev());
                NewSecretAction::None
            }
            (KeyCode::Down, _) => {
                self.move_to(self.step.next());
                NewSecretAction::None
            }
            _ => NewSecretAction::None,
        }
    }

    /// Modal key handler for the unsaved-changes confirm dialog. Left/Right
    /// (and Tab) cycle the focused button, Enter activates it, Esc resolves
    /// to the safe option (`keep editing`) — i.e. dismiss the modal without
    /// taking action.
    fn handle_confirm_exit_key(&mut self, key: KeyEvent) -> NewSecretAction {
        let Some(focused) = self.confirm_exit else {
            return NewSecretAction::None;
        };
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.confirm_exit = None;
                NewSecretAction::None
            }
            (KeyCode::Left, _) => {
                self.confirm_exit = Some(focused.prev());
                NewSecretAction::None
            }
            (KeyCode::Right, _) | (KeyCode::Tab, _) => {
                self.confirm_exit = Some(focused.next());
                NewSecretAction::None
            }
            (KeyCode::Enter, _) => match focused {
                ConfirmButton::Save => {
                    self.confirm_exit = None;
                    self.submit()
                }
                ConfirmButton::Discard => {
                    self.confirm_exit = None;
                    NewSecretAction::Cancel
                }
                ConfirmButton::KeepEditing => {
                    self.confirm_exit = None;
                    NewSecretAction::None
                }
            },
            _ => NewSecretAction::None,
        }
    }

    /// Single-line editor used by every field except `Value`. `Tab` / `Enter`
    /// advances to the next field (running field-local validation first);
    /// `Backspace` erases; printable chars append.
    fn handle_single_line_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> NewSecretAction {
        // Configurable "advance to next field" takes precedence over the
        // raw Enter/Tab fall-through so a custom `next_field` binding can
        // still steer field navigation.
        if keymap.next_field.matches(&key) {
            if let Err(msg) = self.validate_current_field() {
                self.status = Some(msg);
                return NewSecretAction::None;
            }
            self.status = None;
            self.move_to(self.step.next());
            return NewSecretAction::None;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
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
                if !Self::accepts_char(self.step, c) {
                    return NewSecretAction::None;
                }
                if let Some(buf) = self.field_buffer_mut(self.step) {
                    buf.push(c);
                }
                NewSecretAction::None
            }
            _ => NewSecretAction::None,
        }
    }

    /// Per-step input filter. The `Tags` step restricts typing to the
    /// `[A-Za-z0-9_.-,]` alphabet so the buffer can never carry a byte
    /// the grammar would later reject. Every other step accepts any
    /// printable char and defers validation to leave/submit time.
    fn accepts_char(step: Step, c: char) -> bool {
        match step {
            Step::Tags => c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' || c == ',',
            _ => true,
        }
    }

    /// Multi-line editor for `Value`. `Enter` inserts a newline; `Tab` moves
    /// to the next field; `Backspace` erases; `Ctrl+S` / `Ctrl+W` submit
    /// (handled in `on_key` before dispatch).
    fn handle_value_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> NewSecretAction {
        // `next_field` is checked before the `Enter` case so a configured
        // `Tab`/custom binding advances instead of inserting a newline.
        if keymap.next_field.matches(&key) {
            self.move_to(self.step.next());
            return NewSecretAction::None;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                self.value.push('\n');
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
            Step::Tags => parse_tags_input(&self.tags).map(|_| ()),
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
            Step::Submit => Ok(()),
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

        let tags = parse_tags_input(&self.tags)?;

        Ok(SecretValue {
            data: self.value.as_bytes().to_vec(),
            content_type: String::new(),
            annotations: Default::default(),
            totp: self.totp.clone(),
            url: self.url.clone(),
            expires_at: expires_at_ts,
            description: self.description.clone(),
            env_key: self.env_key.clone(),
            tags,
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

        for step in [Step::Tags, Step::Totp, Step::EnvKey, Step::ExpiresAt] {
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

        let recipients =
            match age::collect_recipients(&self.ctx.store, self.ctx.recipients_path.as_deref()) {
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
        let area = standard_canvas(frame.area());
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // header
                Constraint::Length(3), // path
                Constraint::Min(3),    // value
                Constraint::Length(3), // description
                Constraint::Length(3), // tags
                Constraint::Length(3), // url
                Constraint::Length(3), // totp
                Constraint::Length(3), // env_key
                Constraint::Length(3), // expires_at
                Constraint::Length(3), // submit button
                Constraint::Length(1), // footer
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_single_line(
            frame,
            chunks[1],
            Step::Path,
            " path ",
            &self.path,
            "prod/api/STRIPE_KEY",
        );
        self.draw_value_field(frame, chunks[2]);
        self.draw_single_line(
            frame,
            chunks[3],
            Step::Description,
            " description ",
            &self.description,
            "human-readable note (optional)",
        );
        self.draw_single_line(
            frame,
            chunks[4],
            Step::Tags,
            " tags ",
            &self.tags,
            "comma-separated, e.g. pci,stripe",
        );
        self.draw_single_line(
            frame,
            chunks[5],
            Step::Url,
            " url ",
            &self.url,
            "https://example.com",
        );
        self.draw_single_line(
            frame,
            chunks[6],
            Step::Totp,
            " totp ",
            &self.totp,
            "otpauth://... or base32 secret",
        );
        self.draw_single_line(
            frame,
            chunks[7],
            Step::EnvKey,
            " env_key ",
            &self.env_key,
            "STRIPE_KEY",
        );
        self.draw_single_line(
            frame,
            chunks[8],
            Step::ExpiresAt,
            " expires_at ",
            &self.expires_at,
            "never | 30d | 6mo | 2027-01-01T00:00:00Z",
        );
        self.draw_submit_button(frame, chunks[9]);
        self.draw_footer(frame, chunks[10]);

        // Modal overlay paints last so it sits above the form.
        if let Some(focused) = self.confirm_exit {
            self.draw_confirm_exit(frame, focused);
        }
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut spans = theme::brand_chip("秘 himitsu");
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "new secret",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_single_line(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        step: Step,
        title: &str,
        content: &str,
        placeholder: &str,
    ) {
        let focused = self.step == step;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title.to_string())
            .title_style(Style::default().fg(theme::border_label()))
            .border_style(Self::border_style(focused));
        let para = Self::field_paragraph(content, placeholder, focused).block(block);
        frame.render_widget(para, area);
    }

    fn draw_value_field(&self, frame: &mut Frame<'_>, area: Rect) {
        let focused = self.step == Step::Value;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" value ")
            .title_style(Style::default().fg(theme::border_label()))
            .border_style(Self::border_style(focused));
        let para = Self::field_paragraph(
            &self.value,
            "value here — Enter inserts a newline",
            focused,
        )
        .block(block)
        .wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }

    /// Build the paragraph body shared by every field: muted placeholder
    /// when empty + unfocused, otherwise the buffer with a trailing cursor
    /// while focused. Centralised so the placeholder behaviour is uniform
    /// across single-line and multi-line inputs.
    fn field_paragraph<'a>(
        content: &'a str,
        placeholder: &'a str,
        focused: bool,
    ) -> Paragraph<'a> {
        if !focused && content.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                placeholder,
                Style::default().fg(theme::muted()),
            )))
        } else {
            let mut text = content.to_string();
            if focused {
                text.push('_');
            }
            Paragraph::new(text)
        }
    }

    fn border_style(focused: bool) -> Style {
        if focused {
            Style::default().fg(theme::accent())
        } else {
            Style::default().fg(theme::muted())
        }
    }

    /// Submit-button row. Same 3-row footprint as a text field so the layout
    /// above stays stable; only the contents and border colour change with
    /// focus.
    fn draw_submit_button(&self, frame: &mut Frame<'_>, area: Rect) {
        let focused = self.step == Step::Submit;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Self::border_style(focused));
        let label_style = if focused {
            Style::default()
                .fg(theme::accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::muted())
        };
        let para = Paragraph::new(Line::from(Span::styled("[ submit ]", label_style)))
            .alignment(Alignment::Center)
            .block(block);
        frame.render_widget(para, area);
    }

    /// Centered "unsaved changes" popup. Painted on top of the form when the
    /// user presses Esc with at least one populated field.
    fn draw_confirm_exit(&self, frame: &mut Frame<'_>, focused: ConfirmButton) {
        let area = confirm_popup_rect(frame.area());
        frame.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(
                " unsaved changes ",
                Style::default()
                    .fg(theme::border_label())
                    .add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::default().fg(theme::accent()));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // top pad
                Constraint::Length(1), // body
                Constraint::Length(1), // pad
                Constraint::Length(1), // buttons
                Constraint::Min(0),
            ])
            .split(inner);

        let body = Paragraph::new(Line::from(Span::styled(
            "save the new secret, discard, or keep editing?",
            Style::default().fg(theme::footer_text()),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(body, rows[1]);

        let buttons: Vec<Span<'_>> = ConfirmButton::ORDER
            .iter()
            .enumerate()
            .flat_map(|(i, b)| {
                let style = if *b == focused {
                    Style::default()
                        .fg(theme::accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::muted())
                };
                let mut spans = vec![Span::styled(b.label(), style)];
                if i + 1 < ConfirmButton::ORDER.len() {
                    spans.push(Span::raw("  "));
                }
                spans
            })
            .collect();
        frame.render_widget(
            Paragraph::new(Line::from(buttons)).alignment(Alignment::Center),
            rows[3],
        );
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = if let Some(msg) = &self.status {
            Line::from(Span::styled(
                msg.clone(),
                Style::default().fg(theme::danger()),
            ))
        } else {
            let footer = Style::default().fg(theme::footer_text());
            Line::from(vec![
                Span::styled("tab", Style::default().fg(theme::accent())),
                Span::styled(" next    ", footer),
                Span::styled("shift-tab", Style::default().fg(theme::accent())),
                Span::styled(" prev    ", footer),
                Span::styled("ctrl-s / ctrl-w", Style::default().fg(theme::accent())),
                Span::styled(" save    ", footer),
                Span::styled("esc", Style::default().fg(theme::accent())),
                Span::styled(" cancel    ", footer),
                Span::styled("ctrl-c", Style::default().fg(theme::accent())),
                Span::styled(" quit", footer),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }

    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("tab / enter", "next field (wraps); tab cycles into [ submit ]"),
            ("shift-tab", "previous field (wraps)"),
            ("enter (value)", "insert newline"),
            ("enter (submit)", "save the new secret"),
            ("ctrl-s / ctrl-w", "save from any field"),
            ("esc", "cancel (prompts if any field has content)"),
            ("ctrl-c", "quit"),
            ("?", "toggle this help"),
        ]
    }

    pub fn help_title() -> &'static str {
        "new secret · keys"
    }
}

/// Centered rect for the unsaved-changes popup. ~50 columns × 7 rows keeps
/// the dialog readable on small terminals while staying compact.
fn confirm_popup_rect(area: Rect) -> Rect {
    const W: u16 = 50;
    const H: u16 = 7;
    let width = W.min(area.width);
    let height = H.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width) / 2),
        y: area.y + (area.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

/// Parse the raw "comma-separated tags" buffer into a validated list.
///
/// Splits on `,`, trims whitespace around each piece, drops empties, and
/// runs every remaining piece through [`tag_grammar::validate_tag`]. The
/// returned `Vec` is owned so it can move straight into
/// `SecretValue.tags`. Empty input yields an empty vec.
fn parse_tags_input(raw: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for piece in raw.split(',') {
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        tag_grammar::validate_tag(trimmed)?;
        out.push(trimmed.to_string());
    }
    Ok(out)
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keymap::KeyMap;
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
        let km = KeyMap::default();
        for c in s.chars() {
            view.on_key(press(KeyCode::Char(c)), &km);
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
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/API_KEY");
        view.on_key(press(KeyCode::Enter), &km);
        assert_eq!(view.step(), Step::Value);
        assert_eq!(view.path(), "prod/API_KEY");
    }

    #[test]
    fn enter_on_empty_path_is_rejected_with_status() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        view.on_key(press(KeyCode::Enter), &km);
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
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod");
        view.on_key(press(KeyCode::Backspace), &km);
        assert_eq!(view.path(), "pro");
    }

    #[test]
    fn value_step_treats_enter_as_newline() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Enter), &km); // path -> value
        typ(&mut view, "line1");
        view.on_key(press(KeyCode::Enter), &km);
        typ(&mut view, "line2");
        assert_eq!(view.value(), "line1\nline2");
        assert_eq!(view.step(), Step::Value);
    }

    #[test]
    fn esc_on_empty_form_cancels_immediately() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        // No fields populated → no point prompting.
        assert!(matches!(
            view.on_key(press(KeyCode::Esc), &km),
            NewSecretAction::Cancel
        ));
        assert!(view.confirm_exit().is_none());
    }

    #[test]
    fn esc_with_unsaved_changes_opens_confirm_dialog() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "x");
        let out = view.on_key(press(KeyCode::Esc), &km);
        // Modal swallows the cancel — the form must stay alive.
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.confirm_exit(), Some(ConfirmButton::KeepEditing));
    }

    #[test]
    fn ctrl_c_quits_from_any_step() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        assert!(matches!(view.on_key(ctrl('c'), &km), NewSecretAction::Quit));
    }

    #[test]
    fn submit_with_empty_value_fails_and_refocuses_value_step() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Enter), &km);
        let out = view.on_key(ctrl('s'), &km);
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Value);
        assert!(view.status().unwrap().contains("value"));
    }

    #[test]
    fn submit_with_missing_path_refocuses_path_step() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        // Jump forward with Tab even though path is empty — submit() should
        // drag focus back to Path.
        view.on_key(press(KeyCode::Tab), &km);
        let out = view.on_key(ctrl('s'), &km);
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Path);
    }

    #[test]
    fn ctrl_w_submits_like_ctrl_s() {
        let km = KeyMap::default();
        // hm-y6n: tmux-safe alternative to ctrl+s.
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Enter), &km);
        let out = view.on_key(ctrl('w'), &km);
        // Empty value rejects — confirms the chord reached submit().
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Value);
        assert!(view.status().unwrap().contains("value"));
    }

    #[test]
    fn tab_cycle_visits_every_field_and_wraps_to_path() {
        let km = KeyMap::default();
        // hm-r4i + hm-3rr: cycling forward must hit every metadata field,
        // pass through the new submit step, and wrap back to path.
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        let expected = [
            Step::Path,
            Step::Value,
            Step::Description,
            Step::Tags,
            Step::Url,
            Step::Totp,
            Step::EnvKey,
            Step::ExpiresAt,
            Step::Submit,
            Step::Path, // wrap-around
        ];
        let mut seen = vec![view.step()];
        for _ in 0..expected.len() - 1 {
            view.on_key(press(KeyCode::Tab), &km);
            seen.push(view.step());
        }
        assert_eq!(seen, expected);
    }

    #[test]
    fn shift_tab_wraps_backward_from_path_to_submit() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        assert_eq!(view.step(), Step::Path);
        view.on_key(back_tab(), &km);
        // Submit is now the last tab stop.
        assert_eq!(view.step(), Step::Submit);
        view.on_key(back_tab(), &km);
        assert_eq!(view.step(), Step::ExpiresAt);
    }

    #[test]
    fn enter_on_submit_step_invokes_submit() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        // Walk all the way to Submit (8 Tab presses from Path).
        for _ in 0..8 {
            view.on_key(press(KeyCode::Tab), &km);
        }
        assert_eq!(view.step(), Step::Submit);
        let out = view.on_key(press(KeyCode::Enter), &km);
        // Empty value rejects with focus snapped back to Value — confirms
        // Enter on the Submit step reached submit().
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Value);
        assert!(view.status().unwrap().contains("value"));
    }

    #[test]
    fn confirm_dialog_right_cycles_focused_button() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "x");
        view.on_key(press(KeyCode::Esc), &km);
        assert_eq!(view.confirm_exit(), Some(ConfirmButton::KeepEditing));
        view.on_key(press(KeyCode::Right), &km);
        assert_eq!(view.confirm_exit(), Some(ConfirmButton::Save));
        view.on_key(press(KeyCode::Right), &km);
        assert_eq!(view.confirm_exit(), Some(ConfirmButton::Discard));
    }

    #[test]
    fn confirm_dialog_left_reverses() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "x");
        view.on_key(press(KeyCode::Esc), &km);
        assert_eq!(view.confirm_exit(), Some(ConfirmButton::KeepEditing));
        view.on_key(press(KeyCode::Left), &km);
        assert_eq!(view.confirm_exit(), Some(ConfirmButton::Discard));
        view.on_key(press(KeyCode::Left), &km);
        assert_eq!(view.confirm_exit(), Some(ConfirmButton::Save));
    }

    #[test]
    fn confirm_dialog_enter_on_discard_emits_cancel() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "x");
        view.on_key(press(KeyCode::Esc), &km);
        // KeepEditing → Save → Discard
        view.on_key(press(KeyCode::Right), &km);
        view.on_key(press(KeyCode::Right), &km);
        assert_eq!(view.confirm_exit(), Some(ConfirmButton::Discard));
        let out = view.on_key(press(KeyCode::Enter), &km);
        assert!(matches!(out, NewSecretAction::Cancel));
    }

    #[test]
    fn confirm_dialog_esc_returns_to_form() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "x");
        view.on_key(press(KeyCode::Esc), &km);
        assert!(view.confirm_exit().is_some());
        let out = view.on_key(press(KeyCode::Esc), &km);
        assert!(matches!(out, NewSecretAction::None));
        assert!(view.confirm_exit().is_none());
        // The form is still editable: the buffer is intact.
        assert_eq!(view.path(), "x");
    }

    #[test]
    fn full_metadata_roundtrip_populates_secret_value() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/API_KEY");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "hunter2");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "the prod api key");
        view.on_key(press(KeyCode::Tab), &km); // -> tags
        typ(&mut view, "pci,stripe");
        view.on_key(press(KeyCode::Tab), &km); // -> url
        typ(&mut view, "https://api.example.com");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "API_KEY");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "30d");

        let sv = view.build_secret_value().expect("valid build");
        assert_eq!(sv.data, b"hunter2");
        assert_eq!(sv.description, "the prod api key");
        assert_eq!(sv.tags, vec!["pci".to_string(), "stripe".to_string()]);
        assert_eq!(sv.url, "https://api.example.com");
        assert_eq!(sv.totp, "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP");
        assert_eq!(sv.env_key, "API_KEY");
        assert!(sv.expires_at.is_some());
    }

    #[test]
    fn empty_optional_fields_stay_empty_in_secret_value() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "value");

        let sv = view.build_secret_value().expect("valid build");
        assert_eq!(sv.description, "");
        assert!(sv.tags.is_empty());
        assert_eq!(sv.url, "");
        assert_eq!(sv.totp, "");
        assert_eq!(sv.env_key, "");
        assert!(sv.expires_at.is_none());
    }

    #[test]
    fn invalid_totp_blocks_submit_and_keeps_focus() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "value");
        // Walk to the TOTP field.
        view.on_key(press(KeyCode::Tab), &km); // value -> description
        view.on_key(press(KeyCode::Tab), &km); // -> tags
        view.on_key(press(KeyCode::Tab), &km); // -> url
        view.on_key(press(KeyCode::Tab), &km); // -> totp
        assert_eq!(view.step(), Step::Totp);
        typ(&mut view, "short!!!");
        let out = view.on_key(ctrl('s'), &km);
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Totp);
        assert!(view.status().is_some());
    }

    #[test]
    fn invalid_env_key_blocks_submit_and_keeps_focus() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "value");
        view.on_key(press(KeyCode::Tab), &km); // -> description
        view.on_key(press(KeyCode::Tab), &km); // -> tags
        view.on_key(press(KeyCode::Tab), &km); // -> url
        view.on_key(press(KeyCode::Tab), &km); // -> totp
        view.on_key(press(KeyCode::Tab), &km); // -> env_key
        assert_eq!(view.step(), Step::EnvKey);
        typ(&mut view, "1BAD");
        let out = view.on_key(ctrl('s'), &km);
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::EnvKey);
        assert!(view.status().unwrap().contains("letter or underscore"));
    }

    #[test]
    fn invalid_expires_at_blocks_submit_and_keeps_focus() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "value");
        view.on_key(press(KeyCode::Tab), &km); // -> description
        view.on_key(press(KeyCode::Tab), &km); // -> tags
        view.on_key(press(KeyCode::Tab), &km); // -> url
        view.on_key(press(KeyCode::Tab), &km); // -> totp
        view.on_key(press(KeyCode::Tab), &km); // -> env_key
        view.on_key(press(KeyCode::Tab), &km); // -> expires_at
        assert_eq!(view.step(), Step::ExpiresAt);
        typ(&mut view, "not-a-duration");
        let out = view.on_key(ctrl('s'), &km);
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::ExpiresAt);
        assert!(view.status().is_some());
    }

    #[test]
    fn expires_at_never_keyword_clears_to_none() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "value");
        view.on_key(press(KeyCode::Tab), &km); // description
        view.on_key(press(KeyCode::Tab), &km); // tags
        view.on_key(press(KeyCode::Tab), &km); // url
        view.on_key(press(KeyCode::Tab), &km); // totp
        view.on_key(press(KeyCode::Tab), &km); // env_key
        view.on_key(press(KeyCode::Tab), &km); // expires_at
        typ(&mut view, "never");
        let sv = view.build_secret_value().expect("valid build");
        assert!(sv.expires_at.is_none());
    }

    // ── Tags step ───────────────────────────────────────────────────────

    #[test]
    fn parse_tags_input_splits_simple_csv() {
        assert_eq!(
            parse_tags_input("a,b,c").unwrap(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn parse_tags_input_trims_whitespace_around_each_piece() {
        assert_eq!(
            parse_tags_input(" a , b ").unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn parse_tags_input_drops_empty_pieces() {
        assert_eq!(
            parse_tags_input("a,,b").unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn parse_tags_input_rejects_invalid_tag_in_list() {
        // Belt-and-braces against pasted/injected input that bypasses the
        // typing-time filter.
        let err = parse_tags_input("a,bad tag,b").unwrap_err();
        assert!(
            err.contains("bad tag"),
            "error mentions offending tag: {err}"
        );
    }

    #[test]
    fn parse_tags_input_empty_string_yields_empty_vec() {
        assert_eq!(parse_tags_input("").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn tags_step_filters_disallowed_characters_at_typing_time() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        // Walk to the Tags step.
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab), &km); // -> value
        typ(&mut view, "v");
        view.on_key(press(KeyCode::Tab), &km); // -> description
        view.on_key(press(KeyCode::Tab), &km); // -> tags
        assert_eq!(view.step(), Step::Tags);
        // Space, "!", ":", "/" must not land in the buffer.
        typ(&mut view, "pci, stripe!:/");
        let sv = view.build_secret_value().expect("valid build");
        assert_eq!(sv.tags, vec!["pci".to_string(), "stripe".to_string()]);
    }

    #[test]
    fn invalid_tags_block_submit_and_refocus_tags_step() {
        let km = KeyMap::default();
        let mut view = NewSecretView::new(&empty_ctx());
        typ(&mut view, "prod/KEY");
        view.on_key(press(KeyCode::Tab), &km);
        typ(&mut view, "v");
        // A 65-char tag exceeds MAX_TAG_LEN. Bypassing the typing-time
        // filter by writing straight to the buffer simulates a paste.
        view.tags = "a".repeat(65);
        let out = view.on_key(ctrl('s'), &km);
        assert!(matches!(out, NewSecretAction::None));
        assert_eq!(view.step(), Step::Tags);
        assert!(view.status().is_some());
    }
}
