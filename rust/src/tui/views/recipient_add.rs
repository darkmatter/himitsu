//! "Add recipient" form — an in-TUI affordance for `himitsu recipient add`.
//!
//! Modeled on [`super::remote_add::RemoteAddView`] but built on the non-proto
//! [`FormView::new`] path since there is no `RecipientAddArgs` proto. On submit
//! it calls [`crate::cli::recipient::add_recipient`] — the same logic the CLI's
//! `himitsu recipient add <name> --age-key <key>` invokes — so the TUI and CLI
//! cannot drift.

use crossterm::event::KeyEvent;
use ratatui::Frame;

use super::standard_canvas;
use crate::cli::Context;
use crate::tui::forms::{Field, FormOutcome, FormView};
use crate::tui::keymap::KeyMap;

/// Outcome of handling a key — routed by [`crate::tui::app::App`].
#[derive(Debug, Clone)]
pub enum RecipientAddAction {
    None,
    /// User cancelled (Esc).
    Cancel,
    /// Ctrl-C quit.
    Quit,
    /// Recipient was added successfully. Carries the name for the toast.
    Created(String),
    /// Submission failed; carries the error message for the toast.
    Failed(String),
}

pub struct RecipientAddView {
    form: FormView,
    ctx: Context,
}

impl RecipientAddView {
    pub fn new(ctx: &Context) -> Self {
        Self {
            form: FormView::new("add recipient", recipient_fields()),
            ctx: ctx.clone(),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> RecipientAddAction {
        match self.form.on_key(key, keymap) {
            FormOutcome::Pending => RecipientAddAction::None,
            FormOutcome::Cancel => RecipientAddAction::Cancel,
            FormOutcome::Quit => RecipientAddAction::Quit,
            FormOutcome::Submit => self.submit(),
        }
    }

    fn submit(&mut self) -> RecipientAddAction {
        let name = self
            .form
            .field_value("name")
            .unwrap_or("")
            .trim()
            .to_string();
        let age_key = self
            .form
            .field_value("age-key")
            .unwrap_or("")
            .trim()
            .to_string();
        let description = self
            .form
            .field_value("description")
            .unwrap_or("")
            .trim()
            .to_string();
        let description = if description.is_empty() {
            None
        } else {
            Some(description)
        };

        match crate::cli::recipient::add_recipient(&self.ctx, &name, &age_key, description) {
            Ok(()) => RecipientAddAction::Created(name),
            Err(e) => RecipientAddAction::Failed(format!("{e}")),
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = standard_canvas(frame.area());
        self.form.draw(frame, area);
    }

    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("tab / enter", "next field (wraps)"),
            ("shift-tab", "previous field (wraps)"),
            ("ctrl-s / ctrl-w", "submit"),
            ("esc", "cancel"),
            ("ctrl-c", "quit"),
            ("?", "toggle this help"),
        ]
    }

    pub fn help_title() -> &'static str {
        "add recipient · keys"
    }
}

fn recipient_fields() -> Vec<Field> {
    vec![
        Field::text(
            "name",
            "Name",
            "recipient name; may use / for a path hierarchy (e.g. ops/alice)",
        )
        .required()
        .with_validator(validate_name_field)
        .placeholder("ops/alice"),
        Field::text(
            "age-key",
            "Age key",
            "the recipient's age public key (age1...)",
        )
        .required()
        .with_validator(validate_age_key_field)
        .placeholder("age1xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
        Field::text(
            "description",
            "Description",
            "optional human-readable note stored beside the key",
        ),
    ]
}

/// Defer to the shared recipient-name grammar so the TUI and CLI agree.
/// Empty is accepted here so the form widget doesn't block in-progress typing;
/// the `required` flag enforces presence at submit time.
fn validate_name_field(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    crate::cli::recipient::validate_recipient_name(trimmed)
}

/// Accept a parseable age recipient key. Empty passes so the user can leave
/// the field mid-typing; `required` enforces presence on submit.
fn validate_age_key_field(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    crate::crypto::age::parse_recipient(trimmed)
        .map(|_| ())
        .map_err(|e| format!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::path::PathBuf;

    const AGE_KEY: &str = "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";

    fn empty_ctx() -> Context {
        Context {
            data_dir: PathBuf::new(),
            state_dir: PathBuf::new(),
            store: PathBuf::new(),
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
            project_root: None,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typ(view: &mut RecipientAddView, km: &KeyMap, s: &str) {
        for c in s.chars() {
            view.on_key(press(KeyCode::Char(c)), km);
        }
    }

    #[test]
    fn esc_cancels_the_form() {
        let km = KeyMap::default();
        let mut view = RecipientAddView::new(&empty_ctx());
        assert!(matches!(
            view.on_key(press(KeyCode::Esc), &km),
            RecipientAddAction::Cancel
        ));
    }

    #[test]
    fn ctrl_c_quits() {
        let km = KeyMap::default();
        let mut view = RecipientAddView::new(&empty_ctx());
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(view.on_key(key, &km), RecipientAddAction::Quit));
    }

    #[test]
    fn name_validator_accepts_path_based_name() {
        assert!(validate_name_field("ops/alice").is_ok());
    }

    #[test]
    fn name_validator_rejects_traversal_and_allows_empty() {
        assert!(validate_name_field("../x").is_err());
        assert!(validate_name_field("").is_ok());
        assert!(validate_name_field("   ").is_ok());
    }

    #[test]
    fn age_key_validator_accepts_valid_key() {
        assert!(validate_age_key_field(AGE_KEY).is_ok());
    }

    #[test]
    fn age_key_validator_rejects_garbage() {
        assert!(validate_age_key_field("not-a-key").is_err());
        // Empty is allowed for in-progress typing.
        assert!(validate_age_key_field("").is_ok());
    }

    #[test]
    fn form_reads_typed_values() {
        let km = KeyMap::default();
        let mut view = RecipientAddView::new(&empty_ctx());
        typ(&mut view, &km, "ops/alice");
        assert_eq!(view.form.field_value("name"), Some("ops/alice"));
    }
}
