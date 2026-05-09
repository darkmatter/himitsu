//! Levenshtein-based "did you mean" suggestion helpers.
//!
//! Shared by the `himitsu search` CLI (to print a suggestion when a query
//! returns zero hits) and the TUI autocomplete popup (which surfaces the
//! top-N closest secret paths as the user types a reference).
//!
//! Levenshtein is implemented inline against UTF-8 byte slices: a small DP
//! table is plenty for the single-millisecond budget search has, and pulling
//! in a fuzzy-match crate (`strsim` etc.) for one function is overkill.

/// Case-sensitive byte-Levenshtein distance between two strings.
///
/// Uses two rolling rows of size `b.len() + 1` so peak memory stays
/// `O(min(a, b))`. Operates on bytes, not Unicode codepoints — every existing
/// caller works with ASCII paths (`prod/api/STRIPE_KEY`) so paying the
/// cost of grapheme segmentation would be wasted.
pub fn levenshtein(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];

    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b.len()]
}

/// Heuristic for the maximum "reasonable" edit distance for an `input`.
///
/// Short queries (<6 chars) fall back to a hard floor of 2 so a 3-letter
/// typo doesn't go un-suggested. Longer inputs scale linearly at one third
/// of the input length, keeping suggestions tight on long paths where every
/// edit is expensive to type by accident.
pub fn default_max_distance(input: &str) -> usize {
    (input.len() / 3).max(2)
}

/// Closest single candidate to `input` within `max_distance`.
///
/// Returns `None` when `candidates` is empty or the best match exceeds
/// `max_distance`. Ties are broken lexicographically so the result is
/// deterministic across runs.
pub fn suggest_closest<'a>(
    input: &str,
    candidates: &'a [String],
    max_distance: usize,
) -> Option<&'a String> {
    candidates
        .iter()
        .map(|c| (levenshtein(input, c), c))
        .filter(|(d, _)| *d <= max_distance)
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, c)| c)
}

/// Up to `n` closest candidates within `max_distance`, sorted by ascending
/// distance with lexicographic tie-break.
///
/// Used by the TUI autocomplete popup to render a short ranked list as the
/// user types. Returns owned `String`s so the caller can stash the snapshot
/// without juggling lifetimes against the corpus.
pub fn suggest_top_n(
    input: &str,
    candidates: &[String],
    n: usize,
    max_distance: usize,
) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &String)> = candidates
        .iter()
        .map(|c| (levenshtein(input, c), c))
        .filter(|(d, _)| *d <= max_distance)
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    scored.into_iter().take(n).map(|(_, c)| c.clone()).collect()
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
    fn levenshtein_identical_is_zero() {
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("", ""), 0);
    }

    #[test]
    fn levenshtein_empty_input_returns_other_length() {
        assert_eq!(levenshtein("", "abcd"), 4);
        assert_eq!(levenshtein("abcd", ""), 4);
    }

    #[test]
    fn levenshtein_off_by_one_substitution() {
        assert_eq!(levenshtein("kitten", "sitten"), 1);
    }

    #[test]
    fn levenshtein_off_by_one_insertion() {
        assert_eq!(levenshtein("cat", "cats"), 1);
        assert_eq!(levenshtein("cats", "cat"), 1);
    }

    #[test]
    fn levenshtein_transposition_costs_two_edits() {
        // Standard Levenshtein (no Damerau) treats a transposition as
        // two operations: delete + insert.
        assert_eq!(levenshtein("ab", "ba"), 2);
    }

    #[test]
    fn levenshtein_classic_examples() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
    }

    #[test]
    fn levenshtein_is_case_sensitive() {
        assert_eq!(levenshtein("ABC", "abc"), 3);
    }

    #[test]
    fn default_max_distance_floor_is_two() {
        assert_eq!(default_max_distance(""), 2);
        assert_eq!(default_max_distance("ab"), 2);
        assert_eq!(default_max_distance("abcde"), 2);
    }

    #[test]
    fn default_max_distance_scales_with_length() {
        assert_eq!(default_max_distance("abcdef"), 2);
        assert_eq!(default_max_distance("abcdefghi"), 3);
        assert_eq!(default_max_distance("a".repeat(30).as_str()), 10);
    }

    #[test]
    fn suggest_closest_returns_none_for_empty_corpus() {
        let candidates: Vec<String> = Vec::new();
        assert_eq!(suggest_closest("foo", &candidates, 2), None);
    }

    #[test]
    fn suggest_closest_picks_nearest_within_threshold() {
        let candidates = corpus();
        // Typo: "stripe_kye" → close to "STRIPE_KEY" but case differs, so
        // a case-sensitive query against the lower-cased basename works
        // best. Use an upper-case typo to keep distance small.
        let hit = suggest_closest("prod/api/STRIPE_KYE", &candidates, 4);
        assert_eq!(hit.map(String::as_str), Some("prod/api/STRIPE_KEY"));
    }

    #[test]
    fn suggest_closest_returns_none_above_threshold() {
        let candidates = corpus();
        let hit = suggest_closest("totally/different/path", &candidates, 2);
        assert_eq!(hit, None);
    }

    #[test]
    fn suggest_closest_breaks_ties_lexicographically() {
        // Both candidates are exactly one substitution from "ab"; lexicographic
        // tie-break should prefer "ax" over "az".
        let candidates = vec!["az".to_string(), "ax".to_string()];
        let hit = suggest_closest("ab", &candidates, 2);
        assert_eq!(hit.map(String::as_str), Some("ax"));
    }

    #[test]
    fn suggest_top_n_returns_sorted_distance_lex() {
        let candidates = vec![
            "alpha".to_string(),
            "alpine".to_string(),
            "alphas".to_string(),
            "beta".to_string(),
        ];
        let hits = suggest_top_n("alpha", &candidates, 3, 4);
        // "alpha" is exact (0); "alphas" / "alpine" each cost ≥1.
        assert_eq!(hits[0], "alpha");
        // Remaining two should be sorted by distance, then lexicographic.
        assert!(hits.len() == 3);
        assert!(hits.contains(&"alphas".to_string()));
        assert!(hits.contains(&"alpine".to_string()));
        assert!(!hits.contains(&"beta".to_string()));
    }

    #[test]
    fn suggest_top_n_caps_at_n() {
        let candidates = corpus();
        let hits = suggest_top_n("prod/api/STRIPE_KEY", &candidates, 2, 50);
        assert!(hits.len() <= 2);
    }

    #[test]
    fn suggest_top_n_with_zero_returns_empty() {
        let candidates = corpus();
        let hits = suggest_top_n("prod", &candidates, 0, 50);
        assert!(hits.is_empty());
    }

    #[test]
    fn suggest_top_n_filters_outside_threshold() {
        let candidates = vec!["totally_unrelated".to_string()];
        let hits = suggest_top_n("xyz", &candidates, 5, 2);
        assert!(hits.is_empty());
    }
}
