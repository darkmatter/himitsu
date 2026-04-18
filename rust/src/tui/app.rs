//! Top-level TUI router for the main loop.
//!
//! The init wizard has its own standalone event loop in [`crate::tui::run_init_flow`];
//! this [`App`] wraps the post-init views (search, viewer, new-secret) and
//! routes key events between them based on the action each view returns.
//!
//! Search is the root view: Esc quits, every non-search view pops back to a
//! fresh search view.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::widgets::Clear;
use ratatui::Frame;

use crate::cli::Context;
use crate::tui::keymap::{Bindings, KeyMap};
pub use crate::tui::toast::{Toast, ToastKind};
use crate::tui::views::help::{HelpAction, HelpView};
use crate::tui::views::new_secret::{NewSecretAction, NewSecretView};
use crate::tui::views::search::{SearchAction, SearchView};
use crate::tui::views::secret_viewer::{SecretViewerAction, SecretViewerView};

enum View {
    Search(SearchView),
    SecretViewer(SecretViewerView),
    NewSecret(NewSecretView),
}

/// Intent emitted by [`App::on_key`] when a view needs the outer event
/// loop to do something that requires owning the terminal — e.g. suspend
/// the alternate screen and run `$EDITOR`.
#[derive(Debug)]
pub enum AppIntent {
    /// Suspend the TUI, open the user's editor on the given plaintext,
    /// then call [`App::finish_secret_edit`] with the outcome.
    EditSecretValue(String),
}

pub struct App {
    pub should_quit: bool,
    ctx: Context,
    view: View,
    /// User-configurable keybindings. Cloned into each view via `&KeyMap`
    /// on every key dispatch so views never have to own their own copy.
    keymap: KeyMap,
    /// Modal help overlay. When `Some`, it swallows all key events until
    /// dismissed (Esc or `?`). See [`crate::tui::views::help`].
    help: Option<HelpView>,
    /// Active toast, if any. Rendered over the bottom row of the view area
    /// until [`Toast::is_expired`] returns true, at which point `draw`
    /// clears it. Non-modal: key events still flow to the current view.
    toast: Option<Toast>,
}

impl App {
    pub fn new(ctx: &Context, keymap: KeyMap) -> Self {
        let ctx_owned = clone_ctx(ctx);
        Self {
            should_quit: false,
            view: View::Search(SearchView::new(&ctx_owned)),
            ctx: ctx_owned,
            keymap,
            help: None,
            toast: None,
        }
    }

    /// Publish a transient status-line message. Replaces any previous
    /// toast (rapid actions don't stack) and resets the 3-second TTL.
    pub fn push_toast(&mut self, msg: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast::new(msg, kind));
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<AppIntent> {
        // ── Help overlay intercept (US-012) ────────────────────────────
        // If the overlay is open, route every key to it. Otherwise, the
        // configured `help` chord opens the overlay populated from the
        // current view. Done before view dispatch so inner views never
        // have to swallow `?`.
        if let Some(help) = self.help.as_mut() {
            match help.on_key(key) {
                HelpAction::None => {}
                HelpAction::Close => self.help = None,
            }
            return None;
        }
        if self.keymap.help.matches(&key) {
            self.help = Some(self.help_for_current_view());
            return None;
        }

        match &mut self.view {
            View::Search(search) => match search.on_key(key, &self.keymap) {
                SearchAction::None => {}
                SearchAction::Quit => self.should_quit = true,
                SearchAction::OpenViewer(r) => {
                    self.view = View::SecretViewer(SecretViewerView::new(
                        &self.ctx,
                        r.store,
                        r.store_path,
                        r.path,
                    ));
                }
                SearchAction::NewSecret => {
                    self.view = View::NewSecret(NewSecretView::new(&self.ctx));
                }
                SearchAction::SwitchStore(path) => {
                    let label = path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    self.ctx.store = path;
                    self.view = View::Search(SearchView::new(&self.ctx));
                    self.push_toast(format!("switched to {label}"), ToastKind::Info);
                }
                SearchAction::Copied(path) => {
                    self.push_toast(format!("copied {path}"), ToastKind::Success);
                }
                SearchAction::CopyFailed(msg) => {
                    self.push_toast(msg, ToastKind::Error);
                }
            },
            View::SecretViewer(viewer) => match viewer.on_key(key, &self.keymap) {
                SecretViewerAction::None => {}
                SecretViewerAction::Quit => self.should_quit = true,
                SecretViewerAction::Back => {
                    self.view = View::Search(SearchView::new(&self.ctx));
                }
                SecretViewerAction::EditValue(plain) => {
                    return Some(AppIntent::EditSecretValue(plain));
                }
                SecretViewerAction::Copied => {
                    self.push_toast("copied to clipboard", ToastKind::Success);
                }
                SecretViewerAction::CopyFailed(msg) => {
                    self.push_toast(msg, ToastKind::Error);
                }
                SecretViewerAction::Deleted => {
                    // Rebuild search fresh so the (now missing) secret
                    // drops out of listings.
                    self.view = View::Search(SearchView::new(&self.ctx));
                    self.push_toast("deleted", ToastKind::Success);
                }
            },
            View::NewSecret(form) => match form.on_key(key, &self.keymap) {
                NewSecretAction::None => {}
                NewSecretAction::Quit => self.should_quit = true,
                NewSecretAction::Cancel => {
                    self.view = View::Search(SearchView::new(&self.ctx));
                    self.push_toast("create cancelled", ToastKind::Info);
                }
                NewSecretAction::Created(path) => {
                    self.view = View::Search(SearchView::new(&self.ctx));
                    self.push_toast(format!("created {path}"), ToastKind::Success);
                }
                NewSecretAction::Failed(err) => {
                    self.view = View::Search(SearchView::new(&self.ctx));
                    self.push_toast(format!("create failed: {err}"), ToastKind::Error);
                }
            },
        }
        None
    }

