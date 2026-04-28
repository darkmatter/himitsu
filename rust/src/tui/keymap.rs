//! User-configurable TUI keybindings.
//!
//! The [`KeyMap`] struct holds one list of bindings per **action**, not per
//! key. Views consult the map through small `matches` helpers on
//! [`KeyBinding`] so a single action can be triggered by any number of
//! equivalent key combinations.
//!
//! [`KeyMap::default`] reproduces the hardcoded bindings that shipped before
//! this config existed, so users who never touch their config file see no
//! behaviour change. Users override individual actions by adding a
//! `tui.keys` section to `~/.config/himitsu/config.yaml`:
//!
//! ```yaml
//! tui:
//!   keys:
//!     new_secret: ["F2", "ctrl+n"]
//!     quit:       ["esc", "ctrl+q"]
//! ```
//!
//! Unknown entries fall back to their defaults; missing sections fall back
//! to [`KeyMap::default`]. Parsing happens at deserialisation time, so a
//! malformed binding string surfaces as a clear config error rather than a
//! silent no-op at runtime.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A single key combination: a [`KeyCode`] plus a set of [`KeyModifiers`].
///
/// Serialises/deserialises from strings like `"ctrl+n"`, `"shift+tab"`,
/// `"esc"`, `"?"`, `"F2"`. The canonical string form is
/// `<mod>+<mod>+<code>`, lower-cased, modifiers first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// Shortcut for a bare key with no modifiers.
    pub const fn bare(code: KeyCode) -> Self {
        Self::new(code, KeyModifiers::NONE)
    }

    /// Shortcut for `Ctrl+<ch>`.
    pub const fn ctrl(ch: char) -> Self {
        Self::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    /// Does this binding match a live `KeyEvent`?
    ///
    /// For printable characters we compare case-insensitively when no
    /// `SHIFT` modifier is declared, so a binding like `"y"` matches both
    /// `y` and `Y` without the user having to enumerate both forms.
    /// `Shift+<char>` explicitly requires the uppercase form.
    pub fn matches(&self, key: &KeyEvent) -> bool {
        // Mask away modifiers we don't track (e.g. META) so cross-platform
        // events still match cleanly.
        let tracked = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT;
        let event_mods = key.modifiers & tracked;

        match (self.code, key.code) {
            (KeyCode::Char(a), KeyCode::Char(b)) => {
                // Ascii-letter chars are treated case-insensitively and the
                // SHIFT flag is inferred from whichever form we see — so a
                // binding like `shift+r` matches both a raw
                // `Char('R') + SHIFT` event (common on macOS) and a
                // normalised `Char('r') + SHIFT` event.
                let self_lower = a.to_ascii_lowercase();
                let ev_lower = b.to_ascii_lowercase();
                if !a.eq_ignore_ascii_case(&b) {
                    return false;
                }

                // Fold the uppercase-char shortcut into an explicit SHIFT so
                // comparisons are purely modifier-based.
                let mut self_mods = self.modifiers;
                if a.is_ascii_uppercase() {
                    self_mods |= KeyModifiers::SHIFT;
                }
                let mut ev_mods = event_mods;
                if b.is_ascii_uppercase() {
                    ev_mods |= KeyModifiers::SHIFT;
                }

                if self_mods.contains(KeyModifiers::SHIFT) {
                    // Exact modifier match — shift is load-bearing.
                    let _ = (self_lower, ev_lower);
                    ev_mods == self_mods
                } else {
                    // Shift-insensitive: a bare `y` binding still matches
                    // `Y` / `Shift+y`, but ctrl/alt must still agree.
                    let strip = KeyModifiers::SHIFT;
                    (self_mods & !strip) == (ev_mods & !strip)
                }
            }
            // BackTab on one side is only equivalent to `Tab + Shift` on
            // the other — never to a bare Tab. This lets a binding written
            // as `"backtab"` match a terminal that produces
            // `Tab + KeyModifiers::SHIFT` without collapsing `Tab` and
            // `BackTab` into the same key.
            (KeyCode::BackTab, KeyCode::Tab) => {
                event_mods.contains(KeyModifiers::SHIFT)
                    && (event_mods & !KeyModifiers::SHIFT)
                        == (self.modifiers & !KeyModifiers::SHIFT)
            }
            (KeyCode::Tab, KeyCode::BackTab) => {
                self.modifiers.contains(KeyModifiers::SHIFT)
                    && (event_mods & !KeyModifiers::SHIFT)
                        == (self.modifiers & !KeyModifiers::SHIFT)
            }
            (KeyCode::BackTab, KeyCode::BackTab) => {
                // BackTab always implies shift; treat the SHIFT modifier as
                // optional so a `"backtab"` binding still matches a raw
                // `BackTab + NONE` event.
                let strip = KeyModifiers::SHIFT;
                (self.modifiers & !strip) == (event_mods & !strip)
            }
            (a, b) => a == b && event_mods == self.modifiers,
        }
    }
}

