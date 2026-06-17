//! Named layout constants and shared rect helpers for the TUI.
//!
//! Centralises the design-decision values that control the visual dimensions
//! of views and overlays.  A developer who wants to adjust the TUI's
//! appearance should only need to change values in this file.
//!
//! ## Groups
//!
//! - **Standard canvas** — the centred, min/max size-bounded drawing area that every
//!   view renders into.
//! - **Chrome heights** — single-row chrome shared across views (header bars,
//!   footer rows, spacers).
//! - **Form field heights** — row heights for input widgets in form-style views.
//! - **Popups / overlays** — dimensions for modal dialogs and centred overlays.
//! - **Header columns** — minimum widths for split header columns.

// ── Standard canvas ───────────────────────────────────────────────────────────

/// Horizontal (and vertical) margin subtracted from each axis before
/// capping at [`CANVAS_MAX_WIDTH`] / [`CANVAS_MAX_HEIGHT`].  Applied on
/// both sides, so the effective total gap on each axis is `CANVAS_MARGIN * 2`.
pub const CANVAS_MARGIN: u16 = 4;

/// Maximum width (in columns) of the standard drawing canvas.
pub const CANVAS_MAX_WIDTH: u16 = 80;

/// Maximum height (in rows) of the standard drawing canvas.
pub const CANVAS_MAX_HEIGHT: u16 = 30;

/// Minimum width (in columns) of the standard drawing canvas. Applied when the
/// terminal has room; on terminals narrower than this the canvas uses the full
/// width (see [`standard_canvas`]).
pub const CANVAS_MIN_WIDTH: u16 = 40;

/// Minimum height (in rows) of the standard drawing canvas. Applied when the
/// terminal has room; on terminals shorter than this the canvas uses the full
/// height.
pub const CANVAS_MIN_HEIGHT: u16 = 10;

/// Minimum terminal height (in rows) required to launch the TUI.
/// Below this the UI chrome (header, search input, footer) cannot fit.
pub const MIN_TERMINAL_HEIGHT: u16 = 12;

/// Minimum terminal width (in columns) required to launch the TUI.
pub const MIN_TERMINAL_WIDTH: u16 = 40;

// ── Chrome heights ────────────────────────────────────────────────────────────

/// Height of the single-row header bar rendered at the top of most views
/// (brand chip + active-view name + health pills).
pub const HEADER_HEIGHT: u16 = 1;

/// Height of the single-row footer bar rendered at the bottom of most views
/// (key-hint strip or status message).
pub const FOOTER_HEIGHT: u16 = 1;

/// Height of an empty spacer row used to add breathing room between chrome
/// sections (e.g. between header and content in the search view).
pub const SPACER_HEIGHT: u16 = 0;

// ── Form field heights ────────────────────────────────────────────────────────

/// Height of a bordered single-line text input widget as used in form-style
/// views (new-secret form, store-picker input, etc.).  Includes the border
/// rows, so the usable content area is `FORM_FIELD_HEIGHT - 2`.
pub const FORM_FIELD_HEIGHT: u16 = 3;

/// Height of the search-query input box in the search view.
/// Same as [`FORM_FIELD_HEIGHT`] — defined separately so the search view's
/// constraints read as a deliberate choice rather than a re-used form constant.
pub const SEARCH_INPUT_HEIGHT: u16 = 3;

/// Height of the two-row status/footer area at the bottom of the store
/// picker overlay (error message or key-hint strip).
pub const PICKER_FOOTER_HEIGHT: u16 = 2;

/// Header block height in the init wizard (title text with one blank row
/// above the body border).
pub const WIZARD_HEADER_HEIGHT: u16 = 2;

// ── Popup / overlay dimensions ────────────────────────────────────────────────

/// Width of the "unsaved changes" confirm dialog in the new-secret form.
/// Kept compact so it is readable even on narrow terminals.
pub const CONFIRM_POPUP_WIDTH: u16 = 50;

/// Height of the "unsaved changes" confirm dialog in the new-secret form.
pub const CONFIRM_POPUP_HEIGHT: u16 = 7;

