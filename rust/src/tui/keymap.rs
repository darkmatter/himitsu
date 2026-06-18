//! User-configurable TUI keybindings.
//!
//! The [`KeyMap`] struct holds one list of bindings per **action**, not per
//! key. Views consult the map through small `matches` helpers on
//! [`KeyBinding`] / [`KeyChord`] so a single action can be triggered by any
//! number of equivalent key combinations.
//!
//! [`KeyMap::default`] defines the built-in bindings used when no user
//! override exists. Users override individual actions by adding a
//! `tui.keys` section to `~/.config/himitsu/config.yaml`:
//!
//! ```yaml
//! tui:
//!   keys:
//!     new_secret: ["F2", "ctrl+n"]
//!     # Leader-key chord: press Ctrl+X, then s. Avoids terminal Ctrl+S
//!     # collisions (XOFF).
//!     save_secret: ["ctrl+x s"]
//!     quit: ["esc", "ctrl+q"]
//! ```
//!
//! ## Chord syntax
//!
//! A binding string is one or more chord steps separated by whitespace.
//! Each step uses the canonical `<mod>+<mod>+<code>` form. A single-step
//! binding (`"ctrl+n"`) is just a degenerate chord. Multi-step chords
//! enter "pending" state on their first step — see [`KeyMap::dispatch`].
//!
//! Unknown entries fall back to their defaults; missing sections fall back
//! to [`KeyMap::default`]. Parsing happens at deserialisation time, so a
//! malformed binding string surfaces as a clear config error rather than a
//! silent no-op at runtime.
//!
//! ## Leader-key chords
//!
//! [`LEADER`] (`ctrl+x`) is the only chord prefix. Multi-step bindings must
//! start with it; the leader cannot be bound as a standalone action. After
//! the leader is pressed, the next key within [`CHORD_TIMEOUT_MS`] completes
//! or aborts the chord; otherwise the pending sequence cancels silently.

use std::fmt;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Leader key for multi-step chords (`ctrl+x`). Every chord binding must
/// begin with this step; it cannot be used as a standalone binding.
pub const LEADER: KeyBinding = KeyBinding::ctrl('x');

/// Wall-clock window after the leader key during which the next keypress
/// completes the chord. Expiry clears the pending buffer silently.
pub const CHORD_TIMEOUT_MS: u64 = 1000;

/// [`Duration`] counterpart of [`CHORD_TIMEOUT_MS`] for deadline arithmetic.
pub const CHORD_TIMEOUT: Duration = Duration::from_millis(CHORD_TIMEOUT_MS);