impl fmt::Display for KeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<&str> = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("alt");
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("shift");
        }
        let code = code_to_string(self.code);
        if parts.is_empty() {
            write!(f, "{code}")
        } else {
            write!(f, "{}+{}", parts.join("+"), code)
        }
    }
}

fn code_to_string(code: KeyCode) -> String {
    match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "backtab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::Null => "null".to_string(),
        KeyCode::CapsLock => "capslock".to_string(),
        KeyCode::ScrollLock => "scrolllock".to_string(),
        KeyCode::NumLock => "numlock".to_string(),
        KeyCode::PrintScreen => "printscreen".to_string(),
        KeyCode::Pause => "pause".to_string(),
        KeyCode::Menu => "menu".to_string(),
        KeyCode::KeypadBegin => "keypadbegin".to_string(),
        KeyCode::Media(_) | KeyCode::Modifier(_) => "unsupported".to_string(),
    }
}

impl std::str::FromStr for KeyBinding {
    type Err = KeyBindingParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.is_empty() {
            return Err(KeyBindingParseError::Empty);
        }
        // A lone '+' is itself a legal keycode ("plus"), so don't split it
        // away if it's the only character.
        if input == "+" {
            return Ok(KeyBinding::bare(KeyCode::Char('+')));
        }

        // Split on '+', but if the final segment is empty it means the
        // binding itself ends with a literal '+' (e.g. "ctrl++"). Recover
        // that as a trailing '+' keycode segment.
        let raw: Vec<&str> = input.split('+').collect();
        let (mod_parts, code_part): (Vec<&str>, &str) = if raw.last() == Some(&"") {
            // Trailing '+': everything except the last empty piece is the
            // modifier stack; the code is literally '+'.
            let modifiers = raw[..raw.len() - 1].to_vec();
            (modifiers[..modifiers.len().saturating_sub(1)].to_vec(), "+")
        } else {
            let last = *raw.last().unwrap();
            (raw[..raw.len() - 1].to_vec(), last)
        };

        let mut modifiers = KeyModifiers::NONE;
        for part in &mod_parts {
            let normalised = part.trim().to_ascii_lowercase();
            match normalised.as_str() {
                "ctrl" | "control" | "c" => modifiers |= KeyModifiers::CONTROL,
                "alt" | "meta" | "opt" | "option" | "a" => modifiers |= KeyModifiers::ALT,
                "shift" | "s" => modifiers |= KeyModifiers::SHIFT,
                "" => return Err(KeyBindingParseError::EmptyModifier(input.to_string())),
                _ => {
                    return Err(KeyBindingParseError::UnknownModifier {
                        input: input.to_string(),
                        modifier: part.to_string(),
                    })
                }
            }
        }

        let code = parse_code(code_part).ok_or_else(|| KeyBindingParseError::UnknownCode {
            input: input.to_string(),
            code: code_part.to_string(),
        })?;

        // A single-character binding like "Y" implicitly carries shift; we
        // prefer the canonical lowercase-char + SHIFT form so later matching
        // is consistent.
        let (code, modifiers) = match code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => (
                KeyCode::Char(c.to_ascii_lowercase()),
                modifiers | KeyModifiers::SHIFT,
            ),
            other => (other, modifiers),
        };

        Ok(KeyBinding { code, modifiers })
    }
}

fn parse_code(s: &str) -> Option<KeyCode> {
    let lower = s.trim().to_ascii_lowercase();
    match lower.as_str() {
        "enter" | "return" | "ret" => Some(KeyCode::Enter),
        "esc" | "escape" => Some(KeyCode::Esc),
        "tab" => Some(KeyCode::Tab),
        "backtab" => Some(KeyCode::BackTab),
        "backspace" | "bs" => Some(KeyCode::Backspace),
        "space" | "spc" => Some(KeyCode::Char(' ')),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "pgdown" | "pgdn" => Some(KeyCode::PageDown),
        "delete" | "del" => Some(KeyCode::Delete),
        "insert" | "ins" => Some(KeyCode::Insert),
        _ => {
            // Function keys: f1..f24
            if let Some(rest) = lower.strip_prefix('f') {
                if let Ok(n) = rest.parse::<u8>() {
                    if (1..=24).contains(&n) {
                        return Some(KeyCode::F(n));
                    }
                }
            }
            // Single char (case-preserving from the original input)
            let mut chars = s.chars();
            let first = chars.next()?;
            if chars.next().is_none() {
                return Some(KeyCode::Char(first));
            }
            None
        }
    }
}

/// Errors returned when parsing a key-binding string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyBindingParseError {
    Empty,
    EmptyModifier(String),
    UnknownModifier { input: String, modifier: String },
    UnknownCode { input: String, code: String },
}

