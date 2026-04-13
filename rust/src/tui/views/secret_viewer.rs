//! Secret viewer: shows metadata for a single secret, with opt-in reveal.
//!
//! Opened from the search view (`SearchAction::OpenViewer`). Operations:
//!
//! - `r` — toggle reveal. Decrypts via [`crate::crypto::age`] on demand;
//!   subsequent presses hide the value again.
//! - `y` — copy the (already-revealed or freshly-decrypted) value to the
//!   system clipboard via [`arboard`]. Falls back to a status message if
//!   the clipboard backend is unavailable (e.g. headless CI).
//! - `e` — decrypt the secret, open it in `$EDITOR`, and re-encrypt the
//!   edited plaintext for the current recipients. The TUI suspends its
//!   alternate screen while the editor runs.
//! - `R` — re-encrypt this one secret for the current recipient set via
//!   [`crate::cli::rekey::rekey_store`] (no value change).
//! - `Esc` — emit `SecretViewerAction::Back` so the router pops to the
//!   previous view (search).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::cli::{rekey, Context};
use crate::crypto::{age, secret_value};
use crate::proto::SecretValue;
use crate::remote::store::{self, SecretMeta};

/// Outcome of handling a key — routed by [`crate::tui::app::App`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretViewerAction {
    None,
    Back,
    Quit,
    /// Caller (event loop) should suspend the TUI, open `$EDITOR` on the
    /// carried plaintext, then hand the result back via
    /// [`SecretViewerView::finish_edit`].
    EditValue(String),
    /// The displayed secret was deleted — the router should pop back to
    /// whichever view opened the viewer (refreshed).
    Deleted,
}

#[derive(Debug, Clone)]
enum ValueState {
    Hidden,
    Revealed(String),
}

/// UX mode for the viewer. In [`Mode::ConfirmDelete`], the normal key
/// bindings are suspended and only `y` / `n` / `Esc` are accepted; the
/// underlying view is still rendered, with a confirmation overlay on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    ConfirmDelete,
}

#[derive(Debug, Clone, Copy)]
enum StatusKind {
    Info,
    Error,
}

pub struct SecretViewerView {
    /// Slug label for the store this secret lives in, used only for display.
    store_label: String,
    /// Absolute store path. Needed by the crypto + rekey code paths.
    store_path: PathBuf,
    /// Secret path within the store (e.g. `prod/API_KEY`).
    path: String,
    /// First path segment (e.g. `prod`) — pre-computed for the header.
    env: String,
    meta: SecretMeta,
    value: ValueState,
    status: Option<(String, StatusKind)>,
    mode: Mode,
    /// Context needed to call into `rekey::rekey_store` on `e`.
    ///
    /// Cloned from the app router so the view owns its data.
    ctx: Context,
}

impl SecretViewerView {
    pub fn new(ctx: &Context, store_label: String, store_path: PathBuf, path: String) -> Self {
        let env = path
            .split_once('/')
            .map(|(head, _)| head.to_string())
            .unwrap_or_default();

        // Best-effort metadata read — if it fails we still show the path and
        // let the user try to reveal (which will surface the real error).
        let meta = store::read_secret_meta(&store_path, &path).unwrap_or_default();

        // The viewer inherits the outer context but must operate against the
        // store the result came from, not whatever `ctx.store` happens to be.
        let mut ctx_owned = ctx.clone();
        ctx_owned.store = store_path.clone();

        Self {
            store_label,
            store_path,
            path,
            env,
            meta,
            value: ValueState::Hidden,
            status: None,
            mode: Mode::Normal,
            ctx: ctx_owned,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> SecretViewerAction {
        // Ctrl-C is always a quit, regardless of mode.
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
        ) {
            return SecretViewerAction::Quit;
        }

        // Confirm-delete mode intercepts all keys except Ctrl-C.
        if self.mode == Mode::ConfirmDelete {
            return self.on_key_confirm_delete(key);
        }

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => SecretViewerAction::Back,
            (KeyCode::Char('r'), _) => {
                self.toggle_reveal();
                SecretViewerAction::None
            }
            (KeyCode::Char('y'), _) => {
                self.copy_to_clipboard();
                SecretViewerAction::None
            }
            (KeyCode::Char('R'), _) => {
                self.rekey();
                SecretViewerAction::None
            }
            (KeyCode::Char('e'), _) => self.begin_edit(),
            (KeyCode::Char('d'), _) => {
                self.enter_confirm_delete();
                SecretViewerAction::None
            }
            _ => SecretViewerAction::None,
        }
    }

