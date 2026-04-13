//! Top-level TUI router for the dashboard loop.
//!
//! The init wizard has its own standalone event loop in [`crate::tui::run_init_flow`];
//! this [`App`] wraps the post-init views (dashboard, search) and routes key
//! events between them based on the action each view returns.

use crossterm::event::KeyEvent;
use ratatui::Frame;

use crate::cli::Context;
use crate::tui::views::dashboard::{DashboardAction, DashboardView};
use crate::tui::views::search::{SearchAction, SearchView};
use crate::tui::views::secret_viewer::{SecretViewerAction, SecretViewerView};

enum View {
    Dashboard(DashboardView),
    Search(SearchView),
    SecretViewer(SecretViewerView),
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
}

impl App {
    pub fn new(ctx: &Context) -> Self {
        let ctx_owned = clone_ctx(ctx);
        Self {
            should_quit: false,
            view: View::Dashboard(DashboardView::new(&ctx_owned)),
            ctx: ctx_owned,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<AppIntent> {
        match &mut self.view {
            View::Dashboard(dash) => match dash.on_key(key) {
                DashboardAction::None => {}
                DashboardAction::Quit => self.should_quit = true,
                DashboardAction::EnterSearch => {
                    self.view = View::Search(SearchView::new(&self.ctx));
                }
            },
            View::Search(search) => match search.on_key(key) {
                SearchAction::None => {}
                SearchAction::Quit => self.should_quit = true,
                SearchAction::Back => {
                    self.view = View::Dashboard(DashboardView::new(&self.ctx));
                }
                SearchAction::OpenViewer(r) => {
                    self.view = View::SecretViewer(SecretViewerView::new(
                        &self.ctx,
                        r.store,
                        r.store_path,
                        r.path,
                    ));
                }
            },
            View::SecretViewer(viewer) => match viewer.on_key(key) {
                SecretViewerAction::None => {}
                SecretViewerAction::Quit => self.should_quit = true,
                SecretViewerAction::Back => {
                    // "Previous view" is the search view — rebuild it so the
                    // query is fresh (we don't retain search state on purpose).
                    self.view = View::Search(SearchView::new(&self.ctx));
                }
                SecretViewerAction::EditValue(plain) => {
                    return Some(AppIntent::EditSecretValue(plain));
                }
            },
        }
        None
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
            View::Dashboard(dash) => dash.draw(frame),
            View::Search(search) => search.draw(frame),
            View::SecretViewer(viewer) => viewer.draw(frame),
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