impl fmt::Display for KeyBindingParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty key binding string"),
            Self::EmptyModifier(input) => {
                write!(f, "empty modifier segment in binding '{input}'")
            }
            Self::UnknownModifier { input, modifier } => write!(
                f,
                "unknown modifier '{modifier}' in binding '{input}' \
                 (expected one of: ctrl, alt, shift)"
            ),
            Self::UnknownCode { input, code } => {
                write!(f, "unknown key code '{code}' in binding '{input}'")
            }
        }
    }
}

impl std::error::Error for KeyBindingParseError {}

impl Serialize for KeyBinding {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeyBinding {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Helper so views can take either a `&Vec<KeyBinding>` or `&[KeyBinding]`
/// and test against a live `KeyEvent` with `.matches(&key)`.
pub trait Bindings {
    fn matches(&self, key: &KeyEvent) -> bool;
}

impl Bindings for Vec<KeyBinding> {
    fn matches(&self, key: &KeyEvent) -> bool {
        self.iter().any(|b| b.matches(key))
    }
}

impl Bindings for [KeyBinding] {
    fn matches(&self, key: &KeyEvent) -> bool {
        self.iter().any(|b| b.matches(key))
    }
}

/// User-configurable keybindings grouped by action.
///
/// Each field is a list so multiple key combinations can map to the same
/// action. Unspecified fields fall back to [`KeyMap::default`], which
/// reproduces the original hardcoded bindings.
///
/// Actions are grouped loosely by the view that consumes them, but there is
/// no enforcement — a single binding list can be reused across views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyMap {
    // ── Global ────────────────────────────────────────────────────────
    /// Quit the app from any view (default: Esc, Ctrl+C).
    pub quit: Vec<KeyBinding>,
    /// Open the contextual help overlay (default: `?`).
    pub help: Vec<KeyBinding>,

    // ── Search view ──────────────────────────────────────────────────
    /// Open the command palette overlay (default: Ctrl+P). The palette is
    /// the canonical way to discover and run commands; individual hotkeys
    /// remain as power-user shortcuts.
    pub command_palette: Vec<KeyBinding>,
    /// Open the new-secret form from the search view (default: Ctrl+N).
    pub new_secret: Vec<KeyBinding>,
    /// Open the store-picker overlay (default: Ctrl+S).
    pub switch_store: Vec<KeyBinding>,
    /// Copy the selected search result's value to the clipboard (default: Ctrl+Y).
    pub copy_selected: Vec<KeyBinding>,
    /// Open the envs view (browse/delete preset env labels) from search
    /// (default: Shift+E).
    pub envs: Vec<KeyBinding>,

    // ── Secret viewer ────────────────────────────────────────────────
    /// Reveal / hide the decrypted value (default: `r`).
    pub reveal: Vec<KeyBinding>,
    /// Copy the revealed value to the clipboard (default: `y`).
    pub copy_value: Vec<KeyBinding>,
    /// Rekey the secret (default: `R` — shift+r).
    pub rekey: Vec<KeyBinding>,
    /// Open the external editor on the current secret (default: `e`).
    pub edit: Vec<KeyBinding>,
    /// Enter the confirm-delete overlay (default: `d`).
    pub delete: Vec<KeyBinding>,
    /// Go back to the parent view from the secret viewer (default: Esc).
    pub back: Vec<KeyBinding>,