/// A single chord step: a [`KeyCode`] plus a set of [`KeyModifiers`].
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

    /// Is this binding the chord leader (`ctrl+x`)?
    pub fn is_leader(self) -> bool {
        self.code == KeyCode::Char('x') && self.modifiers.contains(KeyModifiers::CONTROL)
    }

    /// Does this binding match a live `KeyEvent`?
    ///
    /// Ascii letters compare case-insensitively (`"y"`/`"Y"`, `"shift+y"`/
    /// `"shift+Y"`, `"ctrl+y"`/`"ctrl+Y"` are equivalent). Shift is never
    /// inferred from an uppercase letter — only an explicit `shift` modifier
    /// counts. Bare bindings ignore an incidental `SHIFT` on the event.
    pub fn matches(&self, key: &KeyEvent) -> bool {
        // Mask away modifiers we don't track (e.g. META) so cross-platform
        // events still match cleanly.
        let tracked = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT;
        let event_mods = key.modifiers & tracked;

        match (self.code, key.code) {
            (KeyCode::Char(a), KeyCode::Char(b)) => {
                if !a.eq_ignore_ascii_case(&b) {
                    return false;
                }

                if self.modifiers.contains(KeyModifiers::SHIFT) {
                    event_mods == self.modifiers
                } else {
                    let strip = KeyModifiers::SHIFT;
                    (self.modifiers & !strip) == (event_mods & !strip)
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
                let strip = KeyModifiers::SHIFT;
                (self.modifiers & !strip) == (event_mods & !strip)
            }
            (a, b) => a == b && event_mods == self.modifiers,
        }
    }

    /// Palette/help style: modifiers joined with `-` so punctuation key names
    /// (`+`, `-`, `=`) are not confused with the separator.
    pub fn display_dash_separated(self) -> String {
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
        let code = if self.modifiers.is_empty() {
            code_to_string(self.code)
        } else {
            code_to_human_string(self.code)
        };
        if parts.is_empty() {
            code
        } else {
            format!("{}-{}", parts.join("-"), code)
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
        // The named form roundtrips: chord steps are whitespace-separated in
        // the config format, so a literal ' ' would be unparseable.
        KeyCode::Char(' ') => "space".to_string(),
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

/// Punctuation keys rendered with modifier prefixes — avoids `ctrl--` etc.
fn code_to_human_string(code: KeyCode) -> String {
    match code {
        KeyCode::Char('+') => "plus".to_string(),
        KeyCode::Char('-') => "minus".to_string(),
        KeyCode::Char('=') => "equals".to_string(),
        other => code_to_string(other),
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

        let raw: Vec<&str> = input.split('+').collect();
        let (mod_parts, code_part): (Vec<&str>, &str) = if raw.last() == Some(&"") {
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
                    });
                }
            }
        }

        let code = parse_code(code_part).ok_or_else(|| KeyBindingParseError::UnknownCode {
            input: input.to_string(),
            code: code_part.to_string(),
        })?;

        let code = match code {
            KeyCode::Char(c) if c.is_ascii_alphabetic() => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
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
        "plus" => Some(KeyCode::Char('+')),
        "minus" => Some(KeyCode::Char('-')),
        "equals" | "eq" => Some(KeyCode::Char('=')),
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
            if let Some(rest) = lower.strip_prefix('f')
                && let Ok(n) = rest.parse::<u8>()
                && (1..=24).contains(&n)
            {
                return Some(KeyCode::F(n));
            }
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

// ── KeyChord ───────────────────────────────────────────────────────────────

/// A sequence of one or more [`KeyBinding`] steps.
///
/// A single-step chord (the default for most bindings) behaves exactly like
/// the underlying [`KeyBinding`]. Multi-step chords (e.g. `"ctrl+x s"`)
/// require the leader-key dispatcher to track pending state across events;
/// see [`KeyMap::dispatch`].
///
/// String form: chord steps separated by whitespace, each step in canonical
/// `<mod>+<mod>+<code>` form. Examples:
/// - `"ctrl+s"` — single-step, fires immediately.
/// - `"ctrl+x s"` — two-step leader chord: press Ctrl+X, then bare `s`.
/// - `"ctrl+x ctrl+s"` — two Ctrl-modified steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChord {
    steps: Vec<KeyBinding>,
}

impl KeyChord {
    /// Construct a chord from a non-empty step list. Returns `None` for
    /// an empty input — chords with zero steps are nonsensical.
    pub fn try_new(steps: Vec<KeyBinding>) -> Option<Self> {
        if steps.is_empty() {
            None
        } else {
            Some(Self { steps })
        }
    }

    /// Single-step chord wrapping a [`KeyBinding`].
    pub fn single(binding: KeyBinding) -> Self {
        Self {
            steps: vec![binding],
        }
    }

    /// Lift a sequence of live `KeyEvent`s into a chord. Ascii letters are
    /// lower-cased to mirror [`KeyBinding::from_str`] so the resulting chord
    /// round-trips cleanly through `Display`.
    pub fn from_events(events: &[KeyEvent]) -> Option<Self> {
        let steps: Vec<KeyBinding> = events
            .iter()
            .map(|ev| {
                let code = match ev.code {
                    KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                        KeyCode::Char(c.to_ascii_lowercase())
                    }
                    other => other,
                };
                KeyBinding::new(code, ev.modifiers)
            })
            .collect();
        Self::try_new(steps)
    }

    /// Number of chord steps (always ≥ 1; chords with zero steps are
    /// rejected at construction time, so [`Self::try_new`] returns
    /// `Option`).
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_single_step(&self) -> bool {
        self.steps.len() == 1
    }

    /// All chord steps, in order.
    pub fn steps(&self) -> &[KeyBinding] {
        &self.steps
    }

    /// First chord step. Used to surface a "what to press to enter this
    /// chord" hint without exposing the whole sequence.
    pub fn first_step(&self) -> &KeyBinding {
        &self.steps[0]
    }

    pub fn starts_with_leader(&self) -> bool {
        self.steps.first().is_some_and(|step| step.is_leader())
    }

    /// Multi-step chord whose first step is [`LEADER`].
    pub fn is_leader_chord(&self) -> bool {
        !self.is_single_step() && self.starts_with_leader()
    }

    /// Does the supplied event sequence (length N) match the chord's first
    /// N steps exactly? Useful for prefix-matching during chord dispatch.
    pub fn matches_prefix(&self, events: &[KeyEvent]) -> bool {
        if events.len() > self.steps.len() {
            return false;
        }
        events
            .iter()
            .zip(self.steps.iter())
            .all(|(ev, step)| step.matches(ev))
    }

    /// Does the supplied event sequence match the chord exactly (same
    /// length, every step matches)?
    pub fn matches_exact(&self, events: &[KeyEvent]) -> bool {
        events.len() == self.steps.len() && self.matches_prefix(events)
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, step) in self.steps.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{step}")?;
        }
        Ok(())
    }
}

impl std::str::FromStr for KeyChord {
    type Err = KeyBindingParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(KeyBindingParseError::Empty);
        }
        let steps: Result<Vec<KeyBinding>, _> = trimmed
            .split_whitespace()
            .map(|step| step.parse::<KeyBinding>())
            .collect();
        let steps = steps?;
        if steps.is_empty() {
            return Err(KeyBindingParseError::Empty);
        }
        Ok(Self { steps })
    }
}

impl Serialize for KeyChord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeyChord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Single-step chords that participate in direct key matching. The chord
/// [`LEADER`] is excluded — it only opens multi-step sequences.
fn is_bindable_single_step(chord: &KeyChord) -> bool {
    chord.is_single_step() && !chord.first_step().is_leader()
}

/// Helper so views can take either a `&Vec<KeyChord>` or `&[KeyChord]` and
/// test against a live `KeyEvent` with `.matches(&key)`.
///
/// Only **single-step** chords participate in this match. Multi-step chords
/// fire exclusively through [`KeyMap::dispatch`] at the app level — they
/// would be impossible to fire from a one-shot key match.
pub trait Bindings {
    fn matches(&self, key: &KeyEvent) -> bool;
}

impl Bindings for Vec<KeyChord> {
    fn matches(&self, key: &KeyEvent) -> bool {
        self.iter()
            .any(|c| is_bindable_single_step(c) && c.first_step().matches(key))
    }
}

impl Bindings for [KeyChord] {
    fn matches(&self, key: &KeyEvent) -> bool {
        self.iter()
            .any(|c| is_bindable_single_step(c) && c.first_step().matches(key))
    }
}

