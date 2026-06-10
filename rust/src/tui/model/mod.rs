//! Pure state modules backing the result-list views.
//!
//! Graduated out of the search view (2026-06-09 architecture review):
//! results in → rows out, no ratatui imports, unit-testable without a
//! terminal. Views own rendering; these modules own the row model.

pub mod path_folding;
pub mod result_sort;