    // ── New-secret form ───────────────────────────────────────────────
    /// Save the new secret (default: Ctrl+S, Ctrl+W).
    pub save_secret: Vec<KeyBinding>,
    /// Advance to the next form field (default: Tab).
    pub next_field: Vec<KeyBinding>,
    /// Return to the previous form field (default: Shift+Tab).
    pub prev_field: Vec<KeyBinding>,
    /// Cancel the new-secret form and return to search (default: Esc).
    pub cancel: Vec<KeyBinding>,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self {
            quit: vec![KeyBinding::bare(KeyCode::Esc), KeyBinding::ctrl('c')],
            help: vec![KeyBinding::bare(KeyCode::Char('?'))],

            command_palette: vec![KeyBinding::ctrl('p')],
            new_secret: vec![KeyBinding::ctrl('n')],
            switch_store: vec![KeyBinding::ctrl('s')],
            copy_selected: vec![KeyBinding::ctrl('y')],
            envs: vec![KeyBinding::new(KeyCode::Char('e'), KeyModifiers::SHIFT)],

            reveal: vec![KeyBinding::bare(KeyCode::Char('r'))],
            copy_value: vec![KeyBinding::bare(KeyCode::Char('y'))],
            rekey: vec![KeyBinding::new(KeyCode::Char('r'), KeyModifiers::SHIFT)],
            edit: vec![KeyBinding::bare(KeyCode::Char('e'))],
            delete: vec![KeyBinding::bare(KeyCode::Char('d'))],
            back: vec![KeyBinding::bare(KeyCode::Esc)],

            save_secret: vec![KeyBinding::ctrl('s'), KeyBinding::ctrl('w')],
            next_field: vec![KeyBinding::bare(KeyCode::Tab)],
            prev_field: vec![KeyBinding::bare(KeyCode::BackTab)],
            cancel: vec![KeyBinding::bare(KeyCode::Esc)],
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parses_ctrl_n() {
        let b: KeyBinding = "ctrl+n".parse().unwrap();
        assert_eq!(b, KeyBinding::ctrl('n'));
    }

    #[test]
    fn parses_shift_tab() {
        let b: KeyBinding = "shift+tab".parse().unwrap();
        assert_eq!(b, KeyBinding::new(KeyCode::Tab, KeyModifiers::SHIFT));
    }

    #[test]
    fn parses_bare_question_mark() {
        let b: KeyBinding = "?".parse().unwrap();
        assert_eq!(b, KeyBinding::bare(KeyCode::Char('?')));
    }

    #[test]
    fn parses_function_key() {
        let b: KeyBinding = "F2".parse().unwrap();
        assert_eq!(b, KeyBinding::bare(KeyCode::F(2)));
    }

    #[test]
    fn parses_uppercase_char_as_shift() {
        let b: KeyBinding = "Y".parse().unwrap();
        assert_eq!(b, KeyBinding::new(KeyCode::Char('y'), KeyModifiers::SHIFT));
    }

    #[test]
    fn rejects_unknown_modifier() {
        let err = "ctrl+ctrl+foo".parse::<KeyBinding>().unwrap_err();
        assert!(matches!(err, KeyBindingParseError::UnknownCode { .. }));

        let err = "hyper+n".parse::<KeyBinding>().unwrap_err();
        assert!(matches!(err, KeyBindingParseError::UnknownModifier { .. }));
    }

    #[test]
    fn rejects_empty() {
        let err = "".parse::<KeyBinding>().unwrap_err();
        assert!(matches!(err, KeyBindingParseError::Empty));
    }

    #[test]
    fn matches_case_insensitive_char_without_shift() {
        let b = KeyBinding::bare(KeyCode::Char('y'));
        assert!(b.matches(&key(KeyCode::Char('y'), KeyModifiers::NONE)));
        assert!(b.matches(&key(KeyCode::Char('Y'), KeyModifiers::SHIFT)));
    }

    #[test]
    fn matches_respects_explicit_shift() {
        let b = KeyBinding::new(KeyCode::Char('r'), KeyModifiers::SHIFT);
        assert!(b.matches(&key(KeyCode::Char('R'), KeyModifiers::SHIFT)));
        assert!(!b.matches(&key(KeyCode::Char('r'), KeyModifiers::NONE)));
    }

    #[test]
    fn matches_respects_ctrl() {
        let b = KeyBinding::ctrl('n');
        assert!(b.matches(&key(KeyCode::Char('n'), KeyModifiers::CONTROL)));
        assert!(!b.matches(&key(KeyCode::Char('n'), KeyModifiers::NONE)));
    }

    #[test]
    fn default_keymap_has_expected_entries() {
        let km = KeyMap::default();
        assert!(km
            .new_secret
            .matches(&key(KeyCode::Char('n'), KeyModifiers::CONTROL)));
        assert!(km.quit.matches(&key(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(km
            .reveal
            .matches(&key(KeyCode::Char('r'), KeyModifiers::NONE)));
        assert!(km
            .rekey
            .matches(&key(KeyCode::Char('R'), KeyModifiers::SHIFT)));
    }

    #[test]
    fn roundtrip_yaml_default_equals_default() {
        let yaml = serde_yaml::to_string(&KeyMap::default()).unwrap();
        let back: KeyMap = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, KeyMap::default());
    }

    #[test]
    fn yaml_override_layers_over_default() {
        let yaml = r#"
new_secret: ["F2"]
"#;
        let km: KeyMap = serde_yaml::from_str(yaml).unwrap();
        assert!(km
            .new_secret
            .matches(&key(KeyCode::F(2), KeyModifiers::NONE)));
        assert!(!km
            .new_secret
            .matches(&key(KeyCode::Char('n'), KeyModifiers::CONTROL)));
        // Unspecified actions still match the default.
        assert!(km.quit.matches(&key(KeyCode::Esc, KeyModifiers::NONE)));
    }

    #[test]
    fn yaml_malformed_binding_is_rejected() {
        let yaml = r#"
new_secret: ["ctrl+ctrl+foo"]
"#;
        let err = serde_yaml::from_str::<KeyMap>(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("ctrl+ctrl+foo"),
            "expected parse error to mention the bad binding, got: {msg}"
        );
    }
}