// ── KeyAction + KeyMap ─────────────────────────────────────────────────────

/// First-class identifier for every keymap-driven action. Used by the chord
/// dispatcher to deliver completed multi-step bindings to the active view
/// without going through a synthesized [`KeyEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Quit,
    Help,

    CommandPalette,
    NewSecret,
    SwitchStore,
    CopySelected,
    /// In the search view: copy `himitsu read <ref>` for the selected row.
    CopyRefSelected,
    Outputs,
    /// In the search view: collapse all secret paths to top-level folders.
    CollapsePaths,
    /// In the search view: expand all secret paths back to full depth.
    ExpandPaths,
    /// In the search view: toggle the secret-ref autocomplete popup.
    ToggleAutocomplete,
    /// In the search view: refine the query to the selected row's tag.
    RefineTag,
    /// In the search view: sort by the selected results column.
    SortColumn,

    Reveal,
    CopyValue,
    /// In the secret viewer: copy `himitsu read <ref>` for the open secret.
    CopyRef,
    Rekey,
    Edit,
    Delete,
    Back,

    SaveSecret,
    NextField,
    PrevField,
    Cancel,
}

/// Outcome of feeding a key event to [`KeyMap::dispatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// The pending sequence plus this key uniquely match a complete chord.
    /// The matching action fires; the dispatcher's pending buffer should
    /// be cleared.
    Match(KeyAction),
    /// At least one chord is a strict prefix of the pending+key sequence.
    /// The key should be added to the pending buffer; nothing fires yet.
    Pending,
    /// Neither a complete match nor a prefix. The pending buffer should be
    /// cleared and the key should be processed as a normal (non-chord)
    /// keystroke.
    Unmatched,
}

/// User-configurable keybindings grouped by action.
///
/// Each field is a list of [`KeyChord`]s so multiple key combinations can
/// map to the same action. Unspecified fields fall back to
/// [`KeyMap::default`], which supplies the built-in bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyMap {
    // ── Global ────────────────────────────────────────────────────────
    pub quit: Vec<KeyChord>,
    pub help: Vec<KeyChord>,

    // ── Search view ──────────────────────────────────────────────────
    pub command_palette: Vec<KeyChord>,
    pub new_secret: Vec<KeyChord>,
    pub switch_store: Vec<KeyChord>,
    /// Copy the selected search result's value to the clipboard
    /// (default: Ctrl+Y).
    pub copy_selected: Vec<KeyChord>,
    /// Copy `himitsu read <ref>` (the *command*, not the value) to the
    /// clipboard for the selected row. Useful when sharing how to fetch a
    /// secret without putting plaintext on the clipboard.
    pub copy_ref_selected: Vec<KeyChord>,
    /// Open the codegen browser. The serde aliases accept the pre-rename
    /// `outputs` and `envs` config keys, so existing user keymaps keep
    /// working.
    #[serde(rename = "codegen", alias = "outputs", alias = "envs")]
    pub outputs: Vec<KeyChord>,
    /// Collapse all secret paths to top-level folders.
    pub collapse_paths: Vec<KeyChord>,
    /// Expand all secret paths to full depth.
    pub expand_paths: Vec<KeyChord>,
    /// Toggle the secret-ref autocomplete popup.
    pub toggle_autocomplete: Vec<KeyChord>,
    /// Refine the query to the selected row's tag.
    pub refine_tag: Vec<KeyChord>,
    /// Sort by the selected results column (default: `Ctrl+O`).
    pub sort_column: Vec<KeyChord>,

    // ── Secret viewer ────────────────────────────────────────────────
    pub reveal: Vec<KeyChord>,
    /// Copy the revealed value to the clipboard (default: `y`).
    pub copy_value: Vec<KeyChord>,
    /// Copy `himitsu read <ref>` (the *command*) to the clipboard for the
    /// currently open secret.
    pub copy_ref: Vec<KeyChord>,
    pub rekey: Vec<KeyChord>,
    pub edit: Vec<KeyChord>,
    pub delete: Vec<KeyChord>,
    pub back: Vec<KeyChord>,

    // ── New-secret form ───────────────────────────────────────────────
    pub save_secret: Vec<KeyChord>,
    pub next_field: Vec<KeyChord>,
    pub prev_field: Vec<KeyChord>,
    pub cancel: Vec<KeyChord>,
}

impl Default for KeyMap {
    fn default() -> Self {
        let single = |b: KeyBinding| KeyChord::single(b);
        let bare = |c: KeyCode| single(KeyBinding::bare(c));
        let ctrl = |c: char| single(KeyBinding::ctrl(c));
        let shift_char = |c: char| single(KeyBinding::new(KeyCode::Char(c), KeyModifiers::SHIFT));
        let chord = |binding: KeyBinding| {
            KeyChord::try_new(vec![KeyBinding::ctrl('x'), binding])
                .expect("two-step leader chord is non-empty")
        };
        let chord_bare = |c: char| chord(KeyBinding::bare(KeyCode::Char(c)));
        let chord_ctrl = |c: char| chord(KeyBinding::ctrl(c));
        let chord_shift = |c: char| chord(KeyBinding::new(KeyCode::Char(c), KeyModifiers::SHIFT));

        Self {
            quit: vec![bare(KeyCode::Esc), ctrl('c')],
            help: vec![chord_bare('?')],

            command_palette: vec![ctrl('p')],
            new_secret: vec![ctrl('n')],
            switch_store: vec![chord_ctrl('s')],
            copy_selected: vec![ctrl('y')],
            copy_ref_selected: vec![chord_shift('y')],
            outputs: vec![chord_shift('e')],
            collapse_paths: vec![chord_bare('-')],
            expand_paths: vec![chord_bare('+'), chord_bare('=')],
            toggle_autocomplete: vec![chord_ctrl(' ')],
            refine_tag: vec![chord_ctrl('t')],
            sort_column: vec![ctrl('o')],

            reveal: vec![bare(KeyCode::Char('r'))],
            copy_value: vec![bare(KeyCode::Char('y'))],
            copy_ref: vec![chord_shift('y')],
            rekey: vec![shift_char('r')],
            edit: vec![bare(KeyCode::Char('e'))],
            delete: vec![bare(KeyCode::Char('d'))],
            back: vec![bare(KeyCode::Esc)],

            save_secret: vec![ctrl('s'), ctrl('w')],
            next_field: vec![bare(KeyCode::Tab)],
            prev_field: vec![bare(KeyCode::BackTab)],
            cancel: vec![bare(KeyCode::Esc)],
        }
    }
}

