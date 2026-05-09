//! Floating autocomplete popup for "typing a reference to a secret" surfaces.
//!
//! Backed by [`crate::suggest`] so the same Levenshtein code that produces the
//! CLI "did you mean" hint also drives this popup. The widget is intentionally
//! dumb: it owns the corpus and the current query, but it is non-modal — the
//! host view keeps every key event and merely calls [`update_query`] /
//! [`move_selection`] / [`accepted`] as appropriate.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

use crate::suggest;
use crate::tui::theme;

/// Maximum number of entries the popup ever shows. Five is enough to surface
/// realistic alternatives without crowding the underlying view.
const MAX_SUGGESTIONS: usize = 5;

/// Reusable autocomplete popup for secret-path inputs.
pub struct SecretRefAutocomplete {
    corpus: Vec<String>,
    query: String,
    suggestions: Vec<String>,
    selected: usize,
    open: bool,
}

impl SecretRefAutocomplete {
    pub fn new(corpus: Vec<String>) -> Self {
        Self {
            corpus,
            query: String::new(),
            suggestions: Vec::new(),
            selected: 0,
            open: false,
        }
    }

    /// Replace the corpus that suggestions are computed against. Re-runs the
    /// matcher so the visible list stays in sync.
    pub fn set_corpus(&mut self, corpus: Vec<String>) {
        self.corpus = corpus;
        self.recompute();
    }

    /// Update the query string and recompute suggestions. The popup auto-opens
    /// when there is at least one suggestion to show; consumers can still
    /// force it closed via [`set_open`].
    pub fn update_query(&mut self, q: &str) {
        if self.query == q {
            return;
        }
        self.query = q.to_string();
        self.recompute();
    }

    /// Toggle the popup visibility. Useful when the host wires Tab/Ctrl-Space
    /// to dismiss the popup without touching the query.
    pub fn set_open(&mut self, open: bool) {
        self.open = open && !self.suggestions.is_empty();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Move the selection cursor by `delta`, wrapping around at both ends.
    /// No-op when the popup is closed or the suggestion list is empty.
    pub fn move_selection(&mut self, delta: isize) {
        if !self.open || self.suggestions.is_empty() {
            return;
        }
        let len = self.suggestions.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len);
        self.selected = next as usize;
    }

    /// The currently highlighted suggestion, if any. Returns `None` when the
    /// popup is closed or no candidates survived the filter.
    pub fn accepted(&self) -> Option<&str> {
        if !self.open {
            return None;
        }
        self.suggestions.get(self.selected).map(String::as_str)
    }

    /// Render the popup anchored to `anchor`. Drawn below the anchor when
    /// space allows, otherwise above. When the popup is closed or empty this
    /// is a no-op.
    pub fn draw(&self, frame: &mut Frame<'_>, anchor: Rect) {
        if !self.open || self.suggestions.is_empty() {
            return;
        }

        let frame_area = frame.area();
        let height = (self.suggestions.len() as u16 + 2).min(MAX_SUGGESTIONS as u16 + 2);
        let width = anchor.width.max(20);

        let space_below = frame_area.height.saturating_sub(anchor.y + anchor.height);
        let (y, popup_h) = if space_below >= height {
            (anchor.y + anchor.height, height)
        } else if anchor.y >= height {
            (anchor.y.saturating_sub(height), height)
        } else {
            // Cramped: clamp to whatever fits below.
            (anchor.y + anchor.height, space_below.max(2))
        };
        let popup = Rect {
            x: anchor.x,
            y,
            width,
            height: popup_h,
        };

        // Title differentiates "exact prefix exists" from "did you mean":
        // when none of the suggestions start with the query we treat it as a
        // typo-recovery hint, otherwise as a live completion.
        let title = if self.suggestions.iter().any(|s| s.starts_with(&self.query)) {
            " matches "
        } else {
            " did you mean "
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme::border()))
            .title(title)
            .title_style(Style::default().fg(theme::border_label()));

