//! StoreHealth — git/recipient health of a store checkout, with its pill
//! rendering. Graduated out of the search view (2026-06-09 architecture
//! review): the widget owns the health fetch and the pill presentation;
//! host views own layout.

use ratatui::style::Style;
use ratatui::text::Span;

use crate::cli::Context;
use crate::tui::{icons, theme};

/// Health status of a store's git checkout, computed once at view
/// construction. Displayed as a compact indicator in the header bar.
#[derive(Debug, Clone)]
pub enum StoreHealth {
    /// Store checkout is up to date with its remote tracking branch.
    Synced,
    /// Local checkout is behind its remote by N commit(s).
    Behind(u32),
    /// Working tree has uncommitted local changes.
    Dirty,
    /// Both behind remote AND has local changes.
    BehindAndDirty(u32),
    /// Store directory is not a git repo.
    NotGit,
    /// Git repo exists but has no remote configured.
    NoRemote,
    /// Git repo has a remote but the tracking branch doesn't exist yet
    /// (e.g. never pushed).
    NotPushed,
    /// User's own age key is not in the store's recipient list.
    NotRecipient,
    /// Could not determine status for some other reason.
    Unknown,
}

/// Render the active checkout's health; the scope is shown separately.
pub fn render_health_pill(label: &str, health: &StoreHealth) -> Vec<Span<'static>> {
    let (status, color) = match health {
        StoreHealth::Synced => ("synced".to_string(), theme::success()),
        StoreHealth::Behind(n) => (format!("{n} behind"), theme::warning()),
        StoreHealth::Dirty => ("dirty".to_string(), theme::danger()),
        StoreHealth::BehindAndDirty(n) => (format!("{n} behind + dirty"), theme::danger()),
        StoreHealth::NotGit => ("not a git repo".to_string(), theme::warning()),
        StoreHealth::NoRemote => ("no remote".to_string(), theme::warning()),
        StoreHealth::NotPushed => ("not pushed".to_string(), theme::warning()),
        StoreHealth::NotRecipient => ("not a recipient".to_string(), theme::warning()),
        StoreHealth::Unknown => ("unknown".to_string(), theme::muted()),
    };
    let body = format!("{label}: {status}");
    if matches!(health, StoreHealth::Synced) {
        // Quiet steady-state: dot + label on the default background. No
        // bright pill for the happy path.
        vec![
            Span::styled(icons::health(), Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(body, Style::default().fg(color)),
        ]
    } else {
        theme::pill_with(
            format!("{} {body}", icons::health()),
            color,
            theme::on_accent(),
        )
    }
}

/// Check the git health of a store checkout (offline — no fetch).
///
/// Returns a [`StoreHealth`] summarising whether the checkout is behind its
/// remote tracking branch and/or has uncommitted local changes. Also checks
/// whether the user's own age key is in the store's recipient list —
/// [`StoreHealth::NotRecipient`] takes priority over git health because the
/// store is unusable without it.
pub fn check_store_health(ctx: &Context) -> StoreHealth {
    use crate::git;

    let store_path = &ctx.store;

    if let Some(override_health) = store_health_override() {
        return override_health;
    }

    if store_path.as_os_str().is_empty() {
        return StoreHealth::Unknown;
    }

    // Recipient membership check — takes priority because the store is
    // unusable (can't decrypt) if you're not a recipient.
    if !crate::cli::join::is_self_recipient(ctx) {
        return StoreHealth::NotRecipient;
    }

    if !store_path.join(".git").exists() {
        return StoreHealth::NotGit;
    }

    // Current branch name
    let branch = match git::run(&["rev-parse", "--abbrev-ref", "HEAD"], store_path) {
        Ok(b) => b.trim().to_string(),
        Err(_) => return StoreHealth::Unknown,
    };

    // Check if any remote is configured at all
    let has_remote = git::run(&["remote"], store_path)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !has_remote {
        return StoreHealth::NoRemote;
    }

    // Check remote tracking branch exists
    let remote_ref = format!("origin/{branch}");
    if git::run(&["rev-parse", "--verify", &remote_ref], store_path).is_err() {
        return StoreHealth::NotPushed;
    }

    // Behind count
    let behind: u32 = git::run(
        &["rev-list", "--count", &format!("HEAD..{remote_ref}")],
        store_path,
    )
    .ok()
    .and_then(|s| s.trim().parse().ok())
    .unwrap_or(0);

    // Dirty working tree
    let dirty = git::run(&["status", "--short"], store_path)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    match (behind > 0, dirty) {
        (true, true) => StoreHealth::BehindAndDirty(behind),
        (true, false) => StoreHealth::Behind(behind),
        (false, true) => StoreHealth::Dirty,
        (false, false) => StoreHealth::Synced,
    }
}

fn store_health_override() -> Option<StoreHealth> {
    let raw = std::env::var("HIMITSU_TUI_STORE_HEALTH").ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "synced" => Some(StoreHealth::Synced),
        "no-remote" | "no_remote" => Some(StoreHealth::NoRemote),
        "not-pushed" | "not_pushed" => Some(StoreHealth::NotPushed),
        "not-git" | "not_git" => Some(StoreHealth::NotGit),
        "not-recipient" | "not_recipient" => Some(StoreHealth::NotRecipient),
        "dirty" => Some(StoreHealth::Dirty),
        "unknown" => Some(StoreHealth::Unknown),
        _ => None,
    }
}
