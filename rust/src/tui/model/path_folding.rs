//! PathFolding — collapse/expand of secret paths into prefix groups.
//!
//! Pure state module: a flat result list in, display rows out. Owns the
//! store-bucket partitioning, the prefix grouping (a "group" is any
//! top-level path segment with ≥ 2 leaves), and the folded/unfolded row
//! shapes. No ratatui imports — views own rendering.

use crate::cli::search::SearchResult;

use super::result_sort::{compare_strings, sort_results, SearchColumn, SortDirection, SortState};

/// A row in the rendered results list. `Store` headers group secrets by
/// origin (`org/repo` slug or local path) and are never selectable;
/// navigation steps over them. `FoldedGroup` rows appear only in folded
/// mode, one per top-level path prefix shared by ≥ 2 secrets — they
/// collapse the group's leaves into a single selectable row that expands
/// when the user unfolds.
#[derive(Debug, Clone)]
pub enum Row {
    Store {
        name: String,
        count: usize,
    },
    FoldedGroup {
        /// Top-level path segment shared by the collapsed leaves.
        prefix: String,
        /// Number of leaves under this prefix.
        count: usize,
        /// Indentation depth (matches what its children would have if
        /// expanded). 0 in single-store mode, 1 under a `Store` header.
        indent: usize,
    },
    Secret {
        result: SearchResult,
        /// Indentation depth in list-item cells (2 spaces per level).
        /// 0 in single-store mode, 1 under a `Store` header.
        indent: usize,
        /// Top-level path segment when this secret shares a prefix with
        /// ≥ 1 sibling in the same store. The renderer paints this segment
        /// with a subtle accent so the visual grouping survives without a
        /// separate header row. `None` for singletons.
        shared_prefix: Option<String>,
    },
}

/// Group a flat list of results into rows.
///
/// When results span **multiple stores**, rows are partitioned per-store with
/// a `Store` header row per bucket; within each bucket we apply path-prefix
/// grouping. When only one store is present we fall back to the single-store
/// layout (no store header).
///
/// A "group" is any top-level path segment that contains ≥ 2 leaves. In
/// folded mode each such group collapses to a single `FoldedGroup` row; in
/// unfolded mode the leaves render inline with their shared prefix tagged so
/// the renderer can paint it in a subtle accent. Singletons render the same
/// in both modes. The active sort column controls ordering inside each store;
/// store headers stay grouped for readability.
pub fn build_rows(results: &[SearchResult], folded: bool, sort_state: SortState) -> Vec<Row> {
    use std::collections::BTreeMap;

    let mut by_store: BTreeMap<String, Vec<SearchResult>> = BTreeMap::new();
    for r in results {
        by_store.entry(r.store.clone()).or_default().push(r.clone());
    }

    let multi_store = by_store.len() > 1;
    let mut rows = Vec::new();
    let mut store_names: Vec<String> = by_store.keys().cloned().collect();
    if sort_state.column == SearchColumn::Store && sort_state.direction == SortDirection::Desc {
        store_names.reverse();
    }
    for store_name in store_names {
        let bucket = by_store.remove(&store_name).unwrap_or_default();
        if multi_store {
            rows.push(Row::Store {
                name: store_name.clone(),
                count: bucket.len(),
            });
        }
        append_prefix_grouped_rows(&mut rows, bucket, multi_store, folded, sort_state);
    }
    rows
}

/// Append `bucket` rows to `rows` applying path-prefix grouping.
///
/// `under_store_header` adds one level of indent so each store's children
/// visually nest. `folded` collapses ≥ 2-leaf groups into `FoldedGroup` rows.
fn append_prefix_grouped_rows(
    rows: &mut Vec<Row>,
    mut bucket: Vec<SearchResult>,
    under_store_header: bool,
    folded: bool,
    sort_state: SortState,
) {
    use std::collections::HashMap;

    let store_indent: usize = if under_store_header { 1 } else { 0 };
    let bucket_sort = if sort_state.column == SearchColumn::Store {
        SortState {
            column: SearchColumn::Path,
            direction: SortDirection::Asc,
        }
    } else {
        sort_state
    };
    sort_results(&mut bucket, bucket_sort);

    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<SearchResult>> = HashMap::new();
    for r in bucket {
        let prefix = prefix_of(&r.path).to_string();
        if !groups.contains_key(&prefix) {
            order.push(prefix.clone());
        }
        groups.entry(prefix).or_default().push(r);
    }

    let mut folders: Vec<(String, Vec<SearchResult>)> = Vec::new();
    let mut singles: Vec<(String, Vec<SearchResult>)> = Vec::new();
    for name in order {
        let items = groups.remove(&name).unwrap_or_default();
        if items.len() >= 2 {
            folders.push((name, items));
        } else {
            singles.push((name, items));
        }
    }
    if bucket_sort.column == SearchColumn::Path {
        folders.sort_by(|a, b| compare_strings(&a.0, &b.0, bucket_sort.direction));
        singles.sort_by(|a, b| compare_strings(&a.0, &b.0, bucket_sort.direction));
    }

    for (prefix, items) in folders {
        if folded {
            rows.push(Row::FoldedGroup {
                prefix,
                count: items.len(),
                indent: store_indent,
            });
            continue;
        }
        let shared = Some(prefix);
        for result in items {
            rows.push(Row::Secret {
                result,
                indent: store_indent,
                shared_prefix: shared.clone(),
            });
        }
    }
    for (_, items) in singles {
        for result in items {
            rows.push(Row::Secret {
                result,
                indent: store_indent,
                shared_prefix: None,
            });
        }
    }
}

