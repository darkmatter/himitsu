//! Top-level TUI router for the main loop.
//!
//! The init wizard has its own standalone event loop in [`crate::tui::run_init_flow`];
//! this [`App`] wraps the post-init views (search, viewer, new-secret) and
//! routes key events between them based on the action each view returns.
//!
//! Search is the root view: Esc quits, every non-search view pops back to a
//! fresh search view.

use std::collections::HashSet;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::cli::Context;
pub use crate::tui::hint::Hint;
use crate::tui::keymap::{CHORD_TIMEOUT, Dispatch, KeyAction, KeyMap};
use crate::tui::layout::{CONFIRM_POPUP_HEIGHT, CONFIRM_POPUP_WIDTH, centered_length_rect};
use crate::tui::theme;
pub use crate::tui::toast::{Toast, ToastKind};
use crate::tui::views::help::{HelpAction, HelpView};
use crate::tui::views::new_secret::{NewSecretAction, NewSecretView};
use crate::tui::views::outputs::{OutputsAction, OutputsView};
use crate::tui::views::recipient_add::{RecipientAddAction, RecipientAddView};
use crate::tui::views::recipient_list::{RecipientListAction, RecipientListView};
use crate::tui::views::remote_add::{RemoteAddAction, RemoteAddView};
use crate::tui::views::search::{SearchAction, SearchView};
use crate::tui::views::secret_viewer::{SecretViewerAction, SecretViewerView};
use crate::tui::widgets::store_health::StoreHealth;

enum View {
    Search(SearchView),
    SecretViewer(SecretViewerView),
    NewSecret(NewSecretView),
    Outputs(OutputsView),
    RemoteAdd(RemoteAddView),
    RecipientList(RecipientListView),
    RecipientAdd(RecipientAddView),
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
    pub search_dirty: bool,
    ctx: Context,
    view: View,
    /// User-configurable keybindings. Cloned into each view via `&KeyMap`
    /// on every key dispatch so views never have to own their own copy.
    keymap: KeyMap,
    /// Per-mode TUI theme names resolved once at construction (so `random`
    /// picks are stable for the whole session). Applied to the search view
    /// whenever it is (re)constructed, and re-applied on each info-mode
    /// cycle so cycling modes swaps palettes.
    theme_names: [String; 3],
    /// Modal help overlay. When `Some`, it swallows all key events until
    /// dismissed (Esc or `?`). See [`crate::tui::views::help`].
    help: Option<HelpView>,
    /// Active toast, if any. Rendered over the bottom row of the view area
    /// until [`Toast::is_expired`] returns true, at which point `draw`
    /// clears it. Non-modal: key events still flow to the current view.
    toast: Option<Toast>,
    /// Active ambient hint, if any. Painted in the bottom-left strip and
    /// persists until the owning view replaces or clears it. Suppressed
    /// while a toast is on screen so the two surfaces never fight for
    /// attention — see `App::draw`.
    hint: Option<Hint>,
    /// Buffer of chord steps already pressed but not yet resolved. Set by
    /// [`KeyMap::dispatch`] returning [`Dispatch::Pending`]; cleared on the
    /// next match, abort, timeout, or non-chord keypress.
    pending_chord: Vec<KeyEvent>,
    /// When the current pending chord expires if no continuation arrives.
    pending_chord_deadline: Option<std::time::Instant>,
    /// `true` while the active toast is the chord-progress breadcrumb.
    /// Tracked explicitly so [`Self::dismiss_chord_breadcrumb`] doesn't
    /// have to inspect the toast's text — an unrelated info toast that
    /// happens to land during a pending chord must not be cleared by
    /// breadcrumb dismissal.
    chord_breadcrumb_active: bool,
    /// Concrete scope to restore when returning from a subview; None means All.
    subview_store: Option<Context>,
    /// A declined (or attempted) join shouldn't interrupt this session again.
    answered_join: HashSet<PathBuf>,
}