    /// Decrypt the current value and ask the event loop to run `$EDITOR`.
    ///
    /// Returns [`SecretViewerAction::EditValue`] with the plaintext on
    /// success, or [`SecretViewerAction::None`] with a status message set on
    /// decrypt failure (so the user sees *why* the edit did not happen).
    fn begin_edit(&mut self) -> SecretViewerAction {
        match self.decrypt() {
            Ok(plain) => SecretViewerAction::EditValue(plain),
            Err(e) => {
                self.status = Some((format!("edit failed: {e}"), StatusKind::Error));
                SecretViewerAction::None
            }
        }
    }

    /// Transition into the confirm-delete mode. Pure state change so tests
    /// can exercise confirm/cancel without touching the filesystem.
    fn enter_confirm_delete(&mut self) {
        self.mode = Mode::ConfirmDelete;
        // Clear any stale status line so the overlay is unambiguous.
        self.status = None;
    }

    fn cancel_confirm_delete(&mut self) {
        self.mode = Mode::Normal;
    }

    fn on_key_confirm_delete(&mut self, key: KeyEvent) -> SecretViewerAction {
        match (key.code, key.modifiers) {
            // Only an explicit lowercase 'y' confirms. Any other key —
            // including 'n', Esc, arrows, typos — cancels and returns to
            // the normal view. This is the least surprising behaviour for
            // a destructive default-no prompt.
            (KeyCode::Char('y'), _) => match self.delete_secret() {
                Ok(()) => SecretViewerAction::Deleted,
                Err(e) => {
                    self.status = Some((format!("delete failed: {e}"), StatusKind::Error));
                    self.mode = Mode::Normal;
                    SecretViewerAction::None
                }
            },
            _ => {
                self.cancel_confirm_delete();
                SecretViewerAction::None
            }
        }
    }

    /// Handle the result of an external edit. Called by the event loop
    /// after `$EDITOR` exits. `result` is `Ok(Some(new_plaintext))` on a
    /// real change, `Ok(None)` for "no change / cancelled", and
    /// `Err(msg)` for a terminal failure (spawn error, non-zero exit).
    pub fn finish_edit(&mut self, result: std::result::Result<Option<String>, String>) {
        match result {
            Ok(None) => {
                self.status = Some((
                    "edit cancelled (no changes)".to_string(),
                    StatusKind::Info,
                ));
            }
            Ok(Some(plain)) => match self.persist_edited(&plain) {
                Ok(()) => {
                    // Keep the new value visible so the user sees what they
                    // just committed.
                    self.value = ValueState::Revealed(plain);
                    self.status = Some(("edited".to_string(), StatusKind::Info));
                }
                Err(e) => {
                    self.status = Some((format!("edit failed: {e}"), StatusKind::Error));
                }
            },
            Err(e) => {
                self.status = Some((format!("edit failed: {e}"), StatusKind::Error));
            }
        }
    }

    fn persist_edited(&mut self, plaintext: &str) -> crate::error::Result<()> {
        let recipients =
            age::collect_recipients(&self.store_path, self.ctx.recipients_path.as_deref())?;
        // Preserve existing metadata (totp/url/description/env_key/expires_at)
        // when the user edits only the value. Fall back to a bare SecretValue
        // if the original envelope was a legacy raw payload.
        let existing = self.read_decoded().unwrap_or_default();
        let sv = SecretValue {
            data: plaintext.as_bytes().to_vec(),
            content_type: String::new(),
            annotations: Default::default(),
            totp: existing.totp,
            url: existing.url,
            expires_at: existing.expires_at,
            description: existing.description,
            env_key: existing.env_key,
        };
        let wire = secret_value::encode(&sv);
        let ciphertext = age::encrypt(&wire, &recipients)?;
        store::write_secret(&self.store_path, &self.path, &ciphertext)?;
        if let Ok(meta) = store::read_secret_meta(&self.store_path, &self.path) {
            self.meta = meta;
        }
        Ok(())
    }

    fn delete_secret(&mut self) -> crate::error::Result<()> {
        store::delete_secret(&self.store_path, &self.path)
    }

    // ── Actions ────────────────────────────────────────────────────────

    fn toggle_reveal(&mut self) {
        match &self.value {
            ValueState::Revealed(_) => {
                self.value = ValueState::Hidden;
                self.status = None;
            }
            ValueState::Hidden => match self.decrypt() {
                Ok(plain) => {
                    self.value = ValueState::Revealed(plain);
                    self.status = None;
                }
                Err(e) => {
                    self.status = Some((format!("decrypt failed: {e}"), StatusKind::Error));
                }
            },
        }
    }

