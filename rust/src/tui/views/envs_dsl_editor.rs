//! YAML/DSL editor pane for envs.
//!
//! Wraps a [`super::envs_text::TextBuffer`] with:
//! - YAML parse-on-demand (`current_envs`) producing a `(label,
//!   Vec<EnvEntry>)` map
//! - Autocomplete suggestions over a label corpus (item names, group
//!   prefixes, derived env-key names) using `nucleo-matcher`
//! - Live preview production via [`crate::config::env_dsl::resolve_all`]
//!
//! The buffer holds the **full envs YAML body** — i.e. the value of the
//! top-level `envs:` key, e.g.:
//!
//! ```yaml
//! my-env:
//!   - SOME_KEY: some-item
//!   - other-item
//! my-env-{dev,prod}:
//!   - {}/db
//! ```
//!
//! Saving parses the body, validates each label, and writes back via
//! [`crate::config::envs_mut::upsert`] for each top-level label found
//! (plus a delete pass for labels that disappeared).

use std::collections::BTreeMap;

use crossterm::event::{KeyEvent, KeyModifiers};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher};

use super::envs_text::TextBuffer;
use crate::config::env_dsl::{self, ResolutionOutput};
use crate::config::env_resolver::EnvNode;
use crate::config::EnvEntry;

/// Top-level outcome of a key event handled by the DSL editor.
#[derive(Debug, Clone)]
pub enum DslEditorOutcome {
    /// Stay in the editor — possibly with a buffer/selection mutation.
    Pending,
    /// User pressed Esc/Cancel — caller should drop the editor.
    Cancelled,
    /// User pressed Ctrl-S to save. The caller serializes the buffer and
    /// performs the write.
    SaveRequested,
}

/// Autocomplete popup state.
#[derive(Debug, Clone, Default)]
pub struct Autocomplete {
    pub open: bool,
    pub items: Vec<String>,
    pub selected: usize,
    /// Token (prefix) being completed — extracted from the line up to cursor.
    pub token: String,
}

pub struct DslEditor {
    pub buffer: TextBuffer,
    pub autocomplete: Autocomplete,
    /// Original label that this editor was opened for — used as a hint when
    /// the editor is in single-env mode. `None` when authoring fresh.
    pub original_label: Option<String>,
}

impl DslEditor {
    /// Open editor with the given initial YAML body.
    pub fn new(initial_yaml: &str, original_label: Option<String>) -> Self {
        Self {
            buffer: TextBuffer::new(initial_yaml),
            autocomplete: Autocomplete::default(),
            original_label,
        }
    }

    /// Parse the buffer into `(label, entries)` pairs. Returns `Err` with
    /// the raw `serde_yaml` error on parse failure so the preview pane can
    /// render the error.
    pub fn parse_envs(&self) -> Result<BTreeMap<String, Vec<EnvEntry>>, String> {
        let raw = self.buffer.to_string();
        if raw.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        serde_yaml::from_str::<BTreeMap<String, Vec<EnvEntry>>>(&raw).map_err(|e| e.to_string())
    }

    /// Produce a flat resolution against the available items.
    pub fn resolve(&self, available_items: &[String]) -> Result<ResolutionOutput, String> {
        let envs = self.parse_envs()?;
        Ok(env_dsl::resolve_all(&envs, available_items))
    }

    /// Open or refresh the autocomplete popup. The corpus is generally the
    /// list of available item names plus their group prefixes.
    pub fn open_autocomplete(&mut self, corpus: &[String]) {
        let token = current_token(self.buffer.line_before_cursor());
        let items = if token.is_empty() {
            corpus.iter().take(10).cloned().collect()
        } else {
            fuzzy_top(corpus, token, 10)
        };
        self.autocomplete = Autocomplete {
            open: true,
            items,
            selected: 0,
            token: token.to_string(),
        };
    }

    pub fn close_autocomplete(&mut self) {
        self.autocomplete = Autocomplete::default();
    }

    fn accept_autocomplete(&mut self) {
        if !self.autocomplete.open || self.autocomplete.items.is_empty() {
            return;
        }
        let pick = self.autocomplete.items[self.autocomplete.selected].clone();
        // Replace the in-progress token with the picked value.
        for _ in 0..self.autocomplete.token.chars().count() {
            self.buffer.backspace();
        }
        self.buffer.insert_str(&pick);
        self.close_autocomplete();
    }

