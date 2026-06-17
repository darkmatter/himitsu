//! Test-only helpers for mutating process environment variables.
//!
//! Rust 2024 made [`std::env::set_var`] and [`std::env::remove_var`] unsafe because
//! they can race with concurrent readers. Unit tests here are single-threaded.

use std::ffi::OsStr;

/// Set an environment variable in tests.
pub(crate) fn set_var<K: AsRef<OsStr>, V: AsRef<OsStr>>(key: K, value: V) {
    // SAFETY: test-only; callers do not spawn threads that read env concurrently.
    unsafe { std::env::set_var(key, value) }
}

/// Remove an environment variable in tests.
pub(crate) fn remove_var<K: AsRef<OsStr>>(key: K) {
    // SAFETY: test-only; callers do not spawn threads that read env concurrently.
    unsafe { std::env::remove_var(key) }
}