impl App {
    pub fn new(ctx: &Context, keymap: KeyMap, theme: &crate::config::TuiTheme) -> Self {
        let theme_names = resolve_theme_names(theme);
        let mut search = SearchView::new(ctx);
        search.set_theme_names(theme_names.clone());
        let ctx_owned = search.active_context().clone();
        Self {
            should_quit: false,
            search_dirty: false,
            view: View::Search(search),
            ctx: ctx_owned,
            keymap,
            theme_names,
            help: None,
            toast: None,
            hint: None,
            pending_chord: Vec::new(),
            pending_chord_deadline: None,
            chord_breadcrumb_active: false,
            subview_store: None,
            answered_join: HashSet::new(),
        }
    }

    fn fresh_search(&self) -> SearchView {
        let mut view = SearchView::new(&self.ctx);
        view.set_theme_names(self.theme_names.clone());
        if let Some(ctx) = &self.subview_store {
            view.restore_store(&ctx.store);
        }
        view
    }

    fn open_from_search(&mut self, open: impl FnOnce(&Context) -> View) {
        if let View::Search(search) = &self.view {
            let ctx = search.active_context();
            self.subview_store = search.selected_store().map(|_| ctx.clone());
            self.view = open(ctx);
        }
    }

    fn subview_context(&self) -> &Context {
        self.subview_store.as_ref().unwrap_or(&self.ctx)
    }

    fn join_prompt(&self) -> Option<&Context> {
        let View::Search(search) = &self.view else {
            return None;
        };
        let ctx = search.active_context();
        (!ctx.store.as_os_str().is_empty()
            && matches!(search.active_health(), StoreHealth::NotRecipient)
            && !self.answered_join.contains(&ctx.store))
        .then_some(ctx)
    }

