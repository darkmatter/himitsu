//! In-process ratatui TUI for himitsu.
//!
//! Entry points:
//! - [`run`] — launched when the user runs `himitsu` with no subcommand. Opens
//!   the init wizard if no age key exists, otherwise the dashboard.
//! - [`run_init_flow`] — launched from the TTY path of `himitsu init`. Always
//!   starts on the init wizard, then advances to the dashboard on success.

mod app;
mod event;
pub mod forms;
#[cfg(test)]
mod harness;
mod hint;
mod icons;
pub mod keymap;
mod terminal;
mod theme;
mod toast;
mod views;
pub mod widgets;

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{Event, KeyEventKind};

use crate::cli::{init, Context};
use crate::config::{config_path, Config};
use crate::error::Result;
use views::init_wizard::{InitWizardView, Outcome};

pub fn run(ctx: &Context) -> Result<()> {
    // First-run flow: if no age key or store exists, drop straight into the
    // wizard instead of rendering an empty dashboard.
    if should_launch_init_flow(ctx) {
        return run_init_flow();
    }

    // Load TUI settings from the user's global config. A missing file yields
    // the defaults; malformed `tui` settings surface as hard errors so the
    // user sees typos immediately instead of silently losing customization.
    let tui = Config::load(&config_path())?.tui;
    theme::set_theme(&tui.theme)?;
    icons::set_use_nerd_fonts(tui.nerd_fonts);
    let keymap = tui.keys;

    let _guard = terminal::install()?;
    let mut terminal = terminal::new()?;
    let mut app = app::App::new(ctx, keymap);
    event::run_loop(&mut terminal, &mut app)?;
    Ok(())
}

/// Launch the init wizard, then continue into the dashboard on success.
///
/// The wizard runs in its own event loop so we can tear down the alternate
/// screen around [`init::run_init`] — it prints progress to stdout/stderr
/// that would otherwise collide with ratatui's frame buffer. Once the wizard
/// completes we re-derive the context (the user may have moved the data
/// directory) and hand control to the normal dashboard event loop.
pub fn run_init_flow() -> Result<()> {
    let tui = Config::load(&config_path())?.tui;
    theme::set_theme(&tui.theme)?;
    icons::set_use_nerd_fonts(tui.nerd_fonts);
    let keymap = tui.keys;

    let mut guard = Some(terminal::install()?);
    let mut terminal = Some(terminal::new()?);
    let mut wizard = InitWizardView::new();

    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    loop {
        if let Some(term) = terminal.as_mut() {
            term.draw(|frame| wizard.draw(frame))?;
        }

        match wizard.outcome() {
            Outcome::Aborted => return Ok(()),
            Outcome::Completed => break,
            Outcome::Pending => {}
        }

        if let Some(args) = wizard.take_pending_init() {
            // Suspend TUI — init may print to stderr and should not share
            // the alternate screen.
            terminal.take();
            guard.take();

            let ctx = Context {
                data_dir: crate::config::data_dir(),
                state_dir: crate::config::state_dir(),
                store: PathBuf::new(),
                recipients_path: None,
                key_provider: crate::config::KeyProvider::default(),
            };
            let result = init::run(args, &ctx);

            // Resume TUI before reporting the result so the wizard can redraw.
            guard = Some(terminal::install()?);
            terminal = Some(terminal::new()?);
            wizard.on_init_result(result);
            continue;
        }

        if crossterm::event::poll(POLL_INTERVAL)? {
            if let Event::Key(key) = crossterm::event::read()? {
                if key.kind == KeyEventKind::Press {
                    wizard.on_key(key, &keymap);
                }
            }
        }
    }

    // Drop wizard terminal/guard before starting the dashboard loop so the
    // terminal is cleanly restored if dashboard setup fails.
    drop(terminal);
    drop(guard);

    let tui = Config::load(&config_path())?.tui;
    theme::set_theme(&tui.theme)?;
    icons::set_use_nerd_fonts(tui.nerd_fonts);

    let cfg = Config::load(&config_path()).unwrap_or_default();
    let ctx = Context {
        data_dir: crate::config::data_dir(),
        state_dir: crate::config::state_dir(),
        store: crate::config::resolve_store(None).unwrap_or_default(),
        recipients_path: None,
        key_provider: cfg.key_provider,
    };
    if !should_continue_to_dashboard_after_init(&ctx.store) {
        return Ok(());
    }
    run(&ctx)
}

fn should_launch_init_flow(ctx: &Context) -> bool {
    // Fire the wizard only when himitsu isn't initialized OR the user has
    // zero stores registered globally. Running `himitsu` from a directory
    // that doesn't have its own project store should land you on the
    // dashboard with a "no project store" indicator — not bounce you into
    // setup every time you cd into a new repo.
    if !crate::crypto::keystore::is_initialized(&ctx.data_dir) {
        return true;
    }
    !has_any_registered_store(&ctx.stores_dir())
}

fn has_any_registered_store(stores_dir: &std::path::Path) -> bool {
    let Ok(orgs) = std::fs::read_dir(stores_dir) else {
        return false;
    };
    for org in orgs.flatten() {
        let Ok(repos) = std::fs::read_dir(org.path()) else {
            continue;
        };
        if repos
            .flatten()
            .any(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        {
            return true;
        }
    }
    false
}

fn should_continue_to_dashboard_after_init(store: &std::path::Path) -> bool {
    !store.as_os_str().is_empty()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::cli::Context;

    fn ctx_with_state(data_dir: PathBuf, state_dir: PathBuf, store: PathBuf) -> Context {
        Context {
            data_dir,
            state_dir,
            store,
            recipients_path: None,
            key_provider: crate::config::KeyProvider::default(),
        }
    }

    #[test]
    fn should_launch_init_flow_when_no_key() {
        // No pubkey file → not initialized → wizard fires regardless of
        // store state.
        let data_dir = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_state(
            data_dir.path().to_path_buf(),
            state_dir.path().to_path_buf(),
            PathBuf::new(),
        );
        assert!(super::should_launch_init_flow(&ctx));
    }

    #[test]
    fn should_launch_init_flow_when_key_exists_but_no_stores_registered() {
        // Initialized but `stores/` is empty (or missing) → wizard fires.
        let data_dir = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        std::fs::write(data_dir.path().join("key.pub"), "age1pub").unwrap();
        let ctx = ctx_with_state(
            data_dir.path().to_path_buf(),
            state_dir.path().to_path_buf(),
            PathBuf::new(),
        );
        assert!(super::should_launch_init_flow(&ctx));
    }

    #[test]
    fn should_not_launch_init_flow_when_any_store_is_registered() {
        // Initialized + at least one registered store → dashboard, even if
        // the resolved active store is empty (e.g. running from a project
        // that hasn't been wired to a store yet — the project light just
        // goes gray, no wizard).
        let data_dir = tempfile::tempdir().unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        std::fs::write(data_dir.path().join("key.pub"), "age1pub").unwrap();
        let stores = state_dir.path().join("stores");
        std::fs::create_dir_all(stores.join("acme/secrets")).unwrap();

        let ctx = ctx_with_state(
            data_dir.path().to_path_buf(),
            state_dir.path().to_path_buf(),
            PathBuf::new(),
        );
        assert!(!super::should_launch_init_flow(&ctx));
    }

    #[test]
    fn should_not_continue_to_dashboard_after_init_without_store() {
        assert!(!super::should_continue_to_dashboard_after_init(
            &PathBuf::new()
        ));
    }
}
