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

/// Which view the user was on before opening the secret viewer — controls
/// where `Esc` from the viewer pops them back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewerParent {
    Dashboard,
    Search,
}

pub struct App {
    pub should_quit: bool,
    ctx: Context,
    view: View,
    viewer_parent: ViewerParent,
}

impl App {
    pub fn new(ctx: &Context) -> Self {
        let ctx_owned = clone_ctx(ctx);
        Self {
            should_quit: false,
            view: View::Dashboard(DashboardView::new(&ctx_owned)),
            ctx: ctx_owned,
            viewer_parent: ViewerParent::Search,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
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
            },
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