    /// Publish a transient status-line message. Replaces any previous
    /// toast (rapid actions don't stack) and resets the 3-second TTL.
    /// Any active chord breadcrumb is also cleared — a normal toast
    /// supersedes the chord prompt.
    pub fn push_toast(&mut self, msg: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast::new(msg, kind));
        self.chord_breadcrumb_active = false;
    }

    /// Publish an ambient bottom-left hint, replacing any previous one.
    /// No expiry — callers must clear via [`App::clear_hint`] when the
    /// context the hint described goes away (e.g. the view closes or
    /// focus moves to a field with no associated tip).
    pub fn set_hint(&mut self, msg: impl Into<String>) {
        self.hint = Some(Hint::new(msg));
    }

    /// Drop the active hint, if any.
    pub fn clear_hint(&mut self) {
        self.hint = None;
    }

    pub fn refresh_search(&mut self) {
        if let View::Search(search) = &mut self.view {
            search.refresh_results();
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<AppIntent> {
        if let Some(ctx) = self.join_prompt() {
            match (key.code, key.modifiers) {
                (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
                    self.answered_join.insert(ctx.store.clone());
                    if let View::Search(search) = &mut self.view {
                        let outcome = search.run_join();
                        self.handle_search_action(outcome);
                    }
                }
                (KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter, _) => {
                    self.answered_join.insert(ctx.store.clone());
                }
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.should_quit = true,
                _ => {}
            }
            return None;
        }

        // ── Help overlay intercept (US-012) ────────────────────────────
        // If the overlay is open, route every key to it. Done before
        // chord dispatch so the help overlay can never accidentally
        // consume a leader chord step.
        if let Some(help) = self.help.as_mut() {
            match help.on_key(key) {
                HelpAction::None => {}
                HelpAction::Close => self.help = None,
            }
            return None;
        }

        self.tick_chord_timeout();

        // ── Leader-key chord dispatcher ───────────────────────────────
        // Drives the multi-step chord state machine. If the key is part
        // of an in-flight chord (or starts one), it's swallowed here.
        // Only `Unmatched` falls through to the legacy per-key flow.
        match self.keymap.dispatch(&self.pending_chord, &key) {
            Dispatch::Match(action) => {
                self.clear_pending_chord();
                return self.run_keymap_action(action);
            }
            Dispatch::Pending => {
                self.pending_chord.push(key);
                self.pending_chord_deadline = Some(std::time::Instant::now() + CHORD_TIMEOUT);
                self.show_chord_breadcrumb();
                return None;
            }
            Dispatch::Unmatched => {
                if !self.pending_chord.is_empty() {
                    // The pending chord aborted because this key isn't a
                    // continuation. Surface the abort so the user knows
                    // their leader sequence didn't fire anything.
                    let summary = format_pending(&self.pending_chord);
                    self.clear_pending_chord();
                    self.push_toast(format!("chord aborted: {summary}"), ToastKind::Info);
                    return None;
                }
            }
        }

        if self
            .keymap
            .action_for_key_in(&key, &[KeyAction::Help])
            .is_some()
        {
            return self.run_keymap_action(KeyAction::Help);
        }

        // ── Single-key fallthrough (no active chord) ──────────────────
        match &mut self.view {
            View::Search(search) => {
                let action = search.on_key(key, &self.keymap);
                let search_dirty = search.take_search_dirty();
                let intent = self.handle_search_action(action);
                if search_dirty {
                    self.search_dirty = true;
                }
                intent
            }
            View::SecretViewer(viewer) => {
                let action = viewer.on_key(key, &self.keymap);
                self.handle_secret_viewer_action(action)
            }
            View::Outputs(envs) => {
                let action = envs.on_key(key, &self.keymap);
                self.handle_outputs_action(action)
            }
            View::NewSecret(form) => {
                let action = form.on_key(key, &self.keymap);
                self.handle_new_secret_action(action)
            }
            View::RemoteAdd(form) => {
                let action = form.on_key(key, &self.keymap);
                self.handle_remote_add_action(action)
            }
            View::RecipientList(list) => {
                let action = list.on_key(key, &self.keymap);
                self.handle_recipient_list_action(action)
            }
            View::RecipientAdd(form) => {
                let action = form.on_key(key, &self.keymap);
                self.handle_recipient_add_action(action)
            }
        }
    }

    /// Deliver a completed chord (or any [`KeyAction`] resolved by name)
    /// to whichever target owns it. App-level actions like `Help` and
    /// `Quit` are handled directly; everything else is forwarded to the
    /// active view's `dispatch_action`.
    fn run_keymap_action(&mut self, action: KeyAction) -> Option<AppIntent> {
        match action {
            KeyAction::Quit => {
                self.should_quit = true;
                return None;
            }
            KeyAction::Help => {
                self.help = Some(self.help_for_current_view());
                return None;
            }
            _ => {}
        }

        // Clone the keymap up front: the per-view dispatch below borrows
        // `self.view` mutably, so it can't also hold `&self.keymap`.
        let keymap = self.keymap.clone();
        match &mut self.view {
            View::Search(search) => {
                if let Some(action) = search.dispatch_action(action, &keymap) {
                    return self.handle_search_action(action);
                }
            }
            View::SecretViewer(viewer) => {
                if let Some(action) = viewer.dispatch_action(action) {
                    return self.handle_secret_viewer_action(action);
                }
            }
            View::NewSecret(form) => {
                if let Some(action) = form.dispatch_action(action) {
                    return self.handle_new_secret_action(action);
                }
            }
            // These views don't expose an action dispatcher; their
            // keymap-driven behaviour stays inside their `on_key` for now.
            // Falling through is fine — chord completion in those views just
            // no-ops, since none of their bindings are multi-step by default.
            View::Outputs(_)
            | View::RemoteAdd(_)
            | View::RecipientList(_)
            | View::RecipientAdd(_) => {}
        }
        None
    }

    fn handle_search_action(&mut self, action: SearchAction) -> Option<AppIntent> {
        match action {
            SearchAction::None => {}
            SearchAction::Quit => self.should_quit = true,
            SearchAction::OpenViewer(r) => {
                self.open_from_search(|ctx| {
                    View::SecretViewer(SecretViewerView::new(ctx, r.store, r.store_path, r.path))
                });
            }
            SearchAction::NewSecret => {
                self.open_from_search(|ctx| View::NewSecret(NewSecretView::new(ctx)));
                // Seed the bottom-left hint with the tip for the initial
                // focus (Path) so the user sees guidance immediately,
                // before they press any keys.
                if let Some(msg) =
                    NewSecretView::hint_for_step(crate::tui::views::new_secret::Step::Path)
                {
                    self.set_hint(msg);
                }
            }
            SearchAction::AddRemote => {
                self.open_from_search(|ctx| View::RemoteAdd(RemoteAddView::new(ctx)));
            }
            SearchAction::OpenOutputs => {
                self.open_from_search(|ctx| View::Outputs(OutputsView::new(ctx)));
            }
            SearchAction::OpenRecipientList => {
                self.open_from_search(|ctx| View::RecipientList(RecipientListView::new(ctx)));
            }
            SearchAction::OpenRecipientAdd => {
                self.open_from_search(|ctx| View::RecipientAdd(RecipientAddView::new(ctx)));
            }
            SearchAction::SwitchStore(path) => {
                let slug = crate::cli::search::store_label(&path, &self.ctx);
                if let View::Search(search) = &mut self.view
                    && !search.restore_store(&path)
                {
                    // The picker also accepts paths outside the registered checkouts.
                    self.ctx.recipients_path = crate::remote::store::load_store_config(&path)
                        .ok()
                        .and_then(|cfg| cfg.recipients_path);
                    self.ctx.store = path;
                    let mut view = SearchView::new(&self.ctx);
                    view.set_theme_names(self.theme_names.clone());
                    view.restore_store(&self.ctx.store);
                    self.view = View::Search(view);
                }
                self.push_toast(format!("switched to {slug}"), ToastKind::Info);
            }
            SearchAction::ShowHelp => {
                self.help = Some(self.help_for_current_view());
            }
            SearchAction::Copied(path) => {
                self.push_toast(format!("copied {path}"), ToastKind::Success);
            }
            SearchAction::CopyFailed(msg) => {
                self.push_toast(msg, ToastKind::Error);
            }
            SearchAction::Synced(msg) => {
                self.push_toast(msg, ToastKind::Success);
            }
            SearchAction::Rekeyed(msg) => {
                self.push_toast(msg, ToastKind::Success);
            }
            SearchAction::Joined(msg) => {
                self.push_toast(msg, ToastKind::Success);
            }
            SearchAction::CommandFailed(msg) => {
                self.push_toast(msg, ToastKind::Error);
            }
            SearchAction::CommandHint(msg) => {
                self.push_toast(msg, ToastKind::Info);
            }
        }
        None
    }

    fn handle_secret_viewer_action(&mut self, action: SecretViewerAction) -> Option<AppIntent> {
        match action {
            SecretViewerAction::None => {}
            SecretViewerAction::Quit => self.should_quit = true,
            SecretViewerAction::Back => {
                self.view = View::Search(self.fresh_search());
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
                self.view = View::Search(self.fresh_search());
                self.push_toast("deleted", ToastKind::Success);
            }
        }
        None
    }

    fn handle_outputs_action(&mut self, action: OutputsAction) -> Option<AppIntent> {
        match action {
            OutputsAction::None => {}
            OutputsAction::Quit => self.should_quit = true,
            OutputsAction::Back => {
                self.view = View::Search(self.fresh_search());
            }
            OutputsAction::Deleted { label, scope } => {
                let scope_str = match scope {
                    crate::config::outputs::outputs_mut::Scope::Project => "project",
                    crate::config::outputs::outputs_mut::Scope::Global => "global",
                };
                self.push_toast(
                    format!("deleted `{label}` ({scope_str})"),
                    ToastKind::Success,
                );
            }
            OutputsAction::DeleteFailed(msg) => {
                self.push_toast(msg, ToastKind::Error);
            }
            OutputsAction::Created { label, scope } => {
                let scope_str = match scope {
                    crate::config::outputs::outputs_mut::Scope::Project => "project",
                    crate::config::outputs::outputs_mut::Scope::Global => "global",
                };
                self.push_toast(
                    format!("created `{label}` ({scope_str})"),
                    ToastKind::Success,
                );
            }
            OutputsAction::CreateFailed(msg) => {
                self.push_toast(msg, ToastKind::Error);
            }
        }
        None
    }

    fn handle_new_secret_action(&mut self, action: NewSecretAction) -> Option<AppIntent> {
        match action {
            NewSecretAction::None => {}
            NewSecretAction::Quit => self.should_quit = true,
            NewSecretAction::Cancel => {
                self.view = View::Search(self.fresh_search());
                self.clear_hint();
                self.push_toast("create cancelled", ToastKind::Info);
            }
            NewSecretAction::Created(path) => {
                self.view = View::Search(self.fresh_search());
                self.clear_hint();
                self.push_toast(format!("created {path}"), ToastKind::Success);
            }
            NewSecretAction::Failed(err) => {
                self.view = View::Search(self.fresh_search());
                self.clear_hint();
                self.push_toast(format!("create failed: {err}"), ToastKind::Error);
            }
            NewSecretAction::SetHint(msg) => self.set_hint(msg),
            NewSecretAction::ClearHint => self.clear_hint(),
        }
        None
    }

    fn handle_remote_add_action(&mut self, action: RemoteAddAction) -> Option<AppIntent> {
        match action {
            RemoteAddAction::None => {}
            RemoteAddAction::Quit => self.should_quit = true,
            RemoteAddAction::Cancel => {
                self.view = View::Search(self.fresh_search());
                self.push_toast("add remote cancelled", ToastKind::Info);
            }
            RemoteAddAction::Created(slug) => {
                self.view = View::Search(self.fresh_search());
                self.push_toast(format!("added remote {slug}"), ToastKind::Success);
            }
            RemoteAddAction::Failed(err) => {
                self.view = View::Search(self.fresh_search());
                self.push_toast(format!("add remote failed: {err}"), ToastKind::Error);
            }
        }
        None
    }

    fn handle_recipient_list_action(&mut self, action: RecipientListAction) -> Option<AppIntent> {
        match action {
            RecipientListAction::None => {}
            RecipientListAction::Quit => self.should_quit = true,
            RecipientListAction::Back => {
                self.view = View::Search(self.fresh_search());
            }
            RecipientListAction::OpenAdd => {
                self.view = View::RecipientAdd(RecipientAddView::new(self.subview_context()));
            }
            RecipientListAction::Removed(name) => {
                // Stay in the list view (it reloaded itself) and confirm.
                self.push_toast(format!("removed recipient {name}"), ToastKind::Success);
            }
            RecipientListAction::Failed(err) => {
                self.push_toast(format!("remove recipient failed: {err}"), ToastKind::Error);
            }
        }
        None
    }

    fn handle_recipient_add_action(&mut self, action: RecipientAddAction) -> Option<AppIntent> {
        match action {
            RecipientAddAction::None => {}
            RecipientAddAction::Quit => self.should_quit = true,
            RecipientAddAction::Cancel => {
                // Return to the recipient list so the user stays in context.
                self.view = View::RecipientList(RecipientListView::new(self.subview_context()));
                self.push_toast("add recipient cancelled", ToastKind::Info);
            }
            RecipientAddAction::Created(name) => {
                self.view = View::RecipientList(RecipientListView::new(self.subview_context()));
                self.push_toast(format!("added recipient {name}"), ToastKind::Success);
            }
            RecipientAddAction::Failed(err) => {
                self.view = View::RecipientList(RecipientListView::new(self.subview_context()));
                self.push_toast(format!("add recipient failed: {err}"), ToastKind::Error);
            }
        }
        None
    }

    fn clear_pending_chord(&mut self) {
        self.pending_chord.clear();
        self.pending_chord_deadline = None;
        self.dismiss_chord_breadcrumb();
    }

    /// Drop an expired pending chord. Called from the event loop on every
    /// tick and at the start of each keypress. Returns `true` when a chord
    /// was cleared.
    pub fn tick_chord_timeout(&mut self) -> bool {
        if self.pending_chord.is_empty() {
            return false;
        }
        let Some(deadline) = self.pending_chord_deadline else {
            return false;
        };
        if std::time::Instant::now() < deadline {
            return false;
        }
        self.clear_pending_chord();
        true
    }

    fn show_chord_breadcrumb(&mut self) {
        let summary = format_pending(&self.pending_chord);
        // Set the toast directly (don't go through `push_toast`, which
        // would clobber the breadcrumb flag we're about to set).
        self.toast = Some(Toast::new(format!("{summary} …"), ToastKind::Info));
        self.chord_breadcrumb_active = true;
    }

    /// Drop the chord-prompt toast iff it's still the active toast. An
    /// unrelated toast that landed during the pending chord (e.g. an
    /// auto-pull warning) is left alone.
    fn dismiss_chord_breadcrumb(&mut self) {
        if self.chord_breadcrumb_active {
            self.toast = None;
            self.chord_breadcrumb_active = false;
        }
    }

    /// Path to the currently active store. Exposed for integration tests
    /// that drive the App through real key events and need to assert the
    /// router updated `ctx.store` after a `SwitchStore` action.
    #[cfg(test)]
    pub fn active_store(&self) -> &std::path::Path {
        match &self.view {
            View::Search(search) => &search.active_context().store,
            _ => &self.subview_context().store,
        }
    }

    /// Borrow the active toast for integration-test assertions. `None` once
    /// the toast has expired (lazily swept during `draw`).
    #[cfg(test)]
    pub fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref()
    }

    /// Borrow the active hint for integration-test assertions.
    #[cfg(test)]
    pub fn hint(&self) -> Option<&Hint> {
        self.hint.as_ref()
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

    /// Force-expire the active chord continuation window. The next
    /// [`Self::tick_chord_timeout`] call clears the pending buffer.
    #[cfg(test)]
    pub fn expire_chord_timeout_now(&mut self) {
        if self.pending_chord_deadline.is_some() {
            self.pending_chord_deadline =
                Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
        }
    }

    /// Snapshot of the pending-chord buffer for integration tests.
    #[cfg(test)]
    pub fn pending_chord_len(&self) -> usize {
        self.pending_chord.len()
    }

    /// Name of the currently active view, for integration-test assertions.
    /// Returns one of `"search"`, `"secret_viewer"`, `"new_secret"`, `"envs"`.
    #[cfg(test)]
    pub fn current_view(&self) -> &'static str {
        match &self.view {
            View::Search(_) => "search",
            View::SecretViewer(_) => "secret_viewer",
            View::NewSecret(_) => "new_secret",
            View::Outputs(_) => "envs",
            View::RemoteAdd(_) => "remote_add",
            View::RecipientList(_) => "recipient_list",
            View::RecipientAdd(_) => "recipient_add",
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
        // Paint the theme's background across the entire frame before any
        // view draws. Themes that want to inherit the terminal's native
        // background use `Color::Reset`, which is a no-op visually.
        let bg = Block::default().style(Style::default().bg(theme::background()));
        frame.render_widget(bg, frame.area());

        match &mut self.view {
            View::Search(search) => search.draw(frame),
            View::SecretViewer(viewer) => viewer.draw(frame),
            View::NewSecret(form) => form.draw(frame),
            View::Outputs(envs) => envs.draw(frame),
            View::RemoteAdd(form) => form.draw(frame),
            View::RecipientList(list) => list.draw(frame),
            View::RecipientAdd(form) => form.draw(frame),
        }
        // Expire-then-paint the toast. Eviction happens lazily at draw time
        // so we don't need a background tick — any `draw` call (triggered by
        // a key event, window resize, etc.) sweeps a stale toast.
        let now = std::time::Instant::now();
        if let Some(t) = self.toast.as_ref()
            && t.is_expired(now)
        {
            self.toast = None;
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
        } else if let Some(h) = self.hint.as_ref() {
            // Hint is suppressed whenever a toast is active so the two
            // bottom-row surfaces never fight for the same cells. Toasts
            // are transient (3s TTL); the hint reappears as soon as the
            // toast clears.
            let area = frame.area();
            if area.height > 0 {
                let strip = Rect {
                    x: area.x,
                    y: area.y + area.height - 1,
                    width: area.width,
                    height: 1,
                };
                h.render(frame, strip);
            }
        }
        // Help overlay is drawn last so it paints over the underlying view.
        if let Some(help) = self.help.as_ref() {
            help.draw(frame);
        }
        if let Some(ctx) = self.join_prompt() {
            let area =
                centered_length_rect(frame.area(), CONFIRM_POPUP_WIDTH, CONFIRM_POPUP_HEIGHT);
            let store = crate::cli::search::store_label(&ctx.store, ctx);
            let prompt = Paragraph::new(format!(
                "You are not a recipient of this store. Join now?\n{store}"
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::accent()))
                    .title(" Join store ")
                    .title_bottom(" ^Y: add and push key   n/Esc: skip "),
            )
            .wrap(Wrap { trim: false });
            frame.render_widget(Clear, area);
            frame.render_widget(prompt, area);
        }
    }

    /// Build a [`HelpView`] populated with entries for whichever view is
    /// currently active. Views with rebindable actions derive their rows
    /// from the live keymap, so a user rebind shows up in help immediately.
    fn help_for_current_view(&self) -> HelpView {
        match &self.view {
            View::Search(_) => HelpView::new(
                SearchView::help_entries(&self.keymap),
                SearchView::help_title(),
            ),
            View::SecretViewer(_) => HelpView::new(
                SecretViewerView::help_entries(&self.keymap),
                SecretViewerView::help_title(),
            ),
            View::NewSecret(_) => HelpView::new(
                NewSecretView::help_entries(&self.keymap),
                NewSecretView::help_title(),
            ),
            View::Outputs(_) => {
                HelpView::from_static(OutputsView::help_entries(), OutputsView::help_title())
            }
            View::RemoteAdd(_) => {
                HelpView::from_static(RemoteAddView::help_entries(), RemoteAddView::help_title())
            }
            View::RecipientList(_) => HelpView::from_static(
                RecipientListView::help_entries(),
                RecipientListView::help_title(),
            ),
            View::RecipientAdd(_) => HelpView::from_static(
                RecipientAddView::help_entries(),
                RecipientAddView::help_title(),
            ),
        }
    }
}

