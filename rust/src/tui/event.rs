//! Poll-based event loop.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event};

use crate::error::Result;
use crate::tui::app::{App, AppIntent};
use crate::tui::terminal::{self, Tui};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

pub fn run_loop(terminal: &mut Tui, app: &mut App) -> Result<()> {
    let mut pending_refresh: Option<Instant> = None;
    let mut last_refresh: Option<Instant> = None;

    loop {
        terminal.draw(|frame| app.draw(frame))?;

        if event::poll(POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) if key.kind == event::KeyEventKind::Press => {
                    if let Some(intent) = app.on_key(key) {
                        handle_intent(terminal, app, intent)?;
                    }
                }
                Event::Resize(width, height) => {
                    use crate::tui::layout::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH};
                    if (width, height) < (MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT) {
                        app.should_quit = true;
                    }
                }
                _ => {}
            }
        }

        if app.search_dirty {
            let now = Instant::now();
            let debounce_elapsed = last_refresh
                .map(|last| now.duration_since(last) >= SEARCH_DEBOUNCE)
                .unwrap_or(true);

            if pending_refresh.is_none() && debounce_elapsed {
                app.refresh_search();
                last_refresh = Some(now);
            } else {
                pending_refresh = Some(now);
            }
            app.search_dirty = false;
        }

        app.tick_chord_timeout();

        if let Some(last_key) = pending_refresh {
            let now = Instant::now();
            if now.duration_since(last_key) >= SEARCH_DEBOUNCE {
                app.refresh_search();
                last_refresh = Some(now);
                pending_refresh = None;
                app.search_dirty = false;
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_intent(terminal: &mut Tui, app: &mut App, intent: AppIntent) -> Result<()> {
    match intent {
        AppIntent::EditSecretValue(plaintext) => {
            let outcome = terminal::suspend_then(terminal, || run_editor(&plaintext))?;
            app.finish_secret_edit(outcome);
            Ok(())
        }
    }
}

/// Drop guard that deletes a temp file on every exit path.
struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Run `$EDITOR` (fallback `vi`) on `plaintext`, returning
/// `Ok(Some(new))` if the contents changed, `Ok(None)` if unchanged or
/// the editor exited non-zero (treated as cancel), or `Err(msg)` if we
/// could not spawn or read the temp file at all.
fn run_editor(plaintext: &str) -> std::result::Result<Option<String>, String> {
    let path = match create_temp_file() {
        Ok(p) => p,
        Err(e) => return Err(format!("temp file: {e}")),
    };
    let _guard = TempFileGuard(path.clone());

    if let Err(e) = std::fs::write(&path, plaintext) {
        return Err(format!("temp file write: {e}"));
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    // `$EDITOR` may contain flags (e.g. `"code -w"`). Split on whitespace
    // so those arrive as separate argv entries.
    let mut parts = editor.split_whitespace();
    let program = match parts.next() {
        Some(p) => p,
        None => return Err("EDITOR is empty".to_string()),
    };

    let status = Command::new(program)
        .args(parts)
        .arg(&path)
        .status()
        .map_err(|e| format!("spawn {program}: {e}"))?;

    if !status.success() {
        // Non-zero exit ⇒ treat as cancel per spec.
        return Ok(None);
    }

    let new_contents =
        std::fs::read_to_string(&path).map_err(|e| format!("temp file read: {e}"))?;

    if new_contents == plaintext {
        Ok(None)
    } else {
        Ok(Some(new_contents))
    }
}

/// Create an exclusive-creation temp file under `std::env::temp_dir()`
/// with mode `0o600` on unix. Fails if the file already exists, so a
/// racing attacker can't preseed it.
fn create_temp_file() -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};

    // Per-process monotonic counter so two calls in the same nanosecond (or
    // under parallel test runs sharing a pid) never collide on the name.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let dir = std::env::temp_dir();
    let pid = std::process::id();

    // Retry on collision: with create_new(true) an AlreadyExists is racy/
    // duplicate, not fatal — bump the counter and try a fresh name.
    for _ in 0..64 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("himitsu-edit-{pid}-{nanos}-{seq}.txt");
        let path = dir.join(name);

        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&path) {
            Ok(_file) => {
                // Close the handle; the editor will reopen the path.
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not create a unique temp file after 64 attempts",
    ))
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Drop guard must remove the file even if the caller never does.
    #[test]
    fn temp_file_guard_removes_file_on_drop() {
        let path = create_temp_file().unwrap();
        assert!(path.exists());
        {
            let _guard = TempFileGuard(path.clone());
        }
        assert!(!path.exists());
    }

    #[test]
    fn run_editor_with_noop_editor_returns_none() {
        // `true` exits 0 without touching the file ⇒ no change.
        std::env::set_var("EDITOR", "true");
        let out = run_editor("hello").unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn run_editor_with_failing_editor_returns_none_as_cancel() {
        std::env::set_var("EDITOR", "false");
        let out = run_editor("hello").unwrap();
        assert_eq!(out, None);
    }
}
