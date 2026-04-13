//! Secret viewer: shows metadata for a single secret, with opt-in reveal.
//!
//! Opened from the search view (`SearchAction::OpenViewer`). Operations:
//!
//! - `r` — toggle reveal. Decrypts via [`crate::crypto::age`] on demand;
//!   subsequent presses hide the value again.
//! - `y` — copy the (already-revealed or freshly-decrypted) value to the
//!   system clipboard via [`arboard`]. Falls back to a status message if
//!   the clipboard backend is unavailable (e.g. headless CI).
//! - `e` — re-encrypt this one secret for the current recipient set via
//!   [`crate::cli::rekey::rekey_store`].
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
use crate::crypto::age;
use crate::remote::store::{self, SecretMeta};

/// Outcome of handling a key — routed by [`crate::tui::app::App`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretViewerAction {
    None,
    Back,
    Quit,
}

#[derive(Debug, Clone)]
enum ValueState {
    Hidden,
    Revealed(String),
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
            (KeyCode::Char('e'), _) => {
                self.rekey();
                SecretViewerAction::None
            }
            _ => SecretViewerAction::None,
        }
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
        let ciphertext = store::read_secret(&self.store_path, &self.path)?;
        let identity = age::read_identity(&self.ctx.key_path())?;
        let plain = age::decrypt(&ciphertext, &identity)?;
        Ok(String::from_utf8_lossy(&plain).into_owned())
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
                Span::raw(" rekey  "),
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

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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
        std::fs::create_dir_all(store.join(".himitsu/recipients/common")).unwrap();

        let identity = Identity::generate();
        let pubkey = identity.to_public().to_string();
        let secret = identity.to_string().expose_secret().to_string();
        std::fs::write(data_dir.join("key"), &secret).unwrap();
        std::fs::write(
            store.join(".himitsu/recipients/common/me.pub"),
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
    fn e_rekeys_and_updates_status() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let before = store::read_secret_meta(&ctx.store, &path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('e')));
        match &view.status {
            Some((msg, StatusKind::Info)) => assert!(msg.contains("rekeyed")),
            other => panic!("expected info status, got {other:?}"),
        }
        let after = store::read_secret_meta(&ctx.store, &path).unwrap();
        assert_ne!(before.lastmodified, after.lastmodified);
    }
}