/// Pretty-print a pending chord buffer for the breadcrumb toast — defers
/// to [`KeyChord::from_events`] + its `Display` impl so the rendering is
/// always in lock-step with the parser users edit in their config.
fn format_pending(events: &[KeyEvent]) -> String {
    use crate::tui::keymap::KeyChord;
    KeyChord::from_events(events)
        .map(|c| c.to_string())
        .unwrap_or_default()
}

/// Resolve a `TuiTheme` into concrete per-mode theme names (`[user,
/// project, all]`). `random`/`default` are resolved once here so the
/// session keeps a stable random pick instead of re-rolling on every
/// info-mode cycle or view rebuild.
fn resolve_theme_names(theme: &crate::config::TuiTheme) -> [String; 3] {
    let mut names = [
        String::from("himitsu"),
        String::from("himitsu"),
        String::from("himitsu"),
    ];
    for (mode, idx) in [("user", 0usize), ("project", 1), ("all", 2)] {
        names[idx] = crate::tui::theme::resolve_random(theme.name_for(mode))
            .unwrap_or("himitsu")
            .to_string();
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn format_pending_renders_canonical_keys() {
        let events = vec![
            key(KeyCode::Char('x'), KeyModifiers::CONTROL),
            key(KeyCode::Char('s'), KeyModifiers::NONE),
        ];
        assert_eq!(format_pending(&events), "ctrl+x s");
    }
}