/// Top-level path segment of a secret's path, used for prefix grouping.
pub fn prefix_of(path: &str) -> &str {
    match path.split_once('/') {
        Some((head, _)) => head,
        None => path,
    }
}

/// Split `parent` (the slash-terminated path prefix in front of a secret's
/// basename) into a leading "shared" segment and the remainder. The shared
/// segment is `"<prefix>/"` when the leaf is part of a multi-leaf group;
/// otherwise the entire parent stays in the second slot for the dimmed
/// renderer to draw as before.
pub fn split_shared_prefix<'a>(parent: &'a str, shared: Option<&str>) -> (&'a str, &'a str) {
    let Some(prefix) = shared else {
        return ("", parent);
    };
    let head = format!("{prefix}/");
    if parent.starts_with(&head) {
        parent.split_at(head.len())
    } else {
        ("", parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(path: &str, store: &str) -> SearchResult {
        SearchResult {
            path: path.to_string(),
            store: store.to_string(),
            ..Default::default()
        }
    }

    fn default_sort() -> SortState {
        SortState {
            column: SearchColumn::Path,
            direction: SortDirection::Asc,
        }
    }

    #[test]
    fn folded_mode_collapses_multi_leaf_groups_only() {
        let results = vec![
            result("prod/a", "s"),
            result("prod/b", "s"),
            result("lonely", "s"),
        ];
        let rows = build_rows(&results, true, default_sort());
        // One FoldedGroup for prod (2 leaves), one Secret for the singleton.
        assert_eq!(rows.len(), 2);
        match &rows[0] {
            Row::FoldedGroup { prefix, count, .. } => {
                assert_eq!(prefix, "prod");
                assert_eq!(*count, 2);
            }
            other => panic!("expected FoldedGroup, got {other:?}"),
        }
        assert!(matches!(&rows[1], Row::Secret { shared_prefix: None, .. }));
    }

    #[test]
    fn unfolded_mode_tags_shared_prefixes_inline() {
        let results = vec![result("prod/a", "s"), result("prod/b", "s")];
        let rows = build_rows(&results, false, default_sort());
        assert_eq!(rows.len(), 2);
        for row in &rows {
            match row {
                Row::Secret { shared_prefix, .. } => {
                    assert_eq!(shared_prefix.as_deref(), Some("prod"));
                }
                other => panic!("expected Secret, got {other:?}"),
            }
        }
    }

    #[test]
    fn multi_store_results_get_headers_and_indent() {
        let results = vec![result("a", "s1"), result("b", "s2")];
        let rows = build_rows(&results, false, default_sort());
        assert_eq!(rows.len(), 4);
        assert!(matches!(&rows[0], Row::Store { name, count: 1 } if name == "s1"));
        assert!(matches!(&rows[1], Row::Secret { indent: 1, .. }));
        assert!(matches!(&rows[2], Row::Store { name, count: 1 } if name == "s2"));
    }

    #[test]
    fn store_sort_desc_reverses_store_buckets_not_leaves() {
        let results = vec![result("a", "s1"), result("b", "s2")];
        let rows = build_rows(
            &results,
            false,
            SortState {
                column: SearchColumn::Store,
                direction: SortDirection::Desc,
            },
        );
        assert!(matches!(&rows[0], Row::Store { name, .. } if name == "s2"));
    }

    #[test]
    fn split_shared_prefix_only_strips_matching_heads() {
        assert_eq!(split_shared_prefix("prod/", Some("prod")), ("prod/", ""));
        assert_eq!(split_shared_prefix("prod/", None), ("", "prod/"));
        assert_eq!(split_shared_prefix("other/", Some("prod")), ("", "other/"));
    }

    #[test]
    fn prefix_of_takes_head_segment() {
        assert_eq!(prefix_of("prod/db/KEY"), "prod");
        assert_eq!(prefix_of("KEY"), "KEY");
    }
}