// ── KeyRegistry ────────────────────────────────────────────────────────────

/// Which view's help screen lists a [`KeyAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Available everywhere (quit, help).
    Global,
    /// Search view.
    Search,
    /// Secret viewer.
    Viewer,
    /// New-secret form.
    NewSecretForm,
}

/// One registry row: everything every surface needs to know about a
/// [`KeyAction`] — its config field, its help text, which view's help
/// screen lists it, and the command-palette entry that shares it.
pub struct Row {
    /// Accessor for the user-configurable chord list backing this action.
    pub field: fn(&KeyMap) -> &Vec<KeyChord>,
    /// Help-screen description.
    pub help: &'static str,
    /// Which view's help screen lists this action.
    pub scope: Scope,
    /// The command-palette command sharing this action, if any.
    pub palette: Option<crate::tui::views::command_palette::Command>,
}

impl KeyAction {
    /// Every action, in help-screen display order (grouped by scope). Used
    /// to derive [`KeyMap::entries`] — an action missing here simply never
    /// dispatches, which is loud, unlike the silent drift of the old
    /// hand-maintained table.
    pub const ALL: [KeyAction; 24] = [
        // Global
        KeyAction::Help,
        KeyAction::Quit,
        // Search view
        KeyAction::CommandPalette,
        KeyAction::NewSecret,
        KeyAction::SwitchStore,
        KeyAction::CopySelected,
        KeyAction::CopyRefSelected,
        KeyAction::Outputs,
        KeyAction::CollapsePaths,
        KeyAction::ExpandPaths,
        KeyAction::ToggleAutocomplete,
        KeyAction::RefineTag,
        KeyAction::SortColumn,
        // Secret viewer
        KeyAction::Reveal,
        KeyAction::CopyValue,
        KeyAction::CopyRef,
        KeyAction::Rekey,
        KeyAction::Edit,
        KeyAction::Delete,
        KeyAction::Back,
        // New-secret form
        KeyAction::SaveSecret,
        KeyAction::NextField,
        KeyAction::PrevField,
        KeyAction::Cancel,
    ];
}

/// The single source of truth tying a [`KeyAction`] to its config field,
/// help text, scope, and palette link. The exhaustive match means adding a
/// `KeyAction` variant without a row is a **compile error** — this replaces
/// the hand-synced `entries()` table, the per-view hardcoded help strings,
/// and the palette's hand-maintained action map.
pub fn row(action: KeyAction) -> Row {
    use crate::tui::views::command_palette::Command;
    match action {
        KeyAction::Quit => Row {
            field: |km| &km.quit,
            help: "quit",
            scope: Scope::Global,
            palette: Some(Command::Quit),
        },
        KeyAction::Help => Row {
            field: |km| &km.help,
            help: "toggle this help",
            scope: Scope::Global,
            palette: Some(Command::Help),
        },
        KeyAction::CommandPalette => Row {
            field: |km| &km.command_palette,
            help: "open command palette",
            scope: Scope::Search,
            palette: None,
        },
        KeyAction::NewSecret => Row {
            field: |km| &km.new_secret,
            help: "new secret",
            scope: Scope::Search,
            palette: Some(Command::NewSecret),
        },
        KeyAction::SwitchStore => Row {
            field: |km| &km.switch_store,
            help: "switch store",
            scope: Scope::Search,
            palette: Some(Command::SwitchStore),
        },
        KeyAction::CopySelected => Row {
            field: |km| &km.copy_selected,
            help: "copy selection to clipboard",
            scope: Scope::Search,
            palette: None,
        },
        KeyAction::CopyRefSelected => Row {
            field: |km| &km.copy_ref_selected,
            help: "copy `himitsu read <ref>` command",
            scope: Scope::Search,
            palette: None,
        },
        KeyAction::Outputs => Row {
            field: |km| &km.outputs,
            help: "browse codegen",
            scope: Scope::Search,
            palette: Some(Command::Outputs),
        },
        KeyAction::CollapsePaths => Row {
            field: |km| &km.collapse_paths,
            help: "collapse paths to top-level folders",
            scope: Scope::Search,
            palette: None,
        },
        KeyAction::ExpandPaths => Row {
            field: |km| &km.expand_paths,
            help: "expand paths to full depth",
            scope: Scope::Search,
            palette: None,
        },
        KeyAction::ToggleAutocomplete => Row {
            field: |km| &km.toggle_autocomplete,
            help: "toggle ref autocomplete",
            scope: Scope::Search,
            palette: None,
        },
        KeyAction::RefineTag => Row {
            field: |km| &km.refine_tag,
            help: "refine to selected tag",
            scope: Scope::Search,
            palette: None,
        },
        KeyAction::SortColumn => Row {
            field: |km| &km.sort_column,
            help: "sort selected column",
            scope: Scope::Search,
            palette: None,
        },
        KeyAction::Reveal => Row {
            field: |km| &km.reveal,
            help: "reveal / hide value",
            scope: Scope::Viewer,
            palette: None,
        },
        KeyAction::CopyValue => Row {
            field: |km| &km.copy_value,
            help: "copy value to clipboard",
            scope: Scope::Viewer,
            palette: None,
        },
        KeyAction::CopyRef => Row {
            field: |km| &km.copy_ref,
            help: "copy `himitsu read <ref>` command",
            scope: Scope::Viewer,
            palette: None,
        },
        KeyAction::Rekey => Row {
            field: |km| &km.rekey,
            help: "rekey for current recipients",
            scope: Scope::Viewer,
            palette: None,
        },
        KeyAction::Edit => Row {
            field: |km| &km.edit,
            help: "edit value + metadata in $EDITOR",
            scope: Scope::Viewer,
            palette: None,
        },
        KeyAction::Delete => Row {
            field: |km| &km.delete,
            help: "delete secret (with confirm)",
            scope: Scope::Viewer,
            palette: None,
        },
        KeyAction::Back => Row {
            field: |km| &km.back,
            help: "back",
            scope: Scope::Viewer,
            palette: None,
        },
        KeyAction::SaveSecret => Row {
            field: |km| &km.save_secret,
            help: "save from any field",
            scope: Scope::NewSecretForm,
            palette: None,
        },
        KeyAction::NextField => Row {
            field: |km| &km.next_field,
            help: "next field (wraps)",
            scope: Scope::NewSecretForm,
            palette: None,
        },
        KeyAction::PrevField => Row {
            field: |km| &km.prev_field,
            help: "previous field (wraps)",
            scope: Scope::NewSecretForm,
            palette: None,
        },
        KeyAction::Cancel => Row {
            field: |km| &km.cancel,
            help: "cancel",
            scope: Scope::NewSecretForm,
            palette: None,
        },
    }
}