    fn copy_to_clipboard(&mut self) {
        let value = match &self.value {
            ValueState::Revealed(v) => v.clone(),
            ValueState::Hidden => match self.decrypt() {
                Ok(v) => v,
                Err(e) => {
                    self.status = Some((format!("decrypt failed: {e}"), StatusKind::Error));
                    return;
                }
            },
        };

        // Graceful no-op: arboard may fail on headless boxes or when no
        // display server is available. Surface as status rather than crashing.
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(value)) {
            Ok(()) => {
                self.status = Some(("copied to clipboard".to_string(), StatusKind::Info));
            }
            Err(e) => {
                self.status = Some((format!("clipboard unavailable: {e}"), StatusKind::Error));
            }
        }
    }

    fn rekey(&mut self) {
        match rekey::rekey_store(&self.ctx, Some(&self.path)) {
            Ok(n) => {
                // Refresh metadata so `lastmodified` reflects the rewrite.
                if let Ok(meta) = store::read_secret_meta(&self.store_path, &self.path) {
                    self.meta = meta;
                }
                // A revealed value is still valid plaintext, but the
                // ciphertext has changed — keep it displayed for convenience.
                self.status = Some((
                    format!("rekeyed {n} secret(s) for current recipients"),
                    StatusKind::Info,
                ));
            }
            Err(e) => {
                self.status = Some((format!("rekey failed: {e}"), StatusKind::Error));
            }
        }
    }

    fn decrypt(&self) -> crate::error::Result<String> {
        let decoded = self.read_decoded()?;
        Ok(String::from_utf8_lossy(&decoded.data).into_owned())
    }

    fn read_decoded(&self) -> crate::error::Result<secret_value::Decoded> {
        let ciphertext = store::read_secret(&self.store_path, &self.path)?;
        let identity = age::read_identity(&self.ctx.key_path())?;
        let plain = age::decrypt(&ciphertext, &identity)?;
        Ok(secret_value::decode(&plain))
    }

    // ── Drawing ────────────────────────────────────────────────────────

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_body(frame, chunks[1]);
        self.draw_footer(frame, chunks[2]);

        if self.mode == Mode::ConfirmDelete {
            self.draw_confirm_delete(frame, area);
        }
    }

    fn draw_confirm_delete(&self, frame: &mut Frame<'_>, area: Rect) {
        // Center a small prompt box over the existing view. The underlying
        // layout has already been rendered, so this acts as an overlay.
        let prompt = format!(" Delete {}/{}? (y/N) ", self.env, self.path);
        let width = (prompt.len() as u16 + 4).min(area.width.saturating_sub(2));
        let height: u16 = 3;
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let rect = Rect {
            x,
            y,
            width,
            height,
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" confirm delete ")
            .style(Style::default().fg(Color::Red));
        let line = Line::from(vec![Span::styled(
            prompt,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )]);
        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(Paragraph::new(line).block(block), rect);
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
            Span::styled("secret", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(&self.store_label, Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(header), area);
    }

    fn draw_body(&self, frame: &mut Frame<'_>, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(9), Constraint::Min(3)])
            .split(area);

        self.draw_meta(frame, rows[0]);
        self.draw_value(frame, rows[1]);
    }

    fn draw_meta(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title(" metadata ");

        let created = self.meta.created_at.as_deref().unwrap_or("-");
        let modified = self.meta.lastmodified.as_deref().unwrap_or("-");
        let recipients = if self.meta.recipients.is_empty() {
            "-".to_string()
        } else {
            self.meta.recipients.join(", ")
        };

        let lines = vec![
            labeled_line("path        ", &self.path),
            labeled_line("env         ", &self.env),
            labeled_line("created_at  ", created),
            labeled_line("lastmodified", modified),
            labeled_line("recipients  ", &recipients),
        ];
        let p = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
        frame.render_widget(p, area);
    }

    fn draw_value(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title(" value ");

        let content: Line = match &self.value {
            ValueState::Hidden => Line::from(Span::styled(
                "  ●●●●●●●●  (press r to reveal)",
                Style::default().fg(Color::DarkGray),
            )),
            ValueState::Revealed(v) => Line::from(Span::styled(
                format!("  {v}"),
                Style::default().fg(Color::Yellow),
            )),
        };
        frame.render_widget(
            Paragraph::new(content).block(block).wrap(Wrap { trim: false }),
            area,
        );
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = if let Some((msg, kind)) = &self.status {
            let color = match kind {
                StatusKind::Info => Color::Green,
                StatusKind::Error => Color::Red,
            };
            Line::from(Span::styled(msg.clone(), Style::default().fg(color)))
        } else {
            Line::from(vec![
                Span::styled("r", Style::default().fg(Color::Cyan)),
                Span::raw(" reveal  "),
                Span::styled("y", Style::default().fg(Color::Cyan)),
                Span::raw(" copy  "),
                Span::styled("e", Style::default().fg(Color::Cyan)),
                Span::raw(" edit  "),
                Span::styled("R", Style::default().fg(Color::Cyan)),
                Span::raw(" rekey  "),
                Span::styled("d", Style::default().fg(Color::Cyan)),
                Span::raw(" delete  "),
                Span::styled("esc", Style::default().fg(Color::Cyan)),
                Span::raw(" back  "),
                Span::styled("ctrl-c", Style::default().fg(Color::Cyan)),
                Span::raw(" quit"),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }
}

fn labeled_line<'a>(label: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {label} "), Style::default().fg(Color::DarkGray)),
        Span::raw(value.to_string()),
    ])
}