        let items: Vec<ListItem> = self
            .suggestions
            .iter()
            .map(|s| ListItem::new(Line::from(Span::raw(s.clone()))))
            .collect();

        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(theme::accent())
                .fg(theme::on_accent())
                .add_modifier(Modifier::BOLD),
        );

        let mut state = ListState::default();
        state.select(Some(self.selected));

        frame.render_widget(Clear, popup);
        frame.render_stateful_widget(list, popup, &mut state);
    }

    fn recompute(&mut self) {
        let max_dist = suggest::default_max_distance(&self.query);
        self.suggestions =
            suggest::suggest_top_n(&self.query, &self.corpus, MAX_SUGGESTIONS, max_dist);
        if self.selected >= self.suggestions.len() {
            self.selected = 0;
        }
        // Empty-query / empty-corpus drive the popup closed; otherwise the
        // host decides whether to show it (gated behind Tab/Ctrl-Space).
        if self.suggestions.is_empty() || self.query.is_empty() {
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<String> {
        vec![
            "prod/api/STRIPE_KEY".to_string(),
            "prod/api/GITHUB_TOKEN".to_string(),
            "prod/db/POSTGRES_URL".to_string(),
            "staging/api/STRIPE_KEY".to_string(),
        ]
    }

    #[test]
    fn empty_corpus_yields_no_suggestions() {
        let mut ac = SecretRefAutocomplete::new(Vec::new());
        ac.update_query("anything");
        ac.set_open(true);
        assert!(ac.accepted().is_none());
        assert!(!ac.is_open());
    }

    #[test]
    fn exact_match_appears_first() {
        let mut ac = SecretRefAutocomplete::new(corpus());
        ac.update_query("prod/api/STRIPE_KEY");
        ac.set_open(true);
        assert_eq!(ac.accepted(), Some("prod/api/STRIPE_KEY"));
    }

    #[test]
    fn typo_within_threshold_is_suggested() {
        let mut ac = SecretRefAutocomplete::new(corpus());
        ac.update_query("prod/api/STRIPE_KYE");
        ac.set_open(true);
        assert_eq!(ac.accepted(), Some("prod/api/STRIPE_KEY"));
    }

    #[test]
    fn typo_outside_threshold_yields_nothing() {
        let mut ac = SecretRefAutocomplete::new(corpus());
        ac.update_query("totally_unrelated_thing_with_no_overlap");
        ac.set_open(true);
        assert!(ac.accepted().is_none());
        assert!(!ac.is_open());
    }

    #[test]
    fn move_selection_wraps_around() {
        // A small synthetic corpus with several entries within distance ≤ 2
        // of the query so we can exercise wrap-around deterministically.
        let local = vec![
            "alpha".to_string(),
            "alphas".to_string(),
            "alpine".to_string(),
        ];
        let mut ac = SecretRefAutocomplete::new(local);
        ac.update_query("alpha");
        ac.set_open(true);
        let first = ac.accepted().unwrap().to_string();
        ac.move_selection(-1);
        let last = ac.accepted().unwrap().to_string();
        assert_ne!(first, last, "wrap should land on a different entry");
        // Wrap forward should return to the first entry.
        ac.move_selection(1);
        assert_eq!(ac.accepted().map(String::from), Some(first));
    }

    #[test]
    fn closed_popup_returns_no_acceptance() {
        let mut ac = SecretRefAutocomplete::new(corpus());
        ac.update_query("prod");
        // Default state (open=false) means accepted() is silent.
        assert!(ac.accepted().is_none());
    }

    #[test]
    fn set_corpus_refreshes_suggestions() {
        let mut ac = SecretRefAutocomplete::new(Vec::new());
        ac.update_query("alpha");
        ac.set_open(true);
        assert!(!ac.is_open());

        ac.set_corpus(vec!["alpha".to_string(), "alphas".to_string()]);
        ac.set_open(true);
        assert_eq!(ac.accepted(), Some("alpha"));
    }
}