/// Live help rows for one scope: `(chords, description)` with the chord
/// display rendered from the CURRENT keymap, so user rebinds show up in
/// every help screen automatically.
pub fn help_rows(keymap: &KeyMap, scope: Scope) -> Vec<(String, String)> {
    KeyAction::ALL
        .into_iter()
        .filter(|a| row(*a).scope == scope)
        .map(|a| (chords_display(keymap, a), row(a).help.to_string()))
        .collect()
}

/// One live help row for a single action — for views that compose their
/// help screens from individual rows rather than a whole scope.
pub fn help_row(keymap: &KeyMap, action: KeyAction) -> (String, String) {
    (chords_display(keymap, action), row(action).help.to_string())
}

/// Human-facing display of every chord bound to `action`, joined with
/// ` / `. Modifier `+` separators within each step render as `-` to match
/// the command palette's established style (e.g. `ctrl+x -`).
pub fn chords_display(keymap: &KeyMap, action: KeyAction) -> String {
    keymap
        .chords_for(action)
        .iter()
        .map(|c| {
            c.steps()
                .iter()
                .map(|step| step.display_dash_separated())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

impl KeyMap {
    /// `(action, chords)` pairs across every keymap field, derived from
    /// [`KeyAction::ALL`] and the registry [`row`]s — there is no second
    /// table to keep in sync.
    fn entries(&self) -> [(KeyAction, &Vec<KeyChord>); KeyAction::ALL.len()] {
        KeyAction::ALL.map(|action| (action, (row(action).field)(self)))
    }

    /// Single-step direct lookup: find the action whose binding list
    /// contains a single-step chord matching `key`. Returns the FIRST
    /// matching action in [`Self::entries`] order; ties favour the action
    /// declared earliest. Multi-step chords are NEVER returned here —
    /// they fire only via [`Self::dispatch`].
    pub fn action_for_key(&self, key: &KeyEvent) -> Option<KeyAction> {
        for (action, chords) in self.entries() {
            if chords
                .iter()
                .any(|c| is_bindable_single_step(c) && c.first_step().matches(key))
            {
                return Some(action);
            }
        }
        None
    }

    /// View-scoped variant of [`Self::action_for_key`]: scan only the
    /// listed actions, in the order given. Each view declares its own
    /// priority slice (e.g. the secret viewer wants `Rekey` before
    /// `Reveal` so `Shift+R` doesn't fall through to bare `r`); shared
    /// here so the per-view helpers don't each rebuild the same iteration.
    pub fn action_for_key_in(&self, key: &KeyEvent, priority: &[KeyAction]) -> Option<KeyAction> {
        priority
            .iter()
            .copied()
            .find(|&action| self.chords_for(action).matches(key))
    }

    /// Borrow the chord list registered for a given action — a direct
    /// registry-field access; total by construction (the registry match is
    /// exhaustive over [`KeyAction`]).
    pub fn chords_for(&self, action: KeyAction) -> &Vec<KeyChord> {
        (row(action).field)(self)
    }

    /// Drive the leader-key (`ctrl+x`) chord state machine.
    ///
    /// Only multi-step chords beginning with [`LEADER`] participate. The
    /// leader itself never fires an action — it only opens a pending
    /// sequence that must complete (or time out) within
    /// [`CHORD_TIMEOUT_MS`].
    ///
    /// `pending` is the buffer of events accumulated from previously-pending
    /// chord steps; `key` is the just-arrived event. Returns:
    ///
    /// - [`Dispatch::Match`] when `pending + [key]` exactly matches some
    ///   leader chord. Caller should fire the action and clear `pending`.
    /// - [`Dispatch::Pending`] when at least one leader chord has
    ///   `pending + [key]` as a strict prefix. Caller should append `key`
    ///   to `pending` and swallow it.
    /// - [`Dispatch::Unmatched`] otherwise. Caller should clear `pending`
    ///   and treat `key` as a normal (non-chord) keystroke.
    ///
    /// Single-step chords are deliberately invisible to this dispatcher:
    /// they're already handled by each view's existing per-action priority
    /// match (which knows which actions that view cares about).
    pub fn dispatch(&self, pending: &[KeyEvent], key: &KeyEvent) -> Dispatch {
        if pending.is_empty() && !LEADER.matches(key) {
            return Dispatch::Unmatched;
        }

        let mut buf: Vec<KeyEvent> = Vec::with_capacity(pending.len() + 1);
        buf.extend_from_slice(pending);
        buf.push(*key);

        let mut exact: Option<KeyAction> = None;
        let mut has_longer_prefix = false;

        for (action, chords) in self.entries() {
            for chord in chords {
                if !chord.is_leader_chord() {
                    continue;
                }
                if chord.matches_exact(&buf) {
                    if exact.is_none() {
                        exact = Some(action);
                    }
                } else if chord.len() > buf.len() && chord.matches_prefix(&buf) {
                    has_longer_prefix = true;
                }
            }
        }

        if let Some(action) = exact {
            Dispatch::Match(action)
        } else if has_longer_prefix {
            Dispatch::Pending
        } else {
            Dispatch::Unmatched
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

    fn chord_strings(chords: &[KeyChord]) -> Vec<String> {
        chords.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn registry_covers_every_key_action_variant() {
        // Row coverage is a COMPILE error now (the registry match is
        // exhaustive); what's left to pin at test time is that ALL lists
        // every variant exactly once (an action missing from ALL never
        // dispatches), that every action has a non-empty default binding,
        // and that every row carries help text.
        let km = KeyMap::default();
        for action in KeyAction::ALL {
            assert!(
                !km.chords_for(action).is_empty(),
                "{action:?} has no default binding"
            );
            assert!(!row(action).help.is_empty(), "{action:?} has empty help");
        }
        // Duplicate detection without Hash: pairwise.
        for (i, a) in KeyAction::ALL.iter().enumerate() {
            for b in &KeyAction::ALL[i + 1..] {
                assert_ne!(a, b, "duplicate entry in KeyAction::ALL");
            }
        }
        assert_eq!(km.entries().len(), KeyAction::ALL.len());
    }

    #[test]
    fn help_rows_render_live_bindings() {
        // Rebinding an action must change the help row — help screens can't
        // lie. Default first:
        let km = KeyMap::default();
        let rows = help_rows(&km, Scope::Search);
        assert!(
            rows.iter()
                .any(|(k, d)| k == "ctrl-x ctrl-t" && d == "refine to selected tag"),
            "{rows:?}"
        );

        // Rebind and confirm the row follows.
        let remapped = KeyMap {
            refine_tag: vec![KeyChord::single(KeyBinding::ctrl('g'))],
            ..KeyMap::default()
        };
        let rows = help_rows(&remapped, Scope::Search);
        assert!(
            rows.iter()
                .any(|(k, d)| k == "ctrl-g" && d == "refine to selected tag"),
            "{rows:?}"
        );
    }

    #[test]
    fn default_non_footer_actions_are_chord_only() {
        let km = KeyMap::default();

        assert_eq!(chord_strings(&km.help), ["ctrl+x ?"]);
        assert_eq!(chord_strings(&km.switch_store), ["ctrl+x ctrl+s"]);
        assert_eq!(
            chord_strings(&km.toggle_autocomplete),
            ["ctrl+x ctrl+space"]
        );
        assert_eq!(chord_strings(&km.refine_tag), ["ctrl+x ctrl+t"]);
        assert_eq!(chord_strings(&km.copy_ref_selected), ["ctrl+x shift+y"]);
        assert_eq!(chord_strings(&km.outputs), ["ctrl+x shift+e"]);
        assert_eq!(chord_strings(&km.collapse_paths), ["ctrl+x -"]);
        assert_eq!(
            chord_strings(&km.expand_paths),
            ["ctrl+x +", "ctrl+x ="]
        );
        assert_eq!(chord_strings(&km.copy_ref), ["ctrl+x shift+y"]);

        assert!(
            !km.help
                .matches(&key(KeyCode::Char('?'), KeyModifiers::NONE))
        );
        assert!(
            !km.switch_store
                .matches(&key(KeyCode::Char('s'), KeyModifiers::CONTROL))
        );
        assert!(
            !km.toggle_autocomplete
                .matches(&key(KeyCode::Char(' '), KeyModifiers::CONTROL))
        );
        assert!(
            !km.refine_tag
                .matches(&key(KeyCode::Char('t'), KeyModifiers::CONTROL))
        );
        assert!(
            !km.copy_ref_selected
                .matches(&key(KeyCode::Char('Y'), KeyModifiers::SHIFT))
        );
        assert!(
            !km.outputs
                .matches(&key(KeyCode::Char('E'), KeyModifiers::SHIFT))
        );
        assert!(
            !km.collapse_paths
                .matches(&key(KeyCode::Char('-'), KeyModifiers::NONE))
        );
        assert!(
            !km.expand_paths
                .matches(&key(KeyCode::Char('+'), KeyModifiers::NONE))
        );
        assert!(
            !km.expand_paths
                .matches(&key(KeyCode::Char('='), KeyModifiers::NONE))
        );
        assert!(
            !km.copy_ref
                .matches(&key(KeyCode::Char('Y'), KeyModifiers::SHIFT))
        );
    }

    #[test]
    fn legacy_envs_config_key_still_binds_outputs() {
        // The field was renamed envs -> outputs; the serde alias must keep
        // pre-rename user keymaps working.
        let yaml = r#"
envs: ["ctrl+l"]
"#;
        let km: KeyMap = serde_yaml::from_str(yaml).unwrap();
        assert!(
            km.outputs
                .matches(&key(KeyCode::Char('l'), KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn palette_links_resolve_through_registry() {
        use crate::tui::views::command_palette::Command;
        // The palette's key_action() derives from registry rows; spot-check
        // the link survives in both directions.
        assert_eq!(Command::Outputs.key_action(), Some(KeyAction::Outputs));
        assert_eq!(row(KeyAction::Outputs).palette, Some(Command::Outputs));
        assert_eq!(Command::Import.key_action(), None);
    }

    #[test]
    fn fold_actions_have_chord_only_defaults() {
        let km = KeyMap::default();

        let collapse_single = key(KeyCode::Char('-'), KeyModifiers::NONE);
        assert_eq!(km.action_for_key(&collapse_single), None);

        let expand_plus = key(KeyCode::Char('+'), KeyModifiers::NONE);
        let expand_eq = key(KeyCode::Char('='), KeyModifiers::NONE);
        assert_eq!(km.action_for_key(&expand_plus), None);
        assert_eq!(km.action_for_key(&expand_eq), None);
    }

    #[test]
    fn leader_chord_resolves_fold_actions() {
        let km = KeyMap::default();
        let ctrl_x = key(KeyCode::Char('x'), KeyModifiers::CONTROL);

        // Ctrl+x is a prefix of the fold leader chords — pending, not a match.
        assert_eq!(km.dispatch(&[], &ctrl_x), Dispatch::Pending);

        let plus = key(KeyCode::Char('+'), KeyModifiers::NONE);
        assert_eq!(
            km.dispatch(&[ctrl_x], &plus),
            Dispatch::Match(KeyAction::ExpandPaths)
        );

        let equals = key(KeyCode::Char('='), KeyModifiers::NONE);
        assert_eq!(
            km.dispatch(&[ctrl_x], &equals),
            Dispatch::Match(KeyAction::ExpandPaths)
        );

        let minus = key(KeyCode::Char('-'), KeyModifiers::NONE);
        assert_eq!(
            km.dispatch(&[ctrl_x], &minus),
            Dispatch::Match(KeyAction::CollapsePaths)
        );
    }

    #[test]
    fn chords_display_formats_each_step_separately() {
        let km = KeyMap::default();
        assert_eq!(
            chords_display(&km, KeyAction::CollapsePaths),
            "ctrl-x -"
        );
        assert_eq!(
            chords_display(&km, KeyAction::ExpandPaths),
            "ctrl-x + / ctrl-x ="
        );
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
    fn parses_uppercase_letter_equivalent_to_lowercase() {
        let lower: KeyBinding = "y".parse().unwrap();
        let upper: KeyBinding = "Y".parse().unwrap();
        assert_eq!(lower, upper);
        assert_eq!(lower, KeyBinding::bare(KeyCode::Char('y')));
    }

    #[test]
    fn parses_shift_and_ctrl_letter_case_insensitive() {
        let shift_lower: KeyBinding = "shift+y".parse().unwrap();
        let shift_upper: KeyBinding = "shift+Y".parse().unwrap();
        assert_eq!(
            shift_lower,
            KeyBinding::new(KeyCode::Char('y'), KeyModifiers::SHIFT)
        );
        assert_eq!(shift_lower, shift_upper);

        let ctrl_lower: KeyBinding = "ctrl+y".parse().unwrap();
        let ctrl_upper: KeyBinding = "ctrl+Y".parse().unwrap();
        assert_eq!(ctrl_lower, KeyBinding::ctrl('y'));
        assert_eq!(ctrl_lower, ctrl_upper);
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
        assert!(
            km.new_secret
                .matches(&key(KeyCode::Char('n'), KeyModifiers::CONTROL))
        );
        assert!(km.quit.matches(&key(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(
            km.reveal
                .matches(&key(KeyCode::Char('r'), KeyModifiers::NONE))
        );
        assert!(
            km.rekey
                .matches(&key(KeyCode::Char('R'), KeyModifiers::SHIFT))
        );
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
        assert!(
            km.new_secret
                .matches(&key(KeyCode::F(2), KeyModifiers::NONE))
        );
        assert!(
            !km.new_secret
                .matches(&key(KeyCode::Char('n'), KeyModifiers::CONTROL))
        );
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

    // ── Chord parsing ──────────────────────────────────────────────────

    #[test]
    fn parses_single_step_chord() {
        let c: KeyChord = "ctrl+s".parse().unwrap();
        assert!(c.is_single_step());
        assert_eq!(c.first_step(), &KeyBinding::ctrl('s'));
    }

    #[test]
    fn parses_two_step_leader_chord() {
        let c: KeyChord = "ctrl+x s".parse().unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.steps()[0], KeyBinding::ctrl('x'));
        assert_eq!(c.steps()[1], KeyBinding::bare(KeyCode::Char('s')));
    }

    #[test]
    fn parses_chord_with_modifier_steps() {
        let c: KeyChord = "ctrl+x ctrl+s".parse().unwrap();
        assert_eq!(c.steps()[0], KeyBinding::ctrl('x'));
        assert_eq!(c.steps()[1], KeyBinding::ctrl('s'));
    }

    #[test]
    fn chord_display_round_trip() {
        let original = "ctrl+x s";
        let parsed: KeyChord = original.parse().unwrap();
        assert_eq!(parsed.to_string(), original);
    }

    #[test]
    fn empty_chord_string_rejected() {
        assert!("".parse::<KeyChord>().is_err());
        assert!("   ".parse::<KeyChord>().is_err());
    }

    #[test]
    fn chord_yaml_round_trip() {
        let yaml = r#"
save_secret: ["ctrl+x s", "ctrl+w"]
"#;
        let km: KeyMap = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(km.save_secret.len(), 2);
        assert_eq!(km.save_secret[0].len(), 2);
        assert!(km.save_secret[1].is_single_step());

        // Round-trip preserves the chord shape.
        let back = serde_yaml::to_string(&km).unwrap();
        assert!(back.contains("ctrl+x s"));
        assert!(back.contains("ctrl+w"));
    }

    // ── Bindings::matches semantics ────────────────────────────────────

    #[test]
    fn bindings_match_ignores_multi_step_chords() {
        // A user binding `save_secret: ["ctrl+x s"]` must NOT fire on a bare
        // `s` keypress — multi-step chords go through dispatch, never the
        // single-key matcher.
        let km: KeyMap = serde_yaml::from_str(r#"save_secret: ["ctrl+x s"]"#).unwrap();
        assert!(
            !km.save_secret
                .matches(&key(KeyCode::Char('s'), KeyModifiers::NONE))
        );
        assert!(
            !km.save_secret
                .matches(&key(KeyCode::Char('x'), KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn bindings_match_allows_single_step_chords() {
        let km: KeyMap = serde_yaml::from_str(r#"save_secret: ["ctrl+s"]"#).unwrap();
        assert!(
            km.save_secret
                .matches(&key(KeyCode::Char('s'), KeyModifiers::CONTROL))
        );
    }

    // ── KeyMap::dispatch state machine ─────────────────────────────────

    #[test]
    fn dispatch_ignores_single_step_chords() {
        // Single-step bindings flow through each view's per-action priority
        // match, not the chord dispatcher — otherwise typing 'e' into a
        // form would steal the viewer's `edit` binding.
        let km = KeyMap::default();
        let result = km.dispatch(&[], &key(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(result, Dispatch::Unmatched);
    }

    #[test]
    fn dispatch_two_step_chord_pending_then_match() {
        let km: KeyMap = serde_yaml::from_str(r#"save_secret: ["ctrl+x s"]"#).unwrap();

        // Step 1: ctrl+x with no pending → Pending (because ctrl+x is a
        // strict prefix of ctrl+x s).
        let r1 = km.dispatch(&[], &key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(r1, Dispatch::Pending);

        // Step 2: feed 's' with pending = [ctrl+x] → Match.
        let pending = vec![key(KeyCode::Char('x'), KeyModifiers::CONTROL)];
        let r2 = km.dispatch(&pending, &key(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(r2, Dispatch::Match(KeyAction::SaveSecret));
    }

    #[test]
    fn dispatch_chord_aborts_when_no_continuation_matches() {
        let km: KeyMap = serde_yaml::from_str(r#"save_secret: ["ctrl+x s"]"#).unwrap();
        let pending = vec![key(KeyCode::Char('x'), KeyModifiers::CONTROL)];

        // 'q' isn't a continuation of ctrl+x → Unmatched.
        let r = km.dispatch(&pending, &key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert_eq!(r, Dispatch::Unmatched);
    }

    #[test]
    fn dispatch_unmatched_when_no_chord_starts_with_key() {
        let km = KeyMap::default();
        // 'z' isn't bound anywhere by default.
        let r = km.dispatch(&[], &key(KeyCode::Char('z'), KeyModifiers::NONE));
        assert_eq!(r, Dispatch::Unmatched);
    }

    #[test]
    fn leader_cannot_bind_as_single_step_action() {
        let yaml = r#"
quit: ["ctrl+x"]
save_secret: ["ctrl+x s"]
"#;
        let km: KeyMap = serde_yaml::from_str(yaml).unwrap();
        let ctrl_x = key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(km.action_for_key(&ctrl_x), None);
        assert_eq!(km.dispatch(&[], &ctrl_x), Dispatch::Pending);
    }

    #[test]
    fn non_leader_multi_step_chords_are_ignored_by_dispatch() {
        let yaml = r#"
edit: ["e d"]
"#;
        let km: KeyMap = serde_yaml::from_str(yaml).unwrap();
        let e = key(KeyCode::Char('e'), KeyModifiers::NONE);
        assert_eq!(km.dispatch(&[], &e), Dispatch::Unmatched);
    }

    #[test]
    fn action_for_key_skips_multi_step_chords() {
        let km: KeyMap = serde_yaml::from_str(r#"save_secret: ["ctrl+x s"]"#).unwrap();
        // bare 's' must not be claimed by save_secret since the only binding
        // is multi-step.
        assert_eq!(
            km.action_for_key(&key(KeyCode::Char('s'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn action_for_key_finds_single_step_chord() {
        let km = KeyMap::default();
        assert_eq!(
            km.action_for_key(&key(KeyCode::Char('n'), KeyModifiers::CONTROL)),
            Some(KeyAction::NewSecret)
        );
    }
}
