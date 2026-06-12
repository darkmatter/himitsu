//! ResultSort — column/direction ordering over search results.
//!
//! Pure state module: comparators and sort state only, no rendering. The
//! search view consumes it directly; [`super::path_folding`] applies it
//! inside each store bucket.

use crate::cli::search::SearchResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchColumn {
    Path,
    Updated,
    Tags,
    Store,
}

impl SearchColumn {
    pub fn label(self) -> &'static str {
        match self {
            SearchColumn::Path => "PATH",
            SearchColumn::Updated => "UPDATED",
            SearchColumn::Tags => "TAGS",
            SearchColumn::Store => "STORE",
        }
    }

    pub fn base_columns() -> &'static [SearchColumn] {
        &[
            SearchColumn::Path,
            SearchColumn::Updated,
            SearchColumn::Tags,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub fn toggled(self) -> Self {
        match self {
            SortDirection::Asc => SortDirection::Desc,
            SortDirection::Desc => SortDirection::Asc,
        }
    }

    pub fn marker(self) -> char {
        match self {
            SortDirection::Asc => '^',
            SortDirection::Desc => 'v',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortState {
    pub column: SearchColumn,
    pub direction: SortDirection,
}

pub fn sort_results(results: &mut [SearchResult], sort_state: SortState) {
    results.sort_by(|a, b| compare_results(a, b, sort_state));
}

/// Compare two results by the active column + direction, with a stable
/// `(store, path)` tiebreak so equal keys keep a deterministic order.
pub fn compare_results(
    a: &SearchResult,
    b: &SearchResult,
    sort_state: SortState,
) -> std::cmp::Ordering {
    let primary = match sort_state.column {
        SearchColumn::Path => a.path.cmp(&b.path),
        SearchColumn::Updated => result_timestamp(a).cmp(result_timestamp(b)),
        SearchColumn::Tags => result_tags(a).cmp(result_tags(b)),
        SearchColumn::Store => a.store.cmp(&b.store),
    };
    let primary = match sort_state.direction {
        SortDirection::Asc => primary,
        SortDirection::Desc => primary.reverse(),
    };
    primary
        .then_with(|| a.store.cmp(&b.store))
        .then_with(|| a.path.cmp(&b.path))
}

pub fn compare_strings(a: &str, b: &str, direction: SortDirection) -> std::cmp::Ordering {
    match direction {
        SortDirection::Asc => a.cmp(b),
        SortDirection::Desc => b.cmp(a),
    }
}

/// The timestamp a result sorts by: `updated_at`, falling back to
/// `created_at`, then empty (sorts first ascending).
pub fn result_timestamp(result: &SearchResult) -> &str {
    result
        .updated_at
        .as_deref()
        .or(result.created_at.as_deref())
        .unwrap_or("")
}

/// The tag a result sorts by: its first tag, or empty.
pub fn result_tags(result: &SearchResult) -> &str {
    result
        .tags
        .as_deref()
        .and_then(|tags| tags.first())
        .map(String::as_str)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(path: &str, store: &str, updated: Option<&str>, tag: Option<&str>) -> SearchResult {
        SearchResult {
            path: path.to_string(),
            store: store.to_string(),
            updated_at: updated.map(str::to_string),
            tags: tag.map(|t| vec![t.to_string()]),
            ..Default::default()
        }
    }

    #[test]
    fn sorts_by_path_in_both_directions() {
        let mut rs = vec![result("b", "s", None, None), result("a", "s", None, None)];
        sort_results(
            &mut rs,
            SortState {
                column: SearchColumn::Path,
                direction: SortDirection::Asc,
            },
        );
        assert_eq!(rs[0].path, "a");
        sort_results(
            &mut rs,
            SortState {
                column: SearchColumn::Path,
                direction: SortDirection::Desc,
            },
        );
        assert_eq!(rs[0].path, "b");
    }

    #[test]
    fn updated_falls_back_to_created_and_ties_break_on_store_then_path() {
        let mut a = result("x", "s2", None, None);
        a.created_at = Some("2026-01-01".into());
        let b = result("x", "s1", Some("2026-01-01"), None);
        // Same effective timestamp: tiebreak puts store s1 first.
        let mut rs = vec![a, b];
        sort_results(
            &mut rs,
            SortState {
                column: SearchColumn::Updated,
                direction: SortDirection::Asc,
            },
        );
        assert_eq!(rs[0].store, "s1");
    }

    #[test]
    fn tag_sort_uses_first_tag_and_direction_flip_is_involutive() {
        let dir = SortDirection::Asc;
        assert_eq!(dir.toggled().toggled(), dir);

        let a = result("a", "s", None, Some("zeta"));
        let b = result("b", "s", None, Some("alpha"));
        let mut rs = vec![a, b];
        sort_results(
            &mut rs,
            SortState {
                column: SearchColumn::Tags,
                direction: SortDirection::Asc,
            },
        );
        assert_eq!(rs[0].path, "b");
    }
}