    /// Process a key event. The caller passes the corpus so autocomplete
    /// stays cheap (no allocation when the popup is closed).
    pub fn on_key(&mut self, key: KeyEvent, corpus: &[String]) -> DslEditorOutcome {
        // Autocomplete intercepts arrows / Tab / Enter / Esc when open.
        if self.autocomplete.open {
            match key.code {
                crossterm::event::KeyCode::Esc => {
                    self.close_autocomplete();
                    return DslEditorOutcome::Pending;
                }
                crossterm::event::KeyCode::Up => {
                    if self.autocomplete.selected > 0 {
                        self.autocomplete.selected -= 1;
                    }
                    return DslEditorOutcome::Pending;
                }
                crossterm::event::KeyCode::Down => {
                    if self.autocomplete.selected + 1 < self.autocomplete.items.len() {
                        self.autocomplete.selected += 1;
                    }
                    return DslEditorOutcome::Pending;
                }
                crossterm::event::KeyCode::Tab | crossterm::event::KeyCode::Enter => {
                    self.accept_autocomplete();
                    return DslEditorOutcome::Pending;
                }
                _ => {}
            }
            // Fall through: typing characters refreshes the popup below.
        }

        // Ctrl-Space → open autocomplete.
        if matches!(key.code, crossterm::event::KeyCode::Char(' '))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.open_autocomplete(corpus);
            return DslEditorOutcome::Pending;
        }
        // Ctrl-S → save.
        if matches!(key.code, crossterm::event::KeyCode::Char('s'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return DslEditorOutcome::SaveRequested;
        }
        // Esc → cancel (only when autocomplete is closed; handled above).
        if matches!(key.code, crossterm::event::KeyCode::Esc) {
            return DslEditorOutcome::Cancelled;
        }

        let handled = self.buffer.on_key(key);
        // Refresh autocomplete on character typing.
        if handled && self.autocomplete.open {
            let token = current_token(self.buffer.line_before_cursor());
            let items = fuzzy_top(corpus, token, 10);
            self.autocomplete.token = token.to_string();
            self.autocomplete.items = items;
            self.autocomplete.selected = 0;
        }
        DslEditorOutcome::Pending
    }
}

/// Extract the token currently being typed at the end of `line` — letters,
/// digits, dashes, underscores, slashes. Stops at whitespace, `:` or `,`.
fn current_token(line: &str) -> &str {
    let mut start = line.len();
    for (i, c) in line.char_indices().rev() {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == '/' {
            start = i;
        } else {
            break;
        }
    }
    &line[start..]
}

fn fuzzy_top(corpus: &[String], pattern_str: &str, n: usize) -> Vec<String> {
    if pattern_str.is_empty() {
        return corpus.iter().take(n).cloned().collect();
    }
    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let pattern = Pattern::parse(pattern_str, CaseMatching::Ignore, Normalization::Smart);
    let mut scored: Vec<(u32, String)> = corpus
        .iter()
        .filter_map(|c| {
            // Build a per-call buffer for ascii / unicode-safe scoring.
            let mut buf = Vec::new();
            let h = nucleo_matcher::Utf32Str::new(c.as_str(), &mut buf);
            pattern.score(h, &mut matcher).map(|s| (s, c.clone()))
        })
        .collect();
    scored.sort_by_key(|item| std::cmp::Reverse(item.0));
    scored.into_iter().take(n).map(|(_, s)| s).collect()
}

/// Utility: produce a "tree" preview from a parsed envs map. Currently
/// unused by the live preview path (which prefers flat KEY=value pairs)
/// but exposed for callers that want the legacy nested view.
#[allow(dead_code)]
pub fn build_tree_preview(
    envs: &BTreeMap<String, Vec<EnvEntry>>,
    available_items: &[String],
) -> Vec<(String, EnvNode)> {
    let mut out = Vec::new();
    for label in envs.keys() {
        if let Ok(node) = crate::config::env_resolver::resolve(envs, label, available_items) {
            out.push((label.clone(), node));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_buffer_returns_empty_map() {
        let ed = DslEditor::new("", None);
        assert!(ed.parse_envs().unwrap().is_empty());
    }

    #[test]
    fn parse_simple_yaml_returns_envs() {
        let ed = DslEditor::new("dev:\n  - dev/api-key\n", None);
        let envs = ed.parse_envs().unwrap();
        assert_eq!(envs.len(), 1);
        let entries = envs.get("dev").unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn current_token_picks_trailing_word() {
        assert_eq!(current_token("- some-iten"), "some-iten");
        assert_eq!(current_token("  KEY: dev/"), "dev/");
        assert_eq!(current_token(""), "");
    }

    #[test]
    fn fuzzy_top_returns_matches() {
        let corpus = vec![
            "dev/api-key".to_string(),
            "prod/api-key".to_string(),
            "dev/db-pass".to_string(),
        ];
        let hits = fuzzy_top(&corpus, "api", 10);
        assert!(hits.iter().any(|s| s.contains("api")));
    }

    #[test]
    fn resolve_round_trip_with_brace_expansion() {
        // Quote the entry so YAML doesn't parse `{}/db` as a flow mapping.
        let yaml = "env-{dev,prod}:\n  - \"{}/db\"\n";
        let ed = DslEditor::new(yaml, None);
        let items = vec!["dev/db".to_string(), "prod/db".to_string()];
        let out = ed.resolve(&items).unwrap();
        assert_eq!(out.pairs.len(), 2);
    }
}
