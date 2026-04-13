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
            view: View::Dashboard(DashboardView::new(&ctx_owned)),
            ctx: ctx_owned,
            help: None,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // ── Help overlay intercept (US-012) ────────────────────────────
        // If the overlay is open, route every key to it. Otherwise, a
        // top-level `?` opens the overlay populated from the current view.
        // Done before view dispatch so inner views never have to swallow `?`.
        if let Some(help) = self.help.as_mut() {
            match help.on_key(key) {
                HelpAction::None => {}
                HelpAction::Close => self.help = None,
            }
            return;
        }
        if matches!(key.code, KeyCode::Char('?')) {
            self.help = Some(self.help_for_current_view());
            return;
        }

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
            },
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