/// Percentage of the terminal width occupied by the command-palette overlay.
pub const PALETTE_WIDTH_PCT: u16 = 60;

/// Percentage of the terminal height occupied by the command-palette overlay.
pub const PALETTE_HEIGHT_PCT: u16 = 50;

// ── Header column minimum widths ─────────────────────────────────────────────

/// Minimum width (in columns) for the left section of the search-view header
/// (brand chip + view name).  Prevents the right-hand health pills from
/// crowding out the brand when the terminal is narrow.
pub const HEADER_LEFT_MIN_WIDTH: u16 = 20;

use ratatui::layout::{Constraint, Flex, Layout, Rect};

/// The centred, margin-inset, size-bounded drawing area shared by most views.
pub fn standard_canvas(area: Rect) -> Rect {
    centered_bounded_rect(
        area,
        CANVAS_MIN_WIDTH,
        CANVAS_MIN_HEIGHT,
        CANVAS_MAX_WIDTH,
        CANVAS_MAX_HEIGHT,
        CANVAS_MARGIN,
    )
}

/// Centre `area` within [`min_*`, `max_*`] on each axis after optionally
/// insetting by `margin`. When the margin-inset region is smaller than
/// `min_*` but the full terminal can fit the minimum, the canvas expands into
/// the margin zone. When the terminal is too small for the margin on an axis,
/// that margin is skipped (ratatui's [`Rect::inner`] would collapse to zero
/// instead).
pub fn centered_bounded_rect(
    area: Rect,
    min_width: u16,
    min_height: u16,
    max_width: u16,
    max_height: u16,
    margin: u16,
) -> Rect {
    let width = canvas_axis(area.width, margin, min_width, max_width);
    let height = canvas_axis(area.height, margin, min_height, max_height);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}

/// Centre a fixed-size rectangle within `area`, clamping width/height to the
/// available space.
pub fn centered_length_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}

/// Centre a rectangle occupying `percent_x` × `percent_y` percent of `area`.
pub fn centered_percent_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let [area] = Layout::vertical([Constraint::Percentage(percent_y)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn canvas_axis(full: u16, margin: u16, min: u16, max: u16) -> u16 {
    let inset = inset_axis(full, margin);
    let capped = inset.min(max);
    if capped >= min {
        capped
    } else if full >= min {
        min.min(max).min(full)
    } else {
        full
    }
}

fn inset_axis(full: u16, margin: u16) -> u16 {
    if full > margin.saturating_mul(2) {
        full.saturating_sub(margin.saturating_mul(2))
    } else {
        full
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_canvas_centers_odd_width() {
        let area = Rect::new(0, 0, 101, 40);
        let canvas = standard_canvas(area);
        assert_eq!(canvas, Rect::new(11, 5, 80, 30));
    }

    #[test]
    fn standard_canvas_preserves_margin_below_max_width() {
        let area = Rect::new(0, 0, 87, 35);
        let canvas = standard_canvas(area);
        assert_eq!(canvas, Rect::new(4, 4, 79, 27));
    }

    #[test]
    fn standard_canvas_expands_to_min_width_when_margin_inset_is_narrow() {
        let area = Rect::new(0, 0, 45, 20);
        let canvas = standard_canvas(area);
        assert_eq!(canvas, Rect::new(3, 4, 40, 12));
    }

    #[test]
    fn standard_canvas_uses_full_axis_when_margin_would_collapse() {
        let area = Rect::new(0, 0, 7, 6);
        let canvas = standard_canvas(area);
        assert_eq!(canvas, area);
    }

    #[test]
    fn centered_length_rect_clamps_and_centers() {
        let area = Rect::new(0, 0, 100, 20);
        let rect = centered_length_rect(area, 50, 7);
        assert_eq!(rect, Rect::new(25, 7, 50, 7));
    }

    #[test]
    fn centered_percent_rect_matches_legacy_three_chunk_split() {
        let area = Rect::new(0, 0, 100, 40);
        let rect = centered_percent_rect(area, 60, 50);
        assert_eq!(rect, Rect::new(20, 10, 60, 20));
    }
}
