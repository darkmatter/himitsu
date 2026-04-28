//! Global status-line toast for transient feedback (hm-o15).
//!
//! A [`Toast`] is a one-line message (saved / copied / deleted / error, …)
//! that renders in a reserved 1-row strip at the bottom of the terminal for
//! a few seconds and then clears automatically.
//!
//! Toasts are **not modal** — while a toast is on screen key events still go
//! to the active view. Pushing a new toast replaces any previous one, so
//! rapid-fire actions never stack.
//!
//! The router (`tui::app::App`) owns exactly one `Option<Toast>` and reserves
//! the bottom row of the draw area for it. Each view sees a slightly smaller
//! inner rect, which is why the toast lives at the app layer rather than
//! inside any particular view.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::theme;

/// Severity bucket for a toast. Drives the foreground colour and the
/// `[icon]` prefix on the rendered line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// Neutral informational message (e.g. "edit cancelled (no changes)").
    Info,
    /// Positive confirmation (e.g. "copied", "saved", "deleted").
    Success,
    /// Something failed — rendered in red so it stands out against the
    /// otherwise green/grey status bar.
    Error,
}

/// A transient status-line message with an expiry instant.
///
/// `Toast` is intentionally trivial to construct — the only time-aware bit
/// is `expires_at`, which [`Toast::is_expired`] compares against a caller-
/// supplied `now`. Tests avoid `sleep` by calling `expire_toast_now` on the
/// owning `App`, which rewrites `expires_at` to `Instant::now()`.
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub expires_at: Instant,
}

impl Toast {
    /// Default lifetime of a freshly pushed toast.
    pub const DEFAULT_TTL: Duration = Duration::from_secs(3);

    pub fn new(message: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            message: message.into(),
            kind,
            expires_at: Instant::now() + Self::DEFAULT_TTL,
        }
    }

    /// Has `now` reached or passed the expiry?
    ///
    /// Callers pass `Instant::now()` in production; tests drive expiry by
    /// mutating `expires_at` directly via the `expire_toast_now` test helper
    /// on `App`, so they never have to sleep.
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }

    /// Render this toast into a 1-row rect. Callers are responsible for
    /// reserving the row via `Layout`; this function only paints into it.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let (fg, tag) = match self.kind {
            ToastKind::Info => (theme::accent(), "[info] "),
            ToastKind::Success => (theme::success(), "[ok] "),
            ToastKind::Error => (theme::danger(), "[err] "),
        };
        let line = Line::from(vec![
            Span::styled(tag, Style::default().fg(fg).add_modifier(Modifier::BOLD)),
            Span::styled(self.message.clone(), Style::default().fg(fg)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }
}
