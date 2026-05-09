//! Generic, protobuf-driven form widget for the TUI.
//!
//! The TUI exposes a handful of mutating CLI commands that historically had
//! no in-app affordance — `himitsu remote add`, `himitsu recipient add`, etc.
//! Each one used to require shelling out, breaking flow.
//!
//! Rather than hand-roll a bespoke view per command, this module provides
//! a generic [`FormView`] that renders a list of [`Field`]s as labelled
//! inputs and handles navigation, validation, and submission. The shape of
//! each form (which fields exist, what they're called, how they validate)
//! is supplied by an implementation of the [`ProtoForm`] trait, which lives
//! next to the corresponding generated proto message in
//! [`crate::proto::commands`].
//!
//! The protobuf message *is* the schema: adding a field to the proto and
//! the trait impl is enough to surface a new input in the TUI. The form
//! widget itself stays command-agnostic.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::keymap::{Bindings, KeyMap};
use crate::tui::theme;

/// What kind of input the form should render for a [`Field`].
///
/// Kept deliberately small: anything more elaborate (a date picker, a
/// recipient selector) will graduate to its own field variant when a real
/// command needs it. Premature abstraction here just means dead code paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Widget {
    /// Single-line text input (most flag-style args).
    Text,
    /// Multi-line text input. `Enter` inserts a newline; navigation moves
    /// off the field via `Tab` / `Shift-Tab` only.
    TextArea,
}

/// A function pointer validator. Returns `Err(message)` to block field-leave
/// and submit. We use a `fn` rather than `Box<dyn Fn>` so [`Field`] stays
/// `Clone` cheaply — every validator we currently need is stateless.
pub type Validator = fn(&str) -> Result<(), String>;

/// One input in a [`FormView`]. The `name` is the stable identifier used by
/// [`ProtoForm::from_form`] to pick values back out; `label` and `help` are
/// purely presentational.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub widget: Widget,
    pub required: bool,
    pub value: String,
    pub validator: Option<Validator>,
    /// Hint shown in muted style when the field is empty AND unfocused.
    /// Cleared as soon as the user types or focuses the field — it is a
    /// suggestion, not a default.
    pub placeholder: Option<&'static str>,
}

impl Field {
    /// Convenience constructor for the common case: a single-line, optional
    /// text field with no validator.
    pub fn text(name: &'static str, label: &'static str, help: &'static str) -> Self {
        Self {
            name,
            label,
            help,
            widget: Widget::Text,
            required: false,
            value: String::new(),
            validator: None,
            placeholder: None,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn with_validator(mut self, v: Validator) -> Self {
        self.validator = Some(v);
        self
    }

    pub fn placeholder(mut self, p: &'static str) -> Self {
        self.placeholder = Some(p);
        self
    }

    fn validate(&self) -> Result<(), String> {
        if self.required && self.value.trim().is_empty() {
            return Err(format!("{} is required", self.label));
        }
        if self.value.trim().is_empty() {
            return Ok(());
        }
        if let Some(v) = self.validator {
            return v(&self.value);
        }
        Ok(())
    }
}

/// Bridge between a generated proto message and the form widget.
///
/// Implement on each `*Args` message in [`crate::proto::commands`]. The
/// trait owns three responsibilities: declaring the form's title, declaring
/// the ordered field list (with labels / help / validators), and turning
/// the populated form back into a typed instance of the message.
pub trait ProtoForm: Sized {
    /// Human-readable form title shown in the header.
    fn form_title() -> &'static str;

    /// Ordered list of fields. Order is the tab-cycle order users will see.
    fn form_fields() -> Vec<Field>;

    /// Build the typed message from the populated form. Called after the
    /// form's per-field validators have already passed; this hook exists for
    /// cross-field invariants the per-field validators can't express.
    fn from_form(form: &FormView) -> Result<Self, String>;
}

/// Outcome of handing a key event to a [`FormView`].
///
/// The widget handles navigation and editing internally; consuming views
/// only have to react to the high-level intents below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormOutcome {
    /// Stay open, redraw with updated state.
    Pending,
    /// User pressed Esc — abort without submitting.
    Cancel,
    /// User pressed Ctrl+C — quit the whole app.
    Quit,
    /// All field-level validators passed and the user pressed Ctrl+S /
    /// Ctrl+W. The consumer should now call [`ProtoForm::from_form`] to
    /// extract the typed message and perform the action; on failure it can
    /// push a status message back via [`FormView::set_status`].
    Submit,
}

/// Generic field-list editor. Owns the field buffers, focus index, and
/// status line; renders inside any [`Rect`] the consumer hands it.
pub struct FormView {
    title: String,
    fields: Vec<Field>,
    focused: usize,
    status: Option<String>,
}

impl FormView {
    pub fn new(title: impl Into<String>, fields: Vec<Field>) -> Self {
        Self {
            title: title.into(),
            fields,
            focused: 0,
            status: None,
        }
    }

