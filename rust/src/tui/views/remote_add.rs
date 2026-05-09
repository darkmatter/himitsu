//! "Add remote" form — first consumer of the protobuf-driven [`FormView`].
//!
//! The form's shape (fields, labels, validators) lives in the [`ProtoForm`]
//! impl below; this view just hosts a [`FormView`] and translates its
//! outcome into a [`RemoteAddAction`] the app router knows how to handle.
//!
//! On submit we call [`crate::cli::remote::add`] — the same function the
//! CLI's `himitsu remote add` invokes — so the TUI and CLI cannot drift.

use crossterm::event::KeyEvent;
use ratatui::Frame;

use super::standard_canvas;
use crate::cli::Context;
use crate::proto::commands::RemoteAddArgs;
use crate::tui::forms::{Field, FormOutcome, FormView, ProtoForm};
use crate::tui::keymap::KeyMap;

/// Outcome of handling a key — routed by [`crate::tui::app::App`].
#[derive(Debug, Clone)]
pub enum RemoteAddAction {
    None,
    /// User cancelled (Esc).
    Cancel,
    /// Ctrl-C quit.
    Quit,
    /// Remote was added successfully. Carries the resolved slug for the
    /// confirmation toast.
    Created(String),
    /// Submission failed; carries the error message for the toast. The form
    /// is closed regardless because the underlying error (network, auth,
    /// path collision) typically isn't fixable by re-editing the inputs.
    Failed(String),
}

pub struct RemoteAddView {
    form: FormView,
    #[allow(dead_code)]
    ctx: Context,
}

impl RemoteAddView {
    pub fn new(ctx: &Context) -> Self {
        Self {
            form: FormView::for_proto::<RemoteAddArgs>(),
            ctx: ctx.clone(),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> RemoteAddAction {
        match self.form.on_key(key, keymap) {
            FormOutcome::Pending => RemoteAddAction::None,
            FormOutcome::Cancel => RemoteAddAction::Cancel,
            FormOutcome::Quit => RemoteAddAction::Quit,
            FormOutcome::Submit => self.submit(),
        }
    }

    fn submit(&mut self) -> RemoteAddAction {
        let args = match RemoteAddArgs::from_form(&self.form) {
            Ok(a) => a,
            Err(msg) => {
                self.form.set_status(msg.clone());
                return RemoteAddAction::None;
            }
        };

        let url = if args.url.trim().is_empty() {
            None
        } else {
            Some(args.url.as_str())
        };

        match crate::cli::remote::add(&args.slug, url) {
            Ok(outcome) => RemoteAddAction::Created(outcome.slug),
            Err(e) => RemoteAddAction::Failed(format!("{e}")),
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
        "add remote · keys"
    }
}

// ── ProtoForm: the schema lives next to the proto type ────────────────

impl ProtoForm for RemoteAddArgs {
    fn form_title() -> &'static str {
        "add remote"
    }

    fn form_fields() -> Vec<Field> {
        vec![
            Field::text(
                "slug",
                "Slug",
                "org/repo (e.g. acme/secrets) or a full git URL",
            )
            .required()
            .with_validator(validate_slug_or_url)
            .placeholder("myorg/myrepo"),
            Field::text(
                "url",
                "URL",
                "optional override; defaults to git@github.com:<slug>.git",
            )
            .placeholder("git@github.com:myorg/myrepo.git"),
        ]
    }

    fn from_form(form: &FormView) -> Result<Self, String> {
        Ok(RemoteAddArgs {
            slug: form.field_value("slug").unwrap_or("").trim().to_string(),
            url: form.field_value("url").unwrap_or("").trim().to_string(),
        })
    }
}

/// Accept either a bare `org/repo` slug or a recognisable git URL. Defers
/// the heavyweight `validate_remote_slug` check to submit time so the user
/// isn't blocked from leaving the field while still typing.
fn validate_slug_or_url(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if crate::cli::init::parse_remote_slug(trimmed).is_some() {
        return Ok(());
    }
    crate::config::validate_remote_slug(trimmed)
        .map(|_| ())
        .map_err(|e| format!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
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

    fn typ(view: &mut RemoteAddView, km: &KeyMap, s: &str) {
        for c in s.chars() {
            view.on_key(press(KeyCode::Char(c)), km);
        }
    }

    #[test]
    fn esc_cancels_the_form() {
        let km = KeyMap::default();
        let mut view = RemoteAddView::new(&empty_ctx());
        assert!(matches!(
            view.on_key(press(KeyCode::Esc), &km),
            RemoteAddAction::Cancel
        ));
    }

    #[test]
    fn ctrl_c_quits() {
        let km = KeyMap::default();
        let mut view = RemoteAddView::new(&empty_ctx());
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(view.on_key(key, &km), RemoteAddAction::Quit));
    }

    #[test]
    fn slug_validator_accepts_org_slash_repo() {
        assert!(validate_slug_or_url("acme/secrets").is_ok());
    }

    #[test]
    fn slug_validator_accepts_ssh_url() {
        assert!(validate_slug_or_url("git@github.com:acme/secrets.git").is_ok());
    }

    #[test]
    fn slug_validator_accepts_https_url() {
        assert!(validate_slug_or_url("https://github.com/acme/secrets.git").is_ok());
    }

    #[test]
    fn slug_validator_rejects_garbage() {
        assert!(validate_slug_or_url("not a slug").is_err());
    }

    #[test]
    fn slug_validator_allows_empty_for_in_progress_typing() {
        // The form widget skips validators on empty fields; this guarantee
        // means the validator itself doesn't have to be defensive about it.
        assert!(validate_slug_or_url("").is_ok());
        assert!(validate_slug_or_url("   ").is_ok());
    }

    #[test]
    fn from_form_round_trips_proto_args() {
        let km = KeyMap::default();
        let mut view = RemoteAddView::new(&empty_ctx());
        typ(&mut view, &km, "acme/secrets");
        let args = RemoteAddArgs::from_form(&view.form).unwrap();
        assert_eq!(args.slug, "acme/secrets");
        assert_eq!(args.url, "");
    }
}