    /// Path to the currently active store. Exposed for integration tests
    /// that drive the App through real key events and need to assert the
    /// router updated `ctx.store` after a `SwitchStore` action.
    #[cfg(test)]
    pub fn active_store(&self) -> &std::path::Path {
        &self.ctx.store
    }

    /// Borrow the active toast for integration-test assertions. `None` once
    /// the toast has expired (lazily swept during `draw`).
    #[cfg(test)]
    pub fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref()
    }

    /// Force-expire the active toast by rewinding `expires_at` to the
    /// current instant. The next `draw` call will then sweep it away, so
    /// tests can simulate "3 seconds later" without any real sleep.
    #[cfg(test)]
    pub fn expire_toast_now(&mut self) {
        if let Some(t) = self.toast.as_mut() {
            t.expires_at = std::time::Instant::now();
        }
    }

    /// Name of the currently active view, for integration-test assertions.
    /// Returns one of `"search"`, `"secret_viewer"`, `"new_secret"`.
    #[cfg(test)]
    pub fn current_view(&self) -> &'static str {
        match &self.view {
            View::Search(_) => "search",
            View::SecretViewer(_) => "secret_viewer",
            View::NewSecret(_) => "new_secret",
        }
    }

    /// Deliver the result of an external edit back to the currently-active
    /// secret viewer. No-op if the user has already navigated away.
    pub fn finish_secret_edit(&mut self, result: std::result::Result<Option<String>, String>) {
        if let View::SecretViewer(viewer) = &mut self.view {
            viewer.finish_edit(result);
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        match &mut self.view {
            View::Search(search) => search.draw(frame),
            View::SecretViewer(viewer) => viewer.draw(frame),
            View::NewSecret(form) => form.draw(frame),
        }
        // Expire-then-paint the toast. Eviction happens lazily at draw time
        // so we don't need a background tick — any `draw` call (triggered by
        // a key event, window resize, etc.) sweeps a stale toast.
        let now = std::time::Instant::now();
        if let Some(t) = self.toast.as_ref() {
            if t.is_expired(now) {
                self.toast = None;
            }
        }
        if let Some(t) = self.toast.as_ref() {
            let area = frame.area();
            if area.height > 0 {
                let strip = Rect {
                    x: area.x,
                    y: area.y + area.height - 1,
                    width: area.width,
                    height: 1,
                };
                // Clear first so the underlying view's footer doesn't bleed
                // through on rows shorter than the toast.
                frame.render_widget(Clear, strip);
                t.render(frame, strip);
            }
        }
        // Help overlay is drawn last so it paints over the underlying view.
        if let Some(help) = self.help.as_ref() {
            help.draw(frame);
        }
    }

    /// Build a [`HelpView`] populated with entries for whichever view is
    /// currently active.
    fn help_for_current_view(&self) -> HelpView {
        match &self.view {
            View::Search(_) => {
                HelpView::new(SearchView::help_entries(), SearchView::help_title())
            }
            View::SecretViewer(_) => HelpView::new(
                SecretViewerView::help_entries(),
                SecretViewerView::help_title(),
            ),
            View::NewSecret(_) => {
                HelpView::new(NewSecretView::help_entries(), NewSecretView::help_title())
            }
        }
    }
}

fn clone_ctx(ctx: &Context) -> Context {
    Context {
        data_dir: ctx.data_dir.clone(),
        state_dir: ctx.state_dir.clone(),
        store: ctx.store.clone(),
        recipients_path: ctx.recipients_path.clone(),
    }
}
