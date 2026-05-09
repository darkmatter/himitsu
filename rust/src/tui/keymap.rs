//! User-configurable TUI keybindings.
//!
//! The [`KeyMap`] struct holds one list of bindings per **action**, not per
//! key. Views consult the map through small `matches` helpers on
//! [`KeyBinding`] / [`KeyChord`] so a single action can be triggered by any
//! number of equivalent key combinations.
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

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
                    let _ = (self_lower, ev_lower);
                    ev_mods == self_mods
                } else {
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
                    })
                }
            }
        }

        let code = parse_code(code_part).ok_or_else(|| KeyBindingParseError::UnknownCode {
            input: input.to_string(),
            code: code_part.to_string(),
        })?;

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
            if let Some(rest) = lower.strip_prefix('f') {
                if let Ok(n) = rest.parse::<u8>() {
                    if (1..=24).contains(&n) {
                        return Some(KeyCode::F(n));
                    }
                }
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

    /// Lift a sequence of live `KeyEvent`s into a chord. The shift-from-
    /// uppercase-char fixup mirrors `KeyBinding::FromStr` so the resulting
    /// chord round-trips cleanly through `Display` — i.e. `format("{c}")`
    /// on a chord built from the events of `Ctrl+X` then `Y` produces
    /// `"ctrl+x shift+y"`, matching the user-facing config syntax.
    pub fn from_events(events: &[KeyEvent]) -> Option<Self> {
        let steps: Vec<KeyBinding> = events
            .iter()
            .map(|ev| {
                let mut mods = ev.modifiers;
                if let KeyCode::Char(c) = ev.code {
                    if c.is_ascii_uppercase() {
                        mods |= KeyModifiers::SHIFT;
                    }
                }
                KeyBinding::new(ev.code, mods)
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
            .any(|c| c.is_single_step() && c.first_step().matches(key))
    }
}

impl Bindings for [KeyChord] {
    fn matches(&self, key: &KeyEvent) -> bool {
        self.iter()
            .any(|c| c.is_single_step() && c.first_step().matches(key))
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
    Envs,

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
/// [`KeyMap::default`], which reproduces the original hardcoded bindings.
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
    /// secret without putting plaintext on the clipboard. Default: `Y`
    /// (Shift+y) — symmetric with the viewer.
    pub copy_ref_selected: Vec<KeyChord>,
    pub envs: Vec<KeyChord>,

    // ── Secret viewer ────────────────────────────────────────────────
    pub reveal: Vec<KeyChord>,
    /// Copy the revealed value to the clipboard (default: `y`).
    pub copy_value: Vec<KeyChord>,
    /// Copy `himitsu read <ref>` (the *command*) to the clipboard for the
    /// currently open secret. Default: `Y` (Shift+y).
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

        Self {
            quit: vec![bare(KeyCode::Esc), ctrl('c')],
            help: vec![bare(KeyCode::Char('?'))],

            command_palette: vec![ctrl('p')],
            new_secret: vec![ctrl('n')],
            switch_store: vec![ctrl('s')],
            copy_selected: vec![ctrl('y')],
            copy_ref_selected: vec![shift_char('y')],
            envs: vec![shift_char('e')],

            reveal: vec![bare(KeyCode::Char('r'))],
            copy_value: vec![bare(KeyCode::Char('y'))],
            copy_ref: vec![shift_char('y')],
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

impl KeyMap {
    /// `(action, chords)` pairs across every keymap field. Stack-allocated
    /// so the dispatcher (called per keystroke) doesn't churn the heap.
    /// When you add a new `KeyAction`, append it here and bump the array
    /// length.
    fn entries(&self) -> [(KeyAction, &Vec<KeyChord>); 19] {
        [
            (KeyAction::Quit, &self.quit),
            (KeyAction::Help, &self.help),
            (KeyAction::CommandPalette, &self.command_palette),
            (KeyAction::NewSecret, &self.new_secret),
            (KeyAction::SwitchStore, &self.switch_store),
            (KeyAction::CopySelected, &self.copy_selected),
            (KeyAction::CopyRefSelected, &self.copy_ref_selected),
            (KeyAction::Envs, &self.envs),
            (KeyAction::Reveal, &self.reveal),
            (KeyAction::CopyValue, &self.copy_value),
            (KeyAction::CopyRef, &self.copy_ref),
            (KeyAction::Rekey, &self.rekey),
            (KeyAction::Edit, &self.edit),
            (KeyAction::Delete, &self.delete),
            (KeyAction::Back, &self.back),
            (KeyAction::SaveSecret, &self.save_secret),
            (KeyAction::NextField, &self.next_field),
            (KeyAction::PrevField, &self.prev_field),
            (KeyAction::Cancel, &self.cancel),
        ]
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
                .any(|c| c.is_single_step() && c.first_step().matches(key))
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

    /// Borrow the chord list registered for a given action. Looked up
    /// linearly through [`Self::entries`] — ~19 entries, called per
    /// keystroke; the cost is dwarfed by terminal redraw.
    pub fn chords_for(&self, action: KeyAction) -> &Vec<KeyChord> {
        for (a, chords) in self.entries() {
            if a == action {
                return chords;
            }
        }
        // entries() covers every variant of KeyAction, so this is
        // unreachable in practice.
        unreachable!("KeyMap::entries missing variant {action:?}")
    }

    /// Drive the leader-key state machine.
    ///
    /// `pending` is the buffer of events accumulated from previously-pending
    /// chord steps; `key` is the just-arrived event. Returns:
    ///
    /// - [`Dispatch::Match`] when `pending + [key]` exactly matches some
    ///   **multi-step** chord. Caller should fire the action and clear
    ///   `pending`.
    /// - [`Dispatch::Pending`] when at least one chord has `pending + [key]`
    ///   as a strict prefix (i.e. more keys could complete a chord). Caller
    ///   should append `key` to `pending` and swallow it.
    /// - [`Dispatch::Unmatched`] otherwise. Caller should clear `pending`
    ///   and treat `key` as a normal non-chord input.
    ///
    /// Single-step chords are deliberately invisible to this dispatcher:
    /// they're already handled by each view's existing per-action priority
    /// match (which knows which actions that view cares about). Letting
    /// `dispatch` claim single-step bindings would steal plain typing
    /// keys (e.g. `e` matching the viewer's `edit` while the user is
    /// typing into the new-secret form's `path` field).
    ///
    /// Caveat — multi-step chords always shadow single-step bindings on
    /// the same first key: if a user binds both `edit: ["e"]` and
    /// `delete: ["e d"]`, pressing `e` enters Pending state because the
    /// `e d` chord has `e` as a prefix. The single-step `e` binding can
    /// then never fire without a continuation that aborts the chord.
    /// This is by design — leader chords need to swallow their first
    /// step or they wouldn't work.
    ///
    /// Resolution rule when several chords match:
    /// - If both an exact multi-step match and a longer prefix-match exist
    ///   for the same buffer, the exact match wins (greedy short-match).
    ///   Practical implication: don't bind both `ctrl+x s` and
    ///   `ctrl+x s ctrl+w` — the shorter chord will always fire first.
    pub fn dispatch(&self, pending: &[KeyEvent], key: &KeyEvent) -> Dispatch {
        let mut buf: Vec<KeyEvent> = Vec::with_capacity(pending.len() + 1);
        buf.extend_from_slice(pending);
        buf.push(*key);

        let mut exact: Option<KeyAction> = None;
        let mut has_longer_prefix = false;

        for (action, chords) in self.entries() {
            for chord in chords {
                if !chord.is_single_step() && chord.matches_exact(&buf) {
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
        assert!(!km
            .save_secret
            .matches(&key(KeyCode::Char('s'), KeyModifiers::NONE)));
        assert!(!km
            .save_secret
            .matches(&key(KeyCode::Char('x'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn bindings_match_allows_single_step_chords() {
        let km: KeyMap = serde_yaml::from_str(r#"save_secret: ["ctrl+s"]"#).unwrap();
        assert!(km
            .save_secret
            .matches(&key(KeyCode::Char('s'), KeyModifiers::CONTROL)));
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
    fn dispatch_lets_chord_take_precedence_over_single_step_prefix() {
        // User binds both `ctrl+x` (single-step) and `ctrl+x s` (chord)
        // to different actions. Pressing Ctrl+X enters Pending state
        // because the chord dispatcher only fires on multi-step matches —
        // the single-step `quit` is left to the per-view path, which the
        // App would only consult if the chord aborts.
        let yaml = r#"
quit: ["ctrl+x"]
save_secret: ["ctrl+x s"]
"#;
        let km: KeyMap = serde_yaml::from_str(yaml).unwrap();
        let r = km.dispatch(&[], &key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(r, Dispatch::Pending);
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
