//! Centralized color palettes for the TUI.
//!
//! All ratatui [`Color`] values used by views live here so the visual
//! identity is maintainable from one place. Use the semantic accessors
//! (e.g. [`accent`], [`muted`]) rather than referencing raw [`Color`]
//! values directly in view code.

use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::error::{HimitsuError, Result};

/// Concrete themes the `random` selector can pick from. Kept in sync with
/// the entries in [`Palette::named`] minus the `random`/`default` aliases.
const RANDOM_POOL: &[&str] = &[
    "himitsu",
    "apathy",
    "apathy-minted",
    "apathy-theory",
    "apathy-storm",
    "ayu",
    "catppuccin",
    "material",
    "rose-pine",
];

#[derive(Debug, Clone, Copy)]
pub(crate) struct Palette {
    pub background: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub muted: Color,
    pub primary: Color,
    pub neutral: Color,
    pub success: Color,
    pub border: Color,
    pub border_label: Color,
    pub footer_text: Color,
    pub warning: Color,
    pub danger: Color,
}

#[derive(Debug, Clone)]
struct ActiveTheme {
    /// Concrete theme name actually in use (random/default already resolved).
    name: &'static str,
    palette: Palette,
}

static ACTIVE_PALETTE: OnceLock<RwLock<ActiveTheme>> = OnceLock::new();

fn active_palette() -> &'static RwLock<ActiveTheme> {
    ACTIVE_PALETTE.get_or_init(|| {
        RwLock::new(ActiveTheme {
            name: "himitsu",
            palette: Palette::himitsu(),
        })
    })
}

fn current() -> Palette {
    active_palette()
        .read()
        .expect("active TUI palette lock poisoned")
        .palette
}

/// Name of the currently active theme. After resolving `random`/`default`,
/// this is the concrete theme that was actually loaded.
pub(crate) fn current_theme_name() -> &'static str {
    active_palette()
        .read()
        .expect("active TUI palette lock poisoned")
        .name
}

/// Select a built-in TUI theme by name.
pub(crate) fn set_theme(name: &str) -> Result<()> {
    let resolved = resolve_theme_name(name)?;
    let palette = Palette::named(resolved)?;
    *active_palette()
        .write()
        .expect("active TUI palette lock poisoned") = ActiveTheme {
        name: resolved,
        palette,
    };
    Ok(())
}

/// Resolve `random`/`default` aliases into a concrete theme name from
/// [`RANDOM_POOL`]; pass other known names through unchanged.
fn resolve_theme_name(name: &str) -> Result<&'static str> {
    match normalize_name(name).as_str() {
        "random" | "default" => Ok(pick_random_theme_name()),
        other => RANDOM_POOL
            .iter()
            .copied()
            .find(|candidate| {
                normalize_name(candidate) == other
                    || matches!(
                        (other, *candidate),
                        ("minted", "apathy-minted")
                            | ("theory", "apathy-theory")
                            | ("apathy-ocean" | "apathetic-ocean" | "storm", "apathy-storm")
                            | ("ayu-dark", "ayu")
                            | ("catppuccin-mocha" | "mocha", "catppuccin")
                            | ("material-ocean", "material")
                            | ("rosepine" | "rosé-pine", "rose-pine")
                    )
            })
            .ok_or_else(|| {
                HimitsuError::InvalidConfig(format!(
                    "unknown TUI theme '{name}'; expected one of: {}",
                    available_themes().join(", ")
                ))
            }),
    }
}

/// Names accepted by [`set_theme`].
pub(crate) fn available_themes() -> &'static [&'static str] {
    &[
        "random",
        "himitsu",
        "apathy",
        "apathy-minted",
        "apathy-theory",
        "apathy-storm",
        "ayu",
        "catppuccin",
        "material",
        "rose-pine",
    ]
}

/// Pick one entry from [`RANDOM_POOL`] using a cheap nanosecond-based seed.
/// Good enough for "surprise me on startup"; not used for anything where
/// real randomness matters.
fn pick_random_theme_name() -> &'static str {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let idx = (nanos as usize) % RANDOM_POOL.len();
    RANDOM_POOL[idx]
}

/// Theme background color painted across the whole TUI frame. Use
/// [`Color::Reset`] to inherit the terminal's native background.
pub(crate) fn background() -> Color {
    current().background
}

/// Render `label` as a single accent-background chip with one cell of
/// horizontal padding on either side. Returns a one-span vector so callers
/// can extend the surrounding [`Line`] without special-casing pill vs
/// non-pill output.
pub(crate) fn brand_chip(label: &str) -> Vec<Span<'_>> {
    chip(label, accent(), on_accent(), true)
}

