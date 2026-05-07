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
use ratatui::style::Style;
use ratatui::widgets::{Block, Clear};
use ratatui::Frame;

use crate::cli::Context;
use crate::tui::keymap::{Dispatch, KeyAction, KeyMap};
use crate::tui::theme;
pub use crate::tui::toast::{Toast, ToastKind};
use crate::tui::views::envs::{EnvsAction, EnvsView};
use crate::tui::views::help::{HelpAction, HelpView};
use crate::tui::views::new_secret::{NewSecretAction, NewSecretView};
use crate::tui::views::remote_add::{RemoteAddAction, RemoteAddView};
use crate::tui::views::search::{SearchAction, SearchView};
use crate::tui::views::secret_viewer::{SecretViewerAction, SecretViewerView};

enum View {
    Search(SearchView),
    SecretViewer(SecretViewerView),
    NewSecret(NewSecretView),
    Envs(EnvsView),
    RemoteAdd(RemoteAddView),
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
    /// Buffer of chord steps already pressed but not yet resolved. Set by
    /// [`KeyMap::dispatch`] returning [`Dispatch::Pending`]; cleared on the
    /// next match, abort, or non-chord keypress.
    pending_chord: Vec<KeyEvent>,
    /// `true` while the active toast is the chord-progress breadcrumb.
    /// Tracked explicitly so [`Self::dismiss_chord_breadcrumb`] doesn't
    /// have to inspect the toast's text — an unrelated info toast that
    /// happens to land during a pending chord must not be cleared by
    /// breadcrumb dismissal.
    chord_breadcrumb_active: bool,
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
            pending_chord: Vec::new(),
            chord_breadcrumb_active: false,
        }
    }

    /// Publish a transient status-line message. Replaces any previous
    /// toast (rapid actions don't stack) and resets the 3-second TTL.
    /// Any active chord breadcrumb is also cleared — a normal toast
    /// supersedes the chord prompt.
    pub fn push_toast(&mut self, msg: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast::new(msg, kind));
        self.chord_breadcrumb_active = false;
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<AppIntent> {
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

        // ── Leader-key chord dispatcher ───────────────────────────────
        // Drives the multi-step chord state machine. If the key is part
        // of an in-flight chord (or starts one), it's swallowed here.
        // Only `Unmatched` falls through to the legacy per-key flow.
        match self.keymap.dispatch(&self.pending_chord, &key) {
            Dispatch::Match(action) => {
                self.pending_chord.clear();
                self.dismiss_chord_breadcrumb();
                return self.run_keymap_action(action);
            }
            Dispatch::Pending => {
                self.pending_chord.push(key);
                self.show_chord_breadcrumb();
                return None;
            }
            Dispatch::Unmatched => {
                if !self.pending_chord.is_empty() {
                    // The pending chord aborted because this key isn't a
                    // continuation. Surface the abort so the user knows
                    // their leader sequence didn't fire anything.
                    let summary = format_pending(&self.pending_chord);
                    self.pending_chord.clear();
                    self.push_toast(
                        format!("chord aborted: {summary}"),
                        ToastKind::Info,
                    );
                    return None;
                }
            }
        }

        // ── Single-key fallthrough (no active chord) ──────────────────
        match &mut self.view {
            View::Search(search) => {
                let action = search.on_key(key, &self.keymap);
                self.handle_search_action(action)
            }
            View::SecretViewer(viewer) => {
                let action = viewer.on_key(key, &self.keymap);
                self.handle_secret_viewer_action(action)
            }
            View::Envs(envs) => {
                let action = envs.on_key(key, &self.keymap);
                self.handle_envs_action(action)
            }
            View::NewSecret(form) => {
                let action = form.on_key(key, &self.keymap);
                self.handle_new_secret_action(action)
            }
            View::RemoteAdd(form) => {
                let action = form.on_key(key, &self.keymap);
                self.handle_remote_add_action(action)
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

        match &mut self.view {
            View::Search(search) => {
                if let Some(action) = search.dispatch_action(action) {
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
            // Envs and RemoteAdd don't yet expose an action dispatcher;
            // their keymap-driven behaviour stays inside their `on_key`
            // for now. Falling through is fine — chord completion in
            // those views just no-ops, since none of their bindings are
            // multi-step by default.
            View::Envs(_) | View::RemoteAdd(_) => {}
        }
        None
    }

    fn handle_search_action(&mut self, action: SearchAction) -> Option<AppIntent> {
        match action {
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
            SearchAction::AddRemote => {
                self.view = View::RemoteAdd(RemoteAddView::new(&self.ctx));
            }
            SearchAction::OpenEnvs => {
                self.view = View::Envs(EnvsView::new(&self.ctx));
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
                self.view = View::Search(SearchView::new(&self.ctx));
                self.push_toast("deleted", ToastKind::Success);
            }
        }
        None
    }

    fn handle_envs_action(&mut self, action: EnvsAction) -> Option<AppIntent> {
        match action {
            EnvsAction::None => {}
            EnvsAction::Quit => self.should_quit = true,
            EnvsAction::Back => {
                self.view = View::Search(SearchView::new(&self.ctx));
            }
            EnvsAction::Deleted { label, scope } => {
                let scope_str = match scope {
                    crate::config::env_cache::Scope::Project => "project",
                    crate::config::env_cache::Scope::Global => "global",
                };
                self.push_toast(
                    format!("deleted `{label}` ({scope_str})"),
                    ToastKind::Success,
                );
            }
            EnvsAction::DeleteFailed(msg) => {
                self.push_toast(msg, ToastKind::Error);
            }
            EnvsAction::Created { label, scope } => {
                let scope_str = match scope {
                    crate::config::env_cache::Scope::Project => "project",
                    crate::config::env_cache::Scope::Global => "global",
                };
                self.push_toast(
                    format!("created `{label}` ({scope_str})"),
                    ToastKind::Success,
                );
            }
            EnvsAction::CreateFailed(msg) => {
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
        }
        None
    }

    fn handle_remote_add_action(&mut self, action: RemoteAddAction) -> Option<AppIntent> {
        match action {
            RemoteAddAction::None => {}
            RemoteAddAction::Quit => self.should_quit = true,
            RemoteAddAction::Cancel => {
                self.view = View::Search(SearchView::new(&self.ctx));
                self.push_toast("add remote cancelled", ToastKind::Info);
            }
            RemoteAddAction::Created(slug) => {
                self.view = View::Search(SearchView::new(&self.ctx));
                self.push_toast(format!("added remote {slug}"), ToastKind::Success);
            }
            RemoteAddAction::Failed(err) => {
                self.view = View::Search(SearchView::new(&self.ctx));
                self.push_toast(format!("add remote failed: {err}"), ToastKind::Error);
            }
        }
        None
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
            View::Envs(_) => "envs",
            View::RemoteAdd(_) => "remote_add",
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
            View::Envs(envs) => envs.draw(frame),
            View::RemoteAdd(form) => form.draw(frame),
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
            View::Search(_) => HelpView::new(SearchView::help_entries(), SearchView::help_title()),
            View::SecretViewer(_) => HelpView::new(
                SecretViewerView::help_entries(),
                SecretViewerView::help_title(),
            ),
            View::NewSecret(_) => {
                HelpView::new(NewSecretView::help_entries(), NewSecretView::help_title())
            }
            View::Envs(_) => HelpView::new(EnvsView::help_entries(), EnvsView::help_title()),
            View::RemoteAdd(_) => {
                HelpView::new(RemoteAddView::help_entries(), RemoteAddView::help_title())
            }
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

fn clone_ctx(ctx: &Context) -> Context {
    Context {
        data_dir: ctx.data_dir.clone(),
        state_dir: ctx.state_dir.clone(),
        store: ctx.store.clone(),
        recipients_path: ctx.recipients_path.clone(),
    }
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
