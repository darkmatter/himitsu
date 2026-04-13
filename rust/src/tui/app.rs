//! Top-level TUI router for the dashboard loop.
//!
//! The init wizard has its own standalone event loop in [`crate::tui::run_init_flow`];
//! this [`App`] wraps the post-init views (dashboard, search) and routes key
//! events between them based on the action each view returns.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;

use crate::cli::Context;
use crate::tui::views::dashboard::{DashboardAction, DashboardView};
use crate::tui::views::help::{HelpAction, HelpView};
use crate::tui::views::search::{SearchAction, SearchView};
use crate::tui::views::secret_viewer::{SecretViewerAction, SecretViewerView};

enum View {
    Dashboard(DashboardView),
    Search(SearchView),
    SecretViewer(SecretViewerView),
}

/// Which view the user was on before opening the secret viewer — controls
/// where `Esc` from the viewer pops them back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewerParent {
    Dashboard,
    Search,
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
    viewer_parent: ViewerParent,
    /// Modal help overlay. When `Some`, it swallows all key events until
    /// dismissed (Esc or `?`). See [`crate::tui::views::help`].
    help: Option<HelpView>,
}

impl App {
    pub fn new(ctx: &Context) -> Self {
        let ctx_owned = clone_ctx(ctx);
        Self {
            should_quit: false,
            view: View::Dashboard(DashboardView::new(&ctx_owned)),
            ctx: ctx_owned,
            viewer_parent: ViewerParent::Search,
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
            View::Dashboard(dash) => match dash.on_key(key) {
                DashboardAction::None => {}
                DashboardAction::Quit => self.should_quit = true,
                DashboardAction::EnterSearch => {
                    self.view = View::Search(SearchView::new(&self.ctx));
                }
                DashboardAction::OpenViewer(r) => {
                    self.viewer_parent = ViewerParent::Dashboard;
                    self.view = View::SecretViewer(SecretViewerView::new(
                        &self.ctx,
                        r.store,
                        r.store_path,
                        r.path,
                    ));
                }
            },
            View::Search(search) => match search.on_key(key) {
                SearchAction::None => {}
                SearchAction::Quit => self.should_quit = true,
                SearchAction::Back => {
                    self.view = View::Dashboard(DashboardView::new(&self.ctx));
                }
                SearchAction::OpenViewer(r) => {
                    self.viewer_parent = ViewerParent::Search;
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
                    // Pop back to whichever view opened the viewer.
                    self.view = match self.viewer_parent {
                        ViewerParent::Dashboard => {
                            View::Dashboard(DashboardView::new(&self.ctx))
                        }
                        ViewerParent::Search => View::Search(SearchView::new(&self.ctx)),
                    };
                }
                SecretViewerAction::EditValue(plain) => {
                    return Some(AppIntent::EditSecretValue(plain));
                }
                SecretViewerAction::Deleted => {
                    // Pop back to whichever view opened the viewer, fresh so
                    // the (now missing) secret drops out of listings.
                    self.view = match self.viewer_parent {
                        ViewerParent::Dashboard => {
                            View::Dashboard(DashboardView::new(&self.ctx))
                        }
                        ViewerParent::Search => View::Search(SearchView::new(&self.ctx)),
                    };
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
        // Help overlay is drawn last so it paints over the underlying view.
        if let Some(help) = self.help.as_ref() {
            help.draw(frame);
        }
    }

    /// Build a [`HelpView`] populated with entries for whichever view is
    /// currently active.
    fn help_for_current_view(&self) -> HelpView {
        match &self.view {
            View::Dashboard(_) => {
                HelpView::new(DashboardView::help_entries(), DashboardView::help_title())
            }
            View::Search(_) => {
                HelpView::new(SearchView::help_entries(), SearchView::help_title())
            }
            View::SecretViewer(_) => HelpView::new(
                SecretViewerView::help_entries(),
                SecretViewerView::help_title(),
            ),
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
