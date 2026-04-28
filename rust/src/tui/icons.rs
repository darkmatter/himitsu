//! Glyph table for TUI chrome.
//!
//! There is no reliable runtime check for Nerd Font support — if the
//! user's terminal lacks one of the patched fonts, Nerd Font code points
//! render as tofu boxes. We follow the convention starship / lazygit use:
//! ship a plain-Unicode default and let users opt in to the icon set via
//! `tui.nerd_fonts: true` in their config.
//!
//! The active set is stored in a global `OnceLock<RwLock<IconSet>>` and
//! configured once during TUI startup via [`set_use_nerd_fonts`]. After
//! that any view can read the relevant glyph through the accessor
//! functions.

use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Copy)]
struct IconSet {
    /// Status indicator for the store-health pill (synced/behind/dirty).
    /// Plain default: a filled bullet that renders cleanly on every
    /// terminal. Nerd Font: the git logo glyph.
    health: &'static str,
}

impl IconSet {
    const fn plain() -> Self {
        Self { health: "●" }
    }

    const fn nerd() -> Self {
        // U+E702 — Nerd Fonts "Pomicons" git logo. Common across all
        // patched font variants and renders at single width.
        Self { health: "\u{e702}" }
    }
}

static ACTIVE: OnceLock<RwLock<IconSet>> = OnceLock::new();

fn active() -> &'static RwLock<IconSet> {
    ACTIVE.get_or_init(|| RwLock::new(IconSet::plain()))
}

/// Toggle whether to use Nerd Font glyphs for the rest of the session.
pub(crate) fn set_use_nerd_fonts(enabled: bool) {
    *active().write().expect("icon table lock poisoned") = if enabled {
        IconSet::nerd()
    } else {
        IconSet::plain()
    };
}

fn current() -> IconSet {
    *active().read().expect("icon table lock poisoned")
}

/// Glyph used in the search-view store-health pill.
pub(crate) fn health() -> &'static str {
    current().health
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_plain_bullet() {
        // Reset just in case a previous test flipped the state.
        set_use_nerd_fonts(false);
        assert_eq!(health(), "●");
    }

    #[test]
    fn nerd_fonts_swap_glyphs() {
        set_use_nerd_fonts(true);
        assert_ne!(health(), "●");
        // Restore the default for any test that runs after this one in
        // the same process.
        set_use_nerd_fonts(false);
    }
}
