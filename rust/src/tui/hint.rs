//! Ambient bottom-left hint strip (hm-isi).
//!
//! A [`Hint`] is a low-stakes, persistent piece of guidance text — the TUI
//! equivalent of a Vim modeline tip or an IDE status hint. Unlike a
//! [`crate::tui::toast::Toast`], a hint:
//!
//! - has no expiry (it persists until the owning view replaces or clears it),
//! - has no severity / colour / icon variants — just one muted, dimmed style,
//! - paints only the **left third** of the bottom strip so a toast can still
//!   own the right-aligned portion if both are simultaneously active.
//!
//! Toasts already cover transient feedback ("saved", "copied", "deleted"); the
//! hint lane is reserved for ambient context that helps the user understand
//! what the focused field expects.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::theme;

/// Hard cap on rendered hint length, in **characters** (not bytes). Anything
/// past this limit is truncated with a trailing `…` so the hint never spills
/// past its left-third strip even on narrow terminals.
pub const MAX_LEN: usize = 60;

/// A persistent ambient hint shown in the bottom-left of the frame.
///
/// Construction is intentionally trivial — there is no state besides the
/// message itself, because the hint lifecycle is owned by whichever view set
/// it (the view clears or replaces the hint when its focus changes).
#[derive(Debug, Clone)]
pub struct Hint {
    pub message: String,
}

impl Hint {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Render this hint into the **left third** of `area`, leaving the rest
    /// of the strip untouched so a coexisting toast can paint over the right
    /// portion. Caller is responsible for reserving the row via [`Layout`];
    /// this function never calls [`ratatui::widgets::Clear`].
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let strip_width = (area.width / 3).max(1);
        let strip = Rect {
            x: area.x,
            y: area.y,
            width: strip_width,
            height: 1,
        };

        let style = Style::default()
            .fg(theme::muted())
            .add_modifier(Modifier::DIM);
        // The whole line (message and the leading marker) shares the same
        // muted/dim style — there is no severity differentiation, that's the
        // toast's job.
        let line = Line::from(vec![Span::styled(
            truncate_for_render(&self.message),
            style,
        )]);
        frame.render_widget(Paragraph::new(line), strip);
    }
}

/// Truncate `message` to at most [`MAX_LEN`] characters, appending an ellipsis
/// when truncation occurred. Operates on `chars` (not bytes) so multi-byte
/// codepoints can never be sliced mid-character.
fn truncate_for_render(message: &str) -> String {
    let char_count = message.chars().count();
    if char_count <= MAX_LEN {
        return message.to_string();
    }
    // Reserve one char of budget for the trailing `…` so the rendered string
    // is at most MAX_LEN characters wide.
    let mut out: String = message.chars().take(MAX_LEN.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untruncated_when_shorter_than_limit() {
        let s = "short hint";
        assert_eq!(truncate_for_render(s), s);
    }

    #[test]
    fn untruncated_at_exactly_max_len() {
        let s = "a".repeat(MAX_LEN);
        let rendered = truncate_for_render(&s);
        assert_eq!(rendered.chars().count(), MAX_LEN);
        assert_eq!(rendered, s, "exactly MAX_LEN should not be truncated");
    }

    #[test]
    fn truncated_with_ellipsis_when_one_over_limit() {
        let s = "a".repeat(MAX_LEN + 1);
        let rendered = truncate_for_render(&s);
        assert_eq!(rendered.chars().count(), MAX_LEN);
        assert!(
            rendered.ends_with('…'),
            "truncated hint should end with ellipsis, got {rendered:?}"
        );
    }

    #[test]
    fn truncated_with_ellipsis_when_far_over_limit() {
        let s = "b".repeat(MAX_LEN * 4);
        let rendered = truncate_for_render(&s);
        assert_eq!(rendered.chars().count(), MAX_LEN);
        assert!(rendered.ends_with('…'));
    }

    #[test]
    fn multibyte_boundary_safe() {
        // A long string of 4-byte codepoints would panic with byte slicing
        // but must be cleanly truncated when slicing on chars.
        let s = "🦀".repeat(MAX_LEN + 5);
        let rendered = truncate_for_render(&s);
        assert_eq!(rendered.chars().count(), MAX_LEN);
        assert!(rendered.ends_with('…'));
        // Round-trip — ensure we produced valid UTF-8 (would have panicked
        // already if not, but keep an explicit assertion).
        assert!(rendered.is_char_boundary(rendered.len()));
    }

    #[test]
    fn multibyte_message_under_limit_unchanged() {
        let s = "· tip: 日本語 hello";
        assert_eq!(truncate_for_render(s), s);
    }

    #[test]
    fn render_into_zero_width_area_is_a_noop() {
        // We only need to verify it doesn't panic — the buffer is unchanged
        // because `render` returns early for zero-width areas.
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let h = Hint::new("· tip: anything");
                h.render(
                    frame,
                    Rect {
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 1,
                    },
                );
            })
            .unwrap();
    }

    #[test]
    fn render_paints_only_left_third_of_strip() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let backend = TestBackend::new(30, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let h = Hint::new("hint!");
                h.render(
                    frame,
                    Rect {
                        x: 0,
                        y: 0,
                        width: 30,
                        height: 1,
                    },
                );
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        // Left third = 10 cells. The hint text "hint!" should land in the
        // first 5 cells; cells beyond the strip width must remain blank so
        // a coexisting toast can claim them.
        let mut left = String::new();
        for x in 0..10 {
            left.push_str(buf[(x, 0)].symbol());
        }
        assert!(left.starts_with("hint!"), "left strip = {left:?}");
        for x in 10..30 {
            assert_eq!(
                buf[(x, 0)].symbol(),
                " ",
                "cell {x} should be untouched by hint render"
            );
        }
    }
}