    /// Build a form for a `ProtoForm`-implementing message in one call.
    pub fn for_proto<P: ProtoForm>() -> Self {
        Self::new(P::form_title(), P::form_fields())
    }

    /// Look up a field's current buffer by name. Used by `from_form` impls
    /// to reconstruct typed messages without depending on field order.
    pub fn field_value(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.as_str())
    }

    /// Push a transient message into the form's status line. Cleared next
    /// time the user edits a field or moves focus.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn focused_field(&self) -> &Field {
        &self.fields[self.focused]
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    pub fn on_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> FormOutcome {
        // Hard-wired panic button — see new_secret.rs for the same rationale:
        // Ctrl+C must work even if a remap collides with a printable char the
        // form is trying to capture.
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
        ) {
            return FormOutcome::Quit;
        }
        if keymap.cancel.matches(&key) {
            return FormOutcome::Cancel;
        }
        if keymap.save_secret.matches(&key) {
            return self.try_submit();
        }
        if keymap.prev_field.matches(&key) {
            self.move_focus(-1);
            return FormOutcome::Pending;
        }
        if keymap.next_field.matches(&key) {
            // Field-leave validation: surface bad input *before* moving focus
            // so the cursor stays where the problem is.
            if let Err(msg) = self.fields[self.focused].validate() {
                self.status = Some(msg);
                return FormOutcome::Pending;
            }
            self.status = None;
            self.move_focus(1);
            return FormOutcome::Pending;
        }