/// Kept for potential future use by other views that want to resolve a
/// label back to a real store path.
#[allow(dead_code)]
pub fn store_path_for(label: &str, ctx: &Context) -> Option<PathBuf> {
    let candidate = ctx.stores_dir().join(label);
    if candidate.exists() {
        return Some(candidate);
    }
    let path = Path::new(label);
    if path.exists() {
        return Some(path.to_path_buf());
    }
    None
}

// ── Help overlay integration (US-012) ─────────────────────────────────
//
// In its own impl block so parallel branches adding new bindings can extend
// `help_entries` without colliding with the main impl.
impl SecretViewerView {
    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("r", "reveal / hide value"),
            ("y", "copy value to clipboard"),
            ("e", "edit value in $EDITOR"),
            ("R", "rekey for current recipients"),
            ("d", "delete secret (with confirm)"),
            ("?", "toggle this help"),
            ("esc", "back"),
            ("ctrl-c", "quit"),
        ]
    }

    pub fn help_title() -> &'static str {
        "secret · keys"
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn shift(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Seed a store with one encrypted secret and return (tempdir, ctx, path).
    ///
    /// Uses real age keys + encryption so reveal/rekey paths run end-to-end.
    fn seeded_store_with_secret() -> (TempDir, Context, String) {
        use ::age::x25519::Identity;
        use secrecy::ExposeSecret;
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data");
        let state_dir = dir.path().join("state");
        let store = state_dir.join("stores/test/repo");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/secrets")).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/recipients")).unwrap();

        let identity = Identity::generate();
        let pubkey = identity.to_public().to_string();
        let secret = identity.to_string().expose_secret().to_string();
        std::fs::write(data_dir.join("key"), &secret).unwrap();
        std::fs::write(
            store.join(".himitsu/recipients/me.pub"),
            format!("{pubkey}\n"),
        )
        .unwrap();

        // Encrypt and write a secret through the real store writer.
        let recipients = age::collect_recipients(&store, None).unwrap();
        let ct = age::encrypt(b"s3cret", &recipients).unwrap();
        store::write_secret(&store, "prod/API_KEY", &ct).unwrap();

        let ctx = Context {
            data_dir,
            state_dir,
            store: store.clone(),
            recipients_path: None,
        };
        (dir, ctx, "prod/API_KEY".to_string())
    }

    #[test]
    fn metadata_is_loaded_on_construction() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        assert_eq!(view.env, "prod");
        assert!(view.meta.created_at.is_some());
        assert!(view.meta.lastmodified.is_some());
        assert_eq!(view.meta.recipients.len(), 1);
    }

    #[test]
    fn esc_returns_back_action() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        assert_eq!(view.on_key(press(KeyCode::Esc)), SecretViewerAction::Back);
    }

    #[test]
    fn ctrl_c_returns_quit() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        assert_eq!(view.on_key(ctrl('c')), SecretViewerAction::Quit);
    }

    #[test]
    fn r_reveals_then_hides_value() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        assert!(matches!(view.value, ValueState::Hidden));
        view.on_key(press(KeyCode::Char('r')));
        match &view.value {
            ValueState::Revealed(v) => assert_eq!(v, "s3cret"),
            _ => panic!("expected Revealed"),
        }
        view.on_key(press(KeyCode::Char('r')));
        assert!(matches!(view.value, ValueState::Hidden));
    }

    #[test]
    fn y_copies_without_revealing_display() {
        // On headless CI arboard may fail — assert it doesn't crash and
        // surfaces *some* status message (info on success, error on miss).
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        view.on_key(press(KeyCode::Char('y')));
        assert!(view.status.is_some(), "y should set a status message");
        // Value state is an implementation detail — either hidden or kept as
        // an in-memory cache is fine. We only require the UI survived.
    }

    #[test]
    fn d_enters_confirm_delete_mode_without_deleting() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        let action = view.on_key(press(KeyCode::Char('d')));
        assert_eq!(action, SecretViewerAction::None);
        assert_eq!(view.mode, Mode::ConfirmDelete);
        // Secret file must still exist — 'd' alone must not delete.
        assert!(store::read_secret_meta(&ctx.store, &path).is_ok());
    }

    #[test]
    fn y_in_confirm_mode_deletes_and_returns_deleted() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('d')));
        assert_eq!(view.mode, Mode::ConfirmDelete);
        let action = view.on_key(press(KeyCode::Char('y')));
        assert_eq!(action, SecretViewerAction::Deleted);
        // Underlying store should no longer contain the secret.
        assert!(store::list_secrets(&ctx.store, None)
            .unwrap()
            .iter()
            .all(|p| p != &path));
    }

    #[test]
    fn n_in_confirm_mode_cancels_without_deleting() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('d')));
        let action = view.on_key(press(KeyCode::Char('n')));
        assert_eq!(action, SecretViewerAction::None);
        assert_eq!(view.mode, Mode::Normal);
        assert!(store::read_secret_meta(&ctx.store, &path).is_ok());
    }

    #[test]
    fn esc_in_confirm_mode_cancels_without_deleting() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('d')));
        let action = view.on_key(press(KeyCode::Esc));
        assert_eq!(action, SecretViewerAction::None);
        assert_eq!(view.mode, Mode::Normal);
        assert!(store::read_secret_meta(&ctx.store, &path).is_ok());
    }

    #[test]
    fn other_keys_in_confirm_mode_cancel() {
        // Documented choice: any non-'y' key cancels the delete. Safer for
        // a destructive default-no prompt than staying in confirm mode.
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('d')));
        view.on_key(press(KeyCode::Char('q')));
        assert_eq!(view.mode, Mode::Normal);
        assert!(store::read_secret_meta(&ctx.store, &path).is_ok());
    }

    #[test]
    fn ctrl_c_in_confirm_mode_still_quits() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        view.on_key(press(KeyCode::Char('d')));
        assert_eq!(view.on_key(ctrl('c')), SecretViewerAction::Quit);
    }

    #[test]
    fn shift_r_rekeys_and_updates_status() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let before = store::read_secret_meta(&ctx.store, &path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(shift('R'));
        match &view.status {
            Some((msg, StatusKind::Info)) => assert!(msg.contains("rekeyed")),
            other => panic!("expected info status, got {other:?}"),
        }
        let after = store::read_secret_meta(&ctx.store, &path).unwrap();
        assert_ne!(before.lastmodified, after.lastmodified);
    }

    #[test]
    fn e_emits_edit_value_action_with_plaintext() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        match view.on_key(press(KeyCode::Char('e'))) {
            SecretViewerAction::EditValue(plain) => assert_eq!(plain, "s3cret"),
            other => panic!("expected EditValue, got {other:?}"),
        }
    }

    #[test]
    fn finish_edit_with_new_value_persists_and_reencrypts() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.finish_edit(Ok(Some("rotated".to_string())));
        match &view.status {
            Some((msg, StatusKind::Info)) => assert_eq!(msg, "edited"),
            other => panic!("expected info 'edited', got {other:?}"),
        }
        // Round-trip: decrypt again and confirm the new ciphertext.
        let plain = view.decrypt().unwrap();
        assert_eq!(plain, "rotated");
    }

    #[test]
    fn finish_edit_with_no_change_reports_cancelled() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        view.finish_edit(Ok(None));
        match &view.status {
            Some((msg, StatusKind::Info)) => {
                assert!(msg.contains("cancelled"), "got {msg}");
            }
            other => panic!("expected info cancelled, got {other:?}"),
        }
    }

    #[test]
    fn footer_hint_lists_edit_and_rekey_bindings() {
        // The footer is drawn via ratatui's Line; we can cheaply assert by
        // rendering the viewer into a TestBackend buffer and searching the
        // text content for the expected hint tokens.
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
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
        assert!(rendered.contains("e edit"), "missing 'e edit' hint: {rendered}");
        assert!(rendered.contains("R rekey"), "missing 'R rekey' hint: {rendered}");
    }
}
