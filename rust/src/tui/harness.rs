//! End-to-end integration-test harness for the TUI `App`.
//!
//! This module is `#[cfg(test)]`-only. It wraps [`crate::tui::app::App`]
//! together with a [`ratatui::backend::TestBackend`]-backed `Terminal` so
//! tests can drive the full router with real `crossterm::event::KeyEvent`s
//! and then assert against the rendered buffer.
//!
//! The goal is to exercise the same code path that the live event loop uses
//! — `App::on_key` → `App::draw` — without touching any real TTY. Unit tests
//! on individual views cover the per-view key handling in detail; this
//! harness covers the **router** (View transitions, context plumbing, status
//! messages surfaced through the search view footer) and gives future tests
//! a single place to simulate multi-step user flows.
//!
//! ## Usage
//!
//! ```ignore
//! let fx = Fixture::new();
//! let mut h = TuiHarness::new(&fx.ctx);
//! h.press_seq(&[KeyCode::Char('D'), KeyCode::Char('A'), KeyCode::Char('T')]);
//! h.press(KeyCode::Enter);
//! assert!(h.contains("DATABASE_URL"));
//! ```

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use crate::cli::Context;
use crate::tui::app::App;

/// Drives an [`App`] against a [`TestBackend`] terminal.
///
/// Every `press*` call runs one tick of the draw loop so the `TestBackend`
/// buffer always reflects the post-key-event state. The default terminal is
/// 120×30 — wide enough for the search view's multi-column layout but small
/// enough to keep test output readable when a buffer dump is needed.
pub struct TuiHarness {
    pub app: App,
    terminal: Terminal<TestBackend>,
}

impl TuiHarness {
    /// Build a harness around a fresh `App`, rendering the initial frame so
    /// the buffer is immediately readable.
    pub fn new(ctx: &Context) -> Self {
        Self::with_size(ctx, 120, 30)
    }

    pub fn with_size(ctx: &Context, width: u16, height: u16) -> Self {
        let backend = TestBackend::new(width, height);
        let terminal = Terminal::new(backend).expect("TestBackend terminal construction");
        let mut h = Self {
            app: App::new(ctx),
            terminal,
        };
        h.tick();
        h
    }

    /// Feed a bare `KeyCode` (no modifiers) and redraw.
    pub fn press(&mut self, code: KeyCode) {
        self.press_event(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// Feed a sequence of bare `KeyCode`s, redrawing after every key so the
    /// intermediate buffer states are reachable for assertions if needed.
    pub fn press_seq(&mut self, codes: &[KeyCode]) {
        for code in codes {
            self.press(*code);
        }
    }

    /// Feed `KeyCode::Char(ch) + CONTROL` — the common chord form used by
    /// all the TUI bindings (Ctrl+N, Ctrl+S, Ctrl+W, …).
    pub fn press_ctrl(&mut self, ch: char) {
        self.press_event(KeyEvent::new(
            KeyCode::Char(ch),
            KeyModifiers::CONTROL,
        ));
    }

    /// Type a UTF-8 string as a sequence of `KeyCode::Char` presses.
    pub fn type_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.press(KeyCode::Char(ch));
        }
    }

    /// Feed a pre-built `KeyEvent` and run one draw tick.
    pub fn press_event(&mut self, key: KeyEvent) {
        // We ignore AppIntent here: none of the flows exercised by the
        // integration tests trip the editor-suspension path, and tests that
        // *do* need it should assert the intent themselves via `App::on_key`
        // directly before falling back into the harness.
        let _ = self.app.on_key(key);
        self.tick();
    }

    fn tick(&mut self) {
        self.terminal
            .draw(|frame| self.app.draw(frame))
            .expect("draw tick");
    }

    /// Clone the current `TestBackend` buffer. Cloned rather than borrowed so
    /// callers can hold it across further `press` calls without fighting the
    /// borrow checker.
    pub fn buffer(&self) -> Buffer {
        self.terminal.backend().buffer().clone()
    }