        let widget = self.fields[self.focused].widget.clone();
        match widget {
            Widget::Text => self.handle_single_line_key(key),
            Widget::TextArea => self.handle_text_area_key(key),
        }
    }

    fn handle_single_line_key(&mut self, key: KeyEvent) -> FormOutcome {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                if let Err(msg) = self.fields[self.focused].validate() {
                    self.status = Some(msg);
                    return FormOutcome::Pending;
                }
                self.status = None;
                self.move_focus(1);
                FormOutcome::Pending
            }
            (KeyCode::Up, _) => {
                self.move_focus(-1);
                FormOutcome::Pending
            }
            (KeyCode::Down, _) => {
                self.move_focus(1);
                FormOutcome::Pending
            }
            (KeyCode::Backspace, _) => {
                self.fields[self.focused].value.pop();
                FormOutcome::Pending
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.fields[self.focused].value.push(c);
                FormOutcome::Pending
            }
            _ => FormOutcome::Pending,
        }
    }

    fn handle_text_area_key(&mut self, key: KeyEvent) -> FormOutcome {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                self.fields[self.focused].value.push('\n');
                FormOutcome::Pending
            }
            (KeyCode::Backspace, _) => {
                self.fields[self.focused].value.pop();
                FormOutcome::Pending
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.fields[self.focused].value.push(c);
                FormOutcome::Pending
            }
            _ => FormOutcome::Pending,
        }
    }

    fn move_focus(&mut self, delta: isize) {
        let len = self.fields.len() as isize;
        let next = (self.focused as isize + delta).rem_euclid(len) as usize;
        self.focused = next;
    }

    /// Run every validator. On the first failure, snap focus to that field
    /// and surface the message; otherwise return [`FormOutcome::Submit`] so
    /// the consumer can pull values out via [`ProtoForm::from_form`].
    fn try_submit(&mut self) -> FormOutcome {
        for (i, field) in self.fields.iter().enumerate() {
            if let Err(msg) = field.validate() {
                self.focused = i;
                self.status = Some(msg);
                return FormOutcome::Pending;
            }
        }
        FormOutcome::Submit
    }

    /// Render the form into `area`. Layout: a 1-row header, one row per
    /// field (3 rows for `TextArea`), a help line, and a footer/status line
    /// at the bottom.
    pub fn draw(&self, frame: &mut Frame<'_>, area: Rect) {
        // One constraint per field plus header + help + footer rows.
        let mut constraints: Vec<Constraint> = vec![Constraint::Length(1)]; // header
        for field in &self.fields {
            constraints.push(match field.widget {
                Widget::Text => Constraint::Length(3),
                Widget::TextArea => Constraint::Min(3),
            });
        }
        constraints.push(Constraint::Length(1)); // help
        constraints.push(Constraint::Length(1)); // footer

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        self.draw_header(frame, chunks[0]);

        for (i, field) in self.fields.iter().enumerate() {
            self.draw_field(frame, chunks[i + 1], field, i == self.focused);
        }

        let help_idx = chunks.len() - 2;
        let footer_idx = chunks.len() - 1;
        self.draw_help(frame, chunks[help_idx]);
        self.draw_footer(frame, chunks[footer_idx]);
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut spans = theme::brand_chip("秘 himitsu");
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            self.title.as_str(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_field(&self, frame: &mut Frame<'_>, area: Rect, field: &Field, focused: bool) {
        let mut title = format!(" {} ", field.label);
        if field.required {
            title = format!(" {} * ", field.label);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_style(Style::default().fg(theme::border_label()))
            .border_style(if focused {
                Style::default().fg(theme::accent())
            } else {
                Style::default().fg(theme::muted())
            });
        // Empty + unfocused renders the placeholder hint in muted style.
        // Focused always shows the buffer with a trailing cursor — typing
        // immediately replaces the hint, so we don't show both.
        let para = if !focused && field.value.is_empty() {
            if let Some(ph) = field.placeholder {
                Paragraph::new(Line::from(Span::styled(
                    ph,
                    Style::default().fg(theme::muted()),
                )))
                .block(block)
            } else {
                Paragraph::new(String::new()).block(block)
            }
        } else {
            let mut text = field.value.clone();
            if focused {
                text.push('_');
            }
            Paragraph::new(text).block(block)
        };
        match field.widget {
            Widget::Text => frame.render_widget(para, area),
            Widget::TextArea => frame.render_widget(para.wrap(Wrap { trim: false }), area),
        }
    }

    fn draw_help(&self, frame: &mut Frame<'_>, area: Rect) {
        let help = self.fields[self.focused].help;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {help}"),
                Style::default().fg(theme::muted()),
            ))),
            area,
        );
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = if let Some(msg) = self.status.as_deref() {
            Line::from(Span::styled(
                msg.to_string(),
                Style::default().fg(theme::danger()),
            ))
        } else {
            let footer = Style::default().fg(theme::footer_text());
            let key = Style::default().fg(theme::accent());
            Line::from(vec![
                Span::styled("tab", key),
                Span::styled(" next    ", footer),
                Span::styled("shift-tab", key),
                Span::styled(" prev    ", footer),
                Span::styled("ctrl-s / ctrl-w", key),
                Span::styled(" save    ", footer),
                Span::styled("esc", key),
                Span::styled(" cancel    ", footer),
                Span::styled("ctrl-c", key),
                Span::styled(" quit", footer),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keymap::KeyMap;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typ(form: &mut FormView, km: &KeyMap, s: &str) {
        for c in s.chars() {
            form.on_key(press(KeyCode::Char(c)), km);
        }
    }

    fn two_field_form() -> FormView {
        FormView::new(
            "test",
            vec![
                Field::text("first", "First", "the first").required(),
                Field::text("second", "Second", "the second"),
            ],
        )
    }

    #[test]
    fn typing_into_focused_field_updates_value() {
        let km = KeyMap::default();
        let mut f = two_field_form();
        typ(&mut f, &km, "hello");
        assert_eq!(f.field_value("first"), Some("hello"));
        assert_eq!(f.field_value("second"), Some(""));
    }

    #[test]
    fn tab_moves_focus_forward_with_wrap() {
        let km = KeyMap::default();
        let mut f = two_field_form();
        typ(&mut f, &km, "x");
        f.on_key(press(KeyCode::Tab), &km);
        assert_eq!(f.focused, 1);
        f.on_key(press(KeyCode::Tab), &km);
        assert_eq!(f.focused, 0);
    }

    #[test]
    fn shift_tab_moves_focus_backward_with_wrap() {
        let km = KeyMap::default();
        let mut f = two_field_form();
        f.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), &km);
        assert_eq!(f.focused, 1);
    }

    #[test]
    fn esc_cancels() {
        let km = KeyMap::default();
        let mut f = two_field_form();
        assert_eq!(f.on_key(press(KeyCode::Esc), &km), FormOutcome::Cancel);
    }

    #[test]
    fn ctrl_c_quits() {
        let km = KeyMap::default();
        let mut f = two_field_form();
        assert_eq!(f.on_key(ctrl('c'), &km), FormOutcome::Quit);
    }

    #[test]
    fn submit_blocks_when_required_field_empty() {
        let km = KeyMap::default();
        let mut f = two_field_form();
        assert_eq!(f.on_key(ctrl('s'), &km), FormOutcome::Pending);
        assert_eq!(f.focused, 0);
        assert!(f.status().unwrap().contains("required"));
    }

    #[test]
    fn submit_passes_when_required_filled() {
        let km = KeyMap::default();
        let mut f = two_field_form();
        typ(&mut f, &km, "filled");
        assert_eq!(f.on_key(ctrl('s'), &km), FormOutcome::Submit);
    }

    #[test]
    fn validator_blocks_field_leave() {
        fn rejects_short(v: &str) -> Result<(), String> {
            if v.len() < 3 {
                Err("too short".into())
            } else {
                Ok(())
            }
        }
        let km = KeyMap::default();
        let mut f = FormView::new(
            "test",
            vec![Field::text("x", "X", "").with_validator(rejects_short)],
        );
        typ(&mut f, &km, "ab");
        f.on_key(press(KeyCode::Tab), &km);
        // Validator failed → focus stays put, status holds the message.
        assert_eq!(f.focused, 0);
        assert_eq!(f.status(), Some("too short"));
    }

    #[test]
    fn empty_optional_field_passes_validator() {
        // Validators are skipped for empty optional fields; the user gets
        // to leave a field blank without tripping syntactic checks.
        fn always_fail(_: &str) -> Result<(), String> {
            Err("nope".into())
        }
        let km = KeyMap::default();
        let mut f = FormView::new(
            "test",
            vec![
                Field::text("a", "A", "").required(),
                Field::text("b", "B", "").with_validator(always_fail),
            ],
        );
        typ(&mut f, &km, "ok");
        f.on_key(press(KeyCode::Tab), &km);
        assert_eq!(f.focused, 1);
        assert_eq!(f.on_key(ctrl('s'), &km), FormOutcome::Submit);
    }

    #[test]
    fn text_area_enter_inserts_newline() {
        let km = KeyMap::default();
        let mut f = FormView::new(
            "test",
            vec![Field {
                name: "body",
                label: "Body",
                help: "",
                widget: Widget::TextArea,
                required: false,
                value: String::new(),
                validator: None,
                placeholder: None,
            }],
        );
        typ(&mut f, &km, "line1");
        f.on_key(press(KeyCode::Enter), &km);
        typ(&mut f, &km, "line2");
        assert_eq!(f.field_value("body"), Some("line1\nline2"));
    }

    // ── ProtoForm wiring ───────────────────────────────────────────────

    struct DummyArgs {
        slug: String,
    }

    impl ProtoForm for DummyArgs {
        fn form_title() -> &'static str {
            "dummy"
        }
        fn form_fields() -> Vec<Field> {
            vec![Field::text("slug", "Slug", "test slug").required()]
        }
        fn from_form(form: &FormView) -> Result<Self, String> {
            Ok(Self {
                slug: form.field_value("slug").unwrap_or("").to_string(),
            })
        }
    }

    #[test]
    fn for_proto_constructor_uses_trait_metadata() {
        let f = FormView::for_proto::<DummyArgs>();
        assert_eq!(f.fields().len(), 1);
        assert_eq!(f.fields()[0].label, "Slug");
    }

    // ── Placeholder rendering ──────────────────────────────────────────

    fn render_field(form: &FormView, focused: bool, field_idx: usize) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut terminal = Terminal::new(TestBackend::new(40, 3)).unwrap();
        let area = Rect::new(0, 0, 40, 3);
        terminal
            .draw(|frame| form.draw_field(frame, area, &form.fields[field_idx], focused))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn placeholder_renders_when_empty_and_unfocused() {
        let form = FormView::new(
            "test",
            vec![Field::text("a", "A", "").placeholder("e.g. hint")],
        );
        let rendered = render_field(&form, false, 0);
        assert!(rendered.contains("e.g. hint"));
        assert!(!rendered.contains('_'));
    }

    #[test]
    fn placeholder_hidden_when_focused() {
        let form = FormView::new(
            "test",
            vec![Field::text("a", "A", "").placeholder("e.g. hint")],
        );
        let rendered = render_field(&form, true, 0);
        assert!(!rendered.contains("e.g. hint"));
        assert!(rendered.contains('_'));
    }

    #[test]
    fn placeholder_hidden_when_value_present() {
        let mut form = FormView::new(
            "test",
            vec![Field::text("a", "A", "").placeholder("e.g. hint")],
        );
        form.fields[0].value = "typed".to_string();
        let rendered = render_field(&form, false, 0);
        assert!(!rendered.contains("e.g. hint"));
        assert!(rendered.contains("typed"));
    }

    #[test]
    fn from_form_round_trips_through_proto_form() {
        let km = KeyMap::default();
        let mut f = FormView::for_proto::<DummyArgs>();
        typ(&mut f, &km, "myorg/repo");
        let args = DummyArgs::from_form(&f).unwrap();
        assert_eq!(args.slug, "myorg/repo");
    }
}