/// Like [`brand_chip`] but with caller-chosen background and foreground;
/// used for status badges (sync state, toasts) where the chip color is
/// state-dependent.
pub(crate) fn pill_with(label: String, bg: Color, fg: Color) -> Vec<Span<'static>> {
    chip(&label, bg, fg, false)
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect()
}

fn chip(label: &str, bg: Color, fg: Color, bold: bool) -> Vec<Span<'_>> {
    let mut style = Style::default().fg(fg).bg(bg);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    vec![Span::styled(format!(" {label} "), style)]
}

/// Highlight / hotkey / selected-row background. The dominant brand color.
pub(crate) fn accent() -> Color {
    current().accent
}

/// Foreground used on top of [`accent`] backgrounds (e.g. selected row text).
pub(crate) fn on_accent() -> Color {
    current().on_accent
}

/// Secondary / de-emphasized text (labels, hints, separators).
pub(crate) fn muted() -> Color {
    current().muted
}

/// Primary text on the default background. Rarely needed explicitly — most
/// widgets inherit the terminal default.
pub(crate) fn primary() -> Color {
    current().primary
}

/// Neutral text used when something is informational but not muted.
pub(crate) fn neutral() -> Color {
    current().neutral
}

/// Success / healthy state (e.g. synced store, info toast).
pub(crate) fn success() -> Color {
    current().success
}

/// Border color used for input fields and other UI elements.
pub(crate) fn border() -> Color {
    current().border
}

/// Labels embedded in borders / panel frames.
pub(crate) fn border_label() -> Color {
    current().border_label
}

/// Non-key text in footer help rows.
pub(crate) fn footer_text() -> Color {
    current().footer_text
}

/// Warning / pending state (e.g. behind remote, expiring soon).
pub(crate) fn warning() -> Color {
    current().warning
}

/// Error / destructive state (e.g. dirty store, expired secret, error toast).
pub(crate) fn danger() -> Color {
    current().danger
}

impl Palette {
    fn named(name: &str) -> Result<Self> {
        match normalize_name(name).as_str() {
            "random" | "default" => Self::named(pick_random_theme_name()),
            "himitsu" => Ok(Self::himitsu()),
            "apathy" => Ok(Self::apathy()),
            "apathy-minted" | "minted" => Ok(Self::apathy_minted()),
            "apathy-theory" | "theory" => Ok(Self::apathy_theory()),
            "apathy-storm" | "apathy-ocean" | "apathetic-ocean" | "storm" => {
                Ok(Self::apathy_storm())
            }
            "ayu" | "ayu-dark" => Ok(Self::ayu()),
            "catppuccin" | "catppuccin-mocha" | "mocha" => Ok(Self::catppuccin()),
            "material" | "material-ocean" => Ok(Self::material()),
            "rose-pine" | "rosepine" | "rosé-pine" => Ok(Self::rose_pine()),
            other => Err(HimitsuError::InvalidConfig(format!(
                "unknown TUI theme '{other}'; expected one of: {}",
                available_themes().join(", ")
            ))),
        }
    }

    fn himitsu() -> Self {
        Self {
            // Inherit terminal default; the himitsu theme is meant to be
            // background-agnostic so it looks right on any terminal scheme.
            background: Color::Reset,
            accent: rgb(103, 232, 249),
            on_accent: rgb(15, 23, 42),
            muted: rgb(148, 163, 184),
            primary: rgb(226, 232, 240),
            neutral: rgb(203, 213, 225),
            success: rgb(110, 231, 183),
            border: rgb(71, 85, 105),
            border_label: rgb(125, 211, 252),
            footer_text: rgb(125, 137, 158),
            warning: rgb(251, 191, 36),
            danger: rgb(248, 113, 113),
        }
    }

    fn apathy() -> Self {
        Self {
            background: hex(0x0b0a0d),
            accent: hex(0x33b3cc),
            on_accent: hex(0x0b0a0d),
            muted: hex(0x6d6d7c),
            primary: hex(0xe3e1e8),
            neutral: hex(0xb5b5b5),
            success: hex(0x47cf7e),
            border: hex(0x45414c),
            border_label: hex(0x93e3db),
            footer_text: hex(0x747277),
            warning: hex(0xe6986b),
            danger: hex(0xff6188),
        }
    }

