//! In-process ratatui TUI for himitsu.
//!
//! Entry points:
//! - [`run`] — launched when the user runs `himitsu` with no subcommand. Opens
//!   the init wizard if no age key exists, otherwise the dashboard.
//! - [`run_init_flow`] — launched from the TTY path of `himitsu init`. Always
//!   starts on the init wizard, then advances to the dashboard on success.

mod app;
mod event;
#[cfg(test)]
mod harness;
pub mod keymap;
mod terminal;
mod views;

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{Event, KeyEventKind};

use crate::cli::{init, Context};
use crate::config::{config_path, Config};
use crate::error::Result;
use views::init_wizard::{InitWizardView, Outcome};

pub fn run(ctx: &Context) -> Result<()> {
    // First-run flow: if no age key exists, drop straight into the wizard.
    if !ctx.data_dir.join("key").exists() {
        return run_init_flow();
    }

    // Load the keymap from the user's global config. A missing file yields
    // the defaults; a malformed `tui.keys` section surfaces as a hard error
    // so the user sees the typo immediately instead of silently losing a
    // rebinding.
    let keymap = Config::load(&config_path())
        .map(|cfg| cfg.tui.keys)
        .unwrap_or_default();

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
            };
            let result = init::run_init(args, &ctx);

            // Resume TUI before reporting the result so the wizard can redraw.
            guard = Some(terminal::install()?);
            terminal = Some(terminal::new()?);
            wizard.on_init_result(result);
            continue;
        }

        if crossterm::event::poll(POLL_INTERVAL)? {
            if let Event::Key(key) = crossterm::event::read()? {
                if key.kind == KeyEventKind::Press {
                    wizard.on_key(key);
                }
            }
        }
    }

    // Drop wizard terminal/guard before starting the dashboard loop so the
    // terminal is cleanly restored if dashboard setup fails.
    drop(terminal);
    drop(guard);

    let ctx = Context {
        data_dir: crate::config::data_dir(),
        state_dir: crate::config::state_dir(),
        store: crate::config::resolve_store(None).unwrap_or_default(),
        recipients_path: None,
    };
    run(&ctx)
}
