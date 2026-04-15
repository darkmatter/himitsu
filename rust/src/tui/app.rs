//! Top-level TUI router for the main loop.
//!
//! The init wizard has its own standalone event loop in [`crate::tui::run_init_flow`];
//! this [`App`] wraps the post-init views (search, viewer, new-secret) and
//! routes key events between them based on the action each view returns.
//!
//! Search is the root view: Esc quits, every non-search view pops back to a
//! fresh search view.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;

use crate::cli::Context;
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
    /// Modal help overlay. When `Some`, it swallows all key events until
    /// dismissed (Esc or `?`). See [`crate::tui::views::help`].
    help: Option<HelpView>,
}

impl App {
    pub fn new(ctx: &Context) -> Self {
        let ctx_owned = clone_ctx(ctx);
        Self {
            should_quit: false,
            view: View::Search(SearchView::new(&ctx_owned)),
            ctx: ctx_owned,
            help: None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<AppIntent> {
        // ── Help overlay intercept (US-012) ────────────────────────────
        // If the overlay is open, route every key to it. Otherwise, a
        // top-level `?` opens the overlay populated from the current view.
        // Done before view dispatch so inner views never have to swallow `?`.
        if let Some(help) = self.help.as_mut() {
            match help.on_key(key) {
                HelpAction::None => {}
                HelpAction::Close => self.help = None,
            }
            return None;
        }
        if matches!(key.code, KeyCode::Char('?')) {
            self.help = Some(self.help_for_current_view());
            return None;
        }

        match &mut self.view {
            View::Search(search) => match search.on_key(key) {
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
                    self.ctx.store = path;
                    self.view = View::Search(SearchView::new(&self.ctx));
                }
            },
            View::SecretViewer(viewer) => match viewer.on_key(key) {
                SecretViewerAction::None => {}
                SecretViewerAction::Quit => self.should_quit = true,
                SecretViewerAction::Back => {
                    self.view = View::Search(SearchView::new(&self.ctx));
                }
                SecretViewerAction::EditValue(plain) => {
                    return Some(AppIntent::EditSecretValue(plain));
                }
                SecretViewerAction::Deleted => {
                    // Rebuild search fresh so the (now missing) secret
                    // drops out of listings.
                    self.view = View::Search(SearchView::new(&self.ctx));
                }
            },
            View::NewSecret(form) => match form.on_key(key) {
                NewSecretAction::None => {}
                NewSecretAction::Quit => self.should_quit = true,
                NewSecretAction::Cancel => {
                    let mut search = SearchView::new(&self.ctx);
                    search.set_status_info("create cancelled");
                    self.view = View::Search(search);
                }
                NewSecretAction::Created(path) => {
                    let mut search = SearchView::new(&self.ctx);
                    search.set_status_info(format!("created {path}"));
                    self.view = View::Search(search);
                }
                NewSecretAction::Failed(err) => {
                    let mut search = SearchView::new(&self.ctx);
                    search.set_status_error(format!("create failed: {err}"));
                    self.view = View::Search(search);
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