    /// Render the current buffer to a newline-delimited string for substring
    /// assertions and human-readable error output.
    pub fn rendered(&self) -> String {
        let buf = self.terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// Does the current rendered buffer contain `needle`? Stripping cells
    /// row by row means the substring must appear contiguously on a single
    /// line, which matches how a user would read the screen.
    pub fn contains(&self, needle: &str) -> bool {
        let buf = self.terminal.backend().buffer();
        for y in 0..buf.area.height {
            let mut row = String::new();
            for x in 0..buf.area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            if row.contains(needle) {
                return true;
            }
        }
        false
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use tempfile::TempDir;

    use ::age::x25519::Identity;
    use secrecy::ExposeSecret;

    use crate::cli::Context;
    use crate::crypto::{age as hage, secret_value};
    use crate::proto::SecretValue;
    use crate::remote::store;

    /// Seeded filesystem layout used by every integration test below.
    ///
    /// ```text
    /// <tmp>/data/key                 (age identity)
    /// <tmp>/state/stores/acme/alpha/.himitsu/recipients/me.pub
    /// <tmp>/state/stores/acme/alpha/.himitsu/secrets/prod/API_KEY.yaml
    /// <tmp>/state/stores/acme/alpha/.himitsu/secrets/prod/DATABASE_URL.yaml
    /// <tmp>/state/stores/acme/beta/.himitsu/recipients/me.pub
    /// <tmp>/state/stores/acme/beta/.himitsu/secrets/prod/BETA_ONLY.yaml
    /// ```
    ///
    /// Both stores encrypt to the same identity so the viewer can decrypt
    /// either one without re-keying. `ctx.store` starts on `alpha`.
    struct Fixture {
        _tmp: TempDir,
        pub ctx: Context,
        pub alpha_path: PathBuf,
        pub beta_path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = TempDir::new().expect("tempdir");
            let data_dir = tmp.path().join("data");
            let state_dir = tmp.path().join("state");
            let stores = state_dir.join("stores");
            let alpha = stores.join("acme/alpha");
            let beta = stores.join("acme/beta");

            std::fs::create_dir_all(&data_dir).unwrap();
            std::fs::create_dir_all(alpha.join(".himitsu/recipients")).unwrap();
            std::fs::create_dir_all(alpha.join(".himitsu/secrets")).unwrap();
            std::fs::create_dir_all(beta.join(".himitsu/recipients")).unwrap();
            std::fs::create_dir_all(beta.join(".himitsu/secrets")).unwrap();

            // One age identity, reused across both stores so either can be
            // decrypted from the same `ctx.data_dir/key`.
            let identity = Identity::generate();
            let pubkey = identity.to_public().to_string();
            let secret_key = identity.to_string().expose_secret().to_string();
            std::fs::write(data_dir.join("key"), &secret_key).unwrap();
            std::fs::write(
                alpha.join(".himitsu/recipients/me.pub"),
                format!("{pubkey}\n"),
            )
            .unwrap();
            std::fs::write(
                beta.join(".himitsu/recipients/me.pub"),
                format!("{pubkey}\n"),
            )
            .unwrap();

            // Seed alpha with two real encrypted secrets.
            let recipients = hage::collect_recipients(&alpha, None).unwrap();
            write_encrypted_secret(&alpha, "prod/API_KEY", b"alpha-api", &recipients);
            write_encrypted_secret(
                &alpha,
                "prod/DATABASE_URL",
                b"postgres://alpha-db",
                &recipients,
            );

            // Seed beta with a unique secret so we can detect a store switch.
            let beta_recipients = hage::collect_recipients(&beta, None).unwrap();
            write_encrypted_secret(&beta, "prod/BETA_ONLY", b"beta-value", &beta_recipients);

            let ctx = Context {
                data_dir,
                state_dir,
                store: alpha.clone(),
                recipients_path: None,
            };

            Self {
                _tmp: tmp,
                ctx,
                alpha_path: alpha,
                beta_path: beta,
            }
        }
    }

    fn write_encrypted_secret(
        store_path: &std::path::Path,
        path: &str,
        data: &[u8],
        recipients: &[::age::x25519::Recipient],
    ) {
        let sv = SecretValue {
            data: data.to_vec(),
            content_type: String::new(),
            annotations: Default::default(),
            totp: String::new(),
            url: String::new(),
            expires_at: None,
            description: String::new(),
            env_key: String::new(),
        };
        let wire = secret_value::encode(&sv);
        let ct = hage::encrypt(&wire, recipients).unwrap();
        store::write_secret(store_path, path, &ct).unwrap();
    }

    // ── Flow 1: search → filter → Enter → viewer ──────────────────────

    #[test]
    fn search_filter_enter_opens_secret_viewer_with_decrypted_value() {
        let fx = Fixture::new();
        let mut h = TuiHarness::new(&fx.ctx);

        // Initial search view should be showing both alpha secrets.
        assert_eq!(h.app.current_view(), "search");
        assert!(
            h.contains("API_KEY") && h.contains("DATABASE_URL"),
            "seeded secrets missing from initial search view:\n{}",
            h.rendered()
        );

        // Type "DAT" to narrow down to DATABASE_URL, then open it.
        h.type_str("DAT");
        assert!(h.contains("DATABASE_URL"));
        assert!(
            !h.contains("API_KEY"),
            "filter 'DAT' should hide API_KEY:\n{}",
            h.rendered()
        );

        h.press(KeyCode::Enter);
        assert_eq!(h.app.current_view(), "secret_viewer");

        // Viewer defaults to Hidden — press 'r' to reveal the decrypted value.
        assert!(
            h.contains("press r to reveal"),
            "expected hidden placeholder:\n{}",
            h.rendered()
        );
        h.press(KeyCode::Char('r'));
        assert!(
            h.contains("postgres://alpha-db"),
            "decrypted value missing after reveal:\n{}",
            h.rendered()
        );
    }

    // ── Flow 2: Ctrl+N → fill form → Ctrl+W → search refresh ─────────

    #[test]
    fn ctrl_n_fill_and_save_returns_to_search_with_new_secret_listed() {
        let fx = Fixture::new();
        let mut h = TuiHarness::new(&fx.ctx);

        h.press_ctrl('n');
        assert_eq!(h.app.current_view(), "new_secret");
        assert!(
            h.contains("new secret"),
            "new secret header missing:\n{}",
            h.rendered()
        );

        // Path field is focused first; tab advances to the value field.
        h.type_str("prod/NEW_KEY");
        h.press(KeyCode::Tab);
        h.type_str("fresh-value");

        // Ctrl+W is the tmux-safe save chord. Successful submit bounces the
        // router back to the search view with a "created …" status line.
        h.press_ctrl('w');
        assert_eq!(h.app.current_view(), "search");
        assert!(
            h.contains("created prod/NEW_KEY"),
            "status line missing after save:\n{}",
            h.rendered()
        );
        assert!(
            h.contains("NEW_KEY"),
            "new secret not rendered in search results:\n{}",
            h.rendered()
        );
    }

    // ── Flow 3: Ctrl+S → Down → Enter → store switch ─────────────────

    #[test]
    fn ctrl_s_picker_down_enter_switches_active_store() {
        let fx = Fixture::new();
        let mut h = TuiHarness::new(&fx.ctx);
        assert_eq!(h.app.active_store(), fx.alpha_path.as_path());

        // Open the store picker. The picker lists alpha (index 0) and beta
        // (index 1) alphabetically, with the cursor starting on the current
        // store (alpha).
        h.press_ctrl('s');
        assert!(
            h.contains("acme/beta"),
            "store picker should list beta:\n{}",
            h.rendered()
        );

        // Down → Enter picks beta.
        h.press(KeyCode::Down);
        h.press(KeyCode::Enter);

        // Router updated ctx.store and rebuilt the search view on beta.
        assert_eq!(h.app.current_view(), "search");
        assert_eq!(
            h.app.active_store(),
            fx.beta_path.as_path(),
            "active store should have switched to beta"
        );
        // Beta's unique secret is now visible (it was listed before too
        // because search aggregates all stores, but the active store is
        // what matters for Ctrl+N / decrypt routing).
        assert!(
            h.contains("BETA_ONLY"),
            "beta secret missing after store switch:\n{}",
            h.rendered()
        );
    }
}