    fn apathy_minted() -> Self {
        Self {
            background: hex(0x0f0d1a),
            accent: hex(0x61ffca),
            on_accent: hex(0x0f0d1a),
            muted: hex(0x6d6d7c),
            primary: hex(0xe6e6f1),
            neutral: hex(0xcbdbe0),
            success: hex(0xa1efe4),
            border: hex(0x45414c),
            border_label: hex(0x95d4ca),
            footer_text: hex(0x747277),
            warning: hex(0xffca85),
            danger: hex(0xff6767),
        }
    }

    fn apathy_theory() -> Self {
        Self {
            background: hex(0x0f0d1a),
            accent: hex(0xa277ff),
            on_accent: hex(0x0f0d1a),
            muted: hex(0x6d6d7c),
            primary: hex(0xe6e6f1),
            neutral: hex(0xc3c1d3),
            success: hex(0xb1d36d),
            border: hex(0x45414c),
            border_label: hex(0xc792ea),
            footer_text: hex(0x7d7a8b),
            warning: hex(0xffca85),
            danger: hex(0xff6188),
        }
    }

    fn apathy_storm() -> Self {
        Self {
            background: hex(0x0f0d1a),
            accent: hex(0x78dce8),
            on_accent: hex(0x0f0d1a),
            muted: hex(0x6d6d7c),
            primary: hex(0xe6e6f1),
            neutral: hex(0xcbdbe0),
            success: hex(0x95d4ca),
            border: hex(0x45414c),
            border_label: hex(0x82aaff),
            footer_text: hex(0x747277),
            warning: hex(0xffca85),
            danger: hex(0xff6767),
        }
    }

    fn ayu() -> Self {
        Self {
            background: hex(0x0b0e14),
            accent: hex(0x39bae6),
            on_accent: hex(0x0b0e14),
            muted: hex(0x626a73),
            primary: hex(0xb3b1ad),
            neutral: hex(0xacb6bf),
            success: hex(0xaad94c),
            border: hex(0x1f2430),
            border_label: hex(0xffcc66),
            footer_text: hex(0x6c7380),
            warning: hex(0xffb454),
            danger: hex(0xf07178),
        }
    }

    fn catppuccin() -> Self {
        Self {
            background: hex(0x1e1e2e),
            accent: hex(0x89dceb),
            on_accent: hex(0x11111b),
            muted: hex(0xa6adc8),
            primary: hex(0xcdd6f4),
            neutral: hex(0xbac2de),
            success: hex(0xa6e3a1),
            border: hex(0x45475a),
            border_label: hex(0xcba6f7),
            footer_text: hex(0x7f849c),
            warning: hex(0xf9e2af),
            danger: hex(0xf38ba8),
        }
    }

    fn material() -> Self {
        Self {
            background: hex(0x0f111a),
            accent: hex(0x89ddff),
            on_accent: hex(0x0f111a),
            muted: hex(0x546e7a),
            primary: hex(0xeeffff),
            neutral: hex(0xb2ccd6),
            success: hex(0xc3e88d),
            border: hex(0x2f3b54),
            border_label: hex(0x82aaff),
            footer_text: hex(0x697098),
            warning: hex(0xffcb6b),
            danger: hex(0xf07178),
        }
    }

    fn rose_pine() -> Self {
        Self {
            background: hex(0x191724),
            accent: hex(0x9ccfd8),
            on_accent: hex(0x191724),
            muted: hex(0x908caa),
            primary: hex(0xe0def4),
            neutral: hex(0xc4a7e7),
            success: hex(0x9ccfd8),
            border: hex(0x524f67),
            border_label: hex(0xebbcba),
            footer_text: hex(0x6e6a86),
            warning: hex(0xf6c177),
            danger: hex(0xeb6f92),
        }
    }
}

fn normalize_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('_', "-")
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

const fn hex(value: u32) -> Color {
    Color::Rgb(
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_named_theme_aliases() {
        assert!(Palette::named("catppuccin-mocha").is_ok());
        assert!(Palette::named("apathetic-ocean").is_ok());
        assert!(Palette::named("rose_pine").is_ok());
    }

    #[test]
    fn rejects_unknown_theme() {
        assert!(Palette::named("neon-nope").is_err());
    }

    #[test]
    fn random_alias_resolves_to_a_real_palette() {
        // Should never bubble up an error — the random pool is hard-coded
        // and every entry must be a name that `Palette::named` knows.
        for _ in 0..16 {
            assert!(Palette::named("random").is_ok());
        }
    }

    #[test]
    fn random_pool_entries_all_resolve() {
        for name in RANDOM_POOL {
            assert!(
                Palette::named(name).is_ok(),
                "random pool name `{name}` is not a known theme"
            );
        }
    }
}
