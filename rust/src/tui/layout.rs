//! Named layout constants for the TUI.
//!
//! Centralises the design-decision values that control the visual dimensions
//! of views and overlays.  A developer who wants to adjust the TUI's
//! appearance should only need to change values in this file.
//!
//! ## Groups
//!
//! - **Standard canvas** — the centred, size-capped drawing area that every
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
