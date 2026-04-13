//! Top-level TUI router for the dashboard loop.
//!
//! The init wizard has its own standalone event loop in [`crate::tui::run_init_flow`];
//! this [`App`] wraps the post-init views (dashboard, search) and routes key
//! events between them based on the action each view returns.

use crossterm::event::KeyEvent;
use ratatui::Frame;

use crate::cli::Context;
use crate::tui::views::dashboard::{DashboardAction, DashboardView};
use crate::tui::views::new_secret::{NewSecretAction, NewSecretView};
use crate::tui::views::search::{SearchAction, SearchView};
use crate::tui::views::secret_viewer::{SecretViewerAction, SecretViewerView};

enum View {
    Dashboard(DashboardView),
    Search(SearchView),
    SecretViewer(SecretViewerView),
    NewSecret(NewSecretView),
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

    pub fn on_key(&mut self, key: KeyEvent) {
        match &mut self.view {
            View::Dashboard(dash) => match dash.on_key(key) {
                DashboardAction::None => {}
                DashboardAction::Quit => self.should_quit = true,
                DashboardAction::EnterSearch => {
                    self.view = View::Search(SearchView::new(&self.ctx));
                }
                DashboardAction::NewSecret => {
                    let default_env = dash.selected_env();
                    self.view =
                        View::NewSecret(NewSecretView::new(&self.ctx, default_env));
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
            View::NewSecret(form) => match form.on_key(key) {
                NewSecretAction::None => {}
                NewSecretAction::Quit => self.should_quit = true,
                NewSecretAction::Cancel => {
                    let mut dash = DashboardView::new(&self.ctx);
                    dash.set_status_info("create cancelled");
                    self.view = View::Dashboard(dash);
                }
                NewSecretAction::Created(path) => {
                    let mut dash = DashboardView::new(&self.ctx);
                    dash.refresh_and_select(Some(&path));
                    dash.set_status_info(format!("created {path}"));
                    self.view = View::Dashboard(dash);
                }
                NewSecretAction::Failed(err) => {
                    let mut dash = DashboardView::new(&self.ctx);
                    dash.set_status_error(format!("create failed: {err}"));
                    self.view = View::Dashboard(dash);
                }
            },
        }
    }

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        match &mut self.view {
            View::Dashboard(dash) => dash.draw(frame),
            View::Search(search) => search.draw(frame),
            View::SecretViewer(viewer) => viewer.draw(frame),
            View::NewSecret(form) => form.draw(frame),
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
