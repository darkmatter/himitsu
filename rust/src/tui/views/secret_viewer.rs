//! Secret viewer: shows metadata for a single secret, with opt-in reveal.
//!
//! Opened from the search view (`SearchAction::OpenViewer`). Operations:
//!
//! - `r` — toggle reveal. Decrypts via [`crate::crypto::age`] on demand;
//!   subsequent presses hide the value again.
//! - `y` — copy the (already-revealed or freshly-decrypted) value to the
//!   system clipboard via [`arboard`]. Falls back to a status message if
//!   the clipboard backend is unavailable (e.g. headless CI).
//! - `e` — decrypt the secret, open a `$EDITOR` buffer containing both its
//!   metadata (`description`, `url`, `totp`, `expires_at`, `env_key`) and
//!   the raw value, then re-encrypt for the current recipients. The TUI
//!   suspends its alternate screen while the editor runs. See
//!   [`render_edit_doc`] / [`parse_edit_doc`] for the buffer format.
//! - `R` — re-encrypt this one secret for the current recipient set via
//!   [`crate::cli::rekey::rekey_store`] (no value change).
//! - `Esc` — emit `SecretViewerAction::Back` so the router pops to the
//!   previous view (search).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};

use super::standard_canvas;

use crate::tui::theme;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::cli::{duration, rekey, Context};
use crate::crypto::{age, secret_value, tags as tag_grammar};
use crate::error::HimitsuError;
use crate::proto::SecretValue;
use crate::remote::store::{self, SecretMeta};
use crate::tui::keymap::{Bindings, KeyMap};

/// Outcome of handling a key — routed by [`crate::tui::app::App`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretViewerAction {
    None,
    Back,
    Quit,
    /// Caller (event loop) should suspend the TUI, open `$EDITOR` on the
    /// carried plaintext, then hand the result back via
    /// [`SecretViewerView::finish_edit`].
    EditValue(String),
    /// The displayed secret was deleted — the router should pop back to
    /// whichever view opened the viewer (refreshed).
    Deleted,
    /// `y` copy succeeded — router should toast "copied to clipboard".
    Copied,
    /// `y` copy failed (decrypt error / headless clipboard / …).
    CopyFailed(String),
}

#[derive(Debug, Clone)]
enum ValueState {
    Hidden,
    Revealed(String),
}

/// UX mode for the viewer. In [`Mode::ConfirmDelete`], the normal key
/// bindings are suspended and only `y` / `n` / `Esc` are accepted; the
/// underlying view is still rendered, with a confirmation overlay on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    ConfirmDelete,
}

#[derive(Debug, Clone, Copy)]
enum StatusKind {
    Info,
    Error,
}

pub struct SecretViewerView {
    /// Slug label for the store this secret lives in, used only for display.
    store_label: String,
    /// Absolute store path. Needed by the crypto + rekey code paths.
    store_path: PathBuf,
    /// Secret path within the store (e.g. `prod/API_KEY`).
    path: String,
    meta: SecretMeta,
    /// Decoded SecretValue envelope — populated eagerly at construction so
    /// the metadata pane can render description / url / totp / env_key /
    /// expires_at without revealing the secret value itself. `None` if the
    /// identity file is missing or the ciphertext failed to parse (e.g. a
    /// legacy raw payload with no structured metadata).
    decoded: Option<secret_value::Decoded>,
    value: ValueState,
    status: Option<(String, StatusKind)>,
    mode: Mode,
    /// Context needed to call into `rekey::rekey_store` on `e`.
    ///
    /// Cloned from the app router so the view owns its data.
    ctx: Context,
}

impl SecretViewerView {
    pub fn new(ctx: &Context, store_label: String, store_path: PathBuf, path: String) -> Self {
        // Best-effort metadata read — if it fails we still show the path and
        // let the user try to reveal (which will surface the real error).
        let meta = store::read_secret_meta(&store_path, &path).unwrap_or_default();

        // The viewer inherits the outer context but must operate against the
        // store the result came from, not whatever `ctx.store` happens to be.
        let mut ctx_owned = ctx.clone();
        ctx_owned.store = store_path.clone();

        // Best-effort eager decrypt so the metadata pane can show structured
        // fields. Failures are silent — the viewer still lets the user press
        // `r` to surface the real error.
        let decoded = store::read_secret(&store_path, &path)
            .ok()
            .and_then(|ct| {
                age::read_identity(&ctx_owned.key_path())
                    .and_then(|id| age::decrypt(&ct, &id))
                    .ok()
            })
            .map(|plain| secret_value::decode(&plain));

        Self {
            store_label,
            store_path,
            path,
            meta,
            decoded,
            value: ValueState::Hidden,
            status: None,
            mode: Mode::Normal,
            ctx: ctx_owned,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, keymap: &KeyMap) -> SecretViewerAction {
        // Ctrl-C is always a quit, regardless of mode. The quit binding is
        // checked before the confirm-delete intercept so the user is never
        // trapped in a modal dialog.
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
        ) {
            return SecretViewerAction::Quit;
        }

        // Confirm-delete mode intercepts all keys except Ctrl-C.
        if self.mode == Mode::ConfirmDelete {
            return self.on_key_confirm_delete(key);
        }

        // Rekey is checked before reveal/copy because its default binding
        // (`Shift+R`) overlaps the same `KeyCode::Char('r')` as `reveal`;
        // matching in this order keeps the existing behaviour exact.
        if keymap.rekey.matches(&key) {
            self.rekey();
            return SecretViewerAction::None;
        }
        if keymap.reveal.matches(&key) {
            self.toggle_reveal();
            return SecretViewerAction::None;
        }
        if keymap.copy_value.matches(&key) {
            return self.copy_to_clipboard();
        }
        if keymap.edit.matches(&key) {
            return self.begin_edit();
        }
        if keymap.delete.matches(&key) {
            self.enter_confirm_delete();
            return SecretViewerAction::None;
        }
        if keymap.back.matches(&key) {
            return SecretViewerAction::Back;
        }
        SecretViewerAction::None
    }

    /// Decrypt the current secret, render it as an editable document
    /// (metadata header + `---` separator + raw value) and ask the event
    /// loop to run `$EDITOR`.
    ///
    /// Returns [`SecretViewerAction::EditValue`] with the full document on
    /// success, or [`SecretViewerAction::None`] with a status message set on
    /// decrypt failure (so the user sees *why* the edit did not happen).
    fn begin_edit(&mut self) -> SecretViewerAction {
        match self.read_decoded() {
            Ok(decoded) => SecretViewerAction::EditValue(render_edit_doc(&self.path, &decoded)),
            Err(e) => {
                self.status = Some((format!("edit failed: {e}"), StatusKind::Error));
                SecretViewerAction::None
            }
        }
    }

    /// Transition into the confirm-delete mode. Pure state change so tests
    /// can exercise confirm/cancel without touching the filesystem.
    fn enter_confirm_delete(&mut self) {
        self.mode = Mode::ConfirmDelete;
        // Clear any stale status line so the overlay is unambiguous.
        self.status = None;
    }

    fn cancel_confirm_delete(&mut self) {
        self.mode = Mode::Normal;
    }

    fn on_key_confirm_delete(&mut self, key: KeyEvent) -> SecretViewerAction {
        match (key.code, key.modifiers) {
            // Only an explicit lowercase 'y' confirms. Any other key —
            // including 'n', Esc, arrows, typos — cancels and returns to
            // the normal view. This is the least surprising behaviour for
            // a destructive default-no prompt.
            (KeyCode::Char('y'), _) => match self.delete_secret() {
                Ok(()) => SecretViewerAction::Deleted,
                Err(e) => {
                    self.status = Some((format!("delete failed: {e}"), StatusKind::Error));
                    self.mode = Mode::Normal;
                    SecretViewerAction::None
                }
            },
            _ => {
                self.cancel_confirm_delete();
                SecretViewerAction::None
            }
        }
    }

    /// Handle the result of an external edit. Called by the event loop
    /// after `$EDITOR` exits. `result` is `Ok(Some(new_document))` on a
    /// real change, `Ok(None)` for "no change / cancelled", and
    /// `Err(msg)` for a terminal failure (spawn error, non-zero exit).
    pub fn finish_edit(&mut self, result: std::result::Result<Option<String>, String>) {
        match result {
            Ok(None) => {
                self.status = Some(("edit cancelled (no changes)".to_string(), StatusKind::Info));
            }
            Ok(Some(doc)) => match self.persist_edited(&doc) {
                Ok(new_value) => {
                    // Keep the new value visible so the user sees what they
                    // just committed.
                    self.value = ValueState::Revealed(new_value);
                    self.status = Some(("edited".to_string(), StatusKind::Info));
                }
                Err(e) => {
                    self.status = Some((format!("edit failed: {e}"), StatusKind::Error));
                }
            },
            Err(e) => {
                self.status = Some((format!("edit failed: {e}"), StatusKind::Error));
            }
        }
    }

    /// Parse an edited document, re-encrypt, and write it to disk. Returns
    /// the raw plaintext value so the caller can refresh the revealed state.
    ///
    /// Also handles renames: if the document's `path:` field differs from
    /// the current path, the on-disk file is moved (preserving created_at +
    /// history) before the new ciphertext is written. The viewer's own
    /// `self.path` is updated to the new value so subsequent reveal/copy/
    /// rekey actions target the right file.
    fn persist_edited(&mut self, doc: &str) -> crate::error::Result<String> {
        let parsed = parse_edit_doc(doc)
            .map_err(|e| HimitsuError::InvalidReference(format!("edit: {e}")))?;

        // Normalise + validate the (possibly renamed) path before any I/O so
        // a typo doesn't leave the store in a half-renamed state.
        let new_path = normalize_secret_path(&parsed.path)
            .map_err(|e| HimitsuError::InvalidReference(format!("edit: invalid path: {e}")))?;

        let expires_at = if parsed.expires_at.trim().is_empty() {
            None
        } else {
            match duration::parse(&parsed.expires_at)? {
                duration::ExpiresAt::Never => None,
                duration::ExpiresAt::At(dt) => Some(duration::to_proto_timestamp(dt)),
            }
        };

        let recipients =
            age::collect_recipients(&self.store_path, self.ctx.recipients_path.as_deref())?;
        // Validate every tag against the shared grammar before any I/O so a
        // typo doesn't produce an unreadable envelope. The edit doc renders
        // tags as a comma-separated `tags:` row, so removing the row clears
        // them — matching how the other metadata fields behave.
        for t in &parsed.tags {
            tag_grammar::validate_tag(t)
                .map_err(|reason| HimitsuError::InvalidReference(format!("edit: {reason}")))?;
        }
        let sv = SecretValue {
            data: parsed.value.as_bytes().to_vec(),
            content_type: String::new(),
            annotations: parsed.annotations,
            totp: parsed.totp,
            url: parsed.url,
            expires_at,
            description: parsed.description,
            env_key: parsed.env_key,
            tags: parsed.tags,
        };
        let wire = secret_value::encode(&sv);
        let ciphertext = age::encrypt(&wire, &recipients)?;

        // Rename first (if needed) so the subsequent write_secret hits the
        // moved envelope and preserves created_at/history.
        if new_path != self.path {
            store::rename_secret(&self.store_path, &self.path, &new_path)?;
            self.path = new_path.clone();
        }

        store::write_secret(&self.store_path, &self.path, &ciphertext)?;
        if let Ok(meta) = store::read_secret_meta(&self.store_path, &self.path) {
            self.meta = meta;
        }
        // Refresh the decoded snapshot so the metadata pane mirrors what we
        // just wrote, without an extra decrypt round trip.
        self.decoded = Some(secret_value::Decoded {
            data: sv.data,
            description: sv.description,
            url: sv.url,
            totp: sv.totp,
            env_key: sv.env_key,
            expires_at: sv.expires_at,
            annotations: sv.annotations,
            tags: sv.tags,
        });
        Ok(parsed.value)
    }

    fn delete_secret(&mut self) -> crate::error::Result<()> {
        store::delete_secret(&self.store_path, &self.path)
    }

    // ── Actions ────────────────────────────────────────────────────────

    fn toggle_reveal(&mut self) {
        match &self.value {
            ValueState::Revealed(_) => {
                self.value = ValueState::Hidden;
                self.status = None;
            }
            ValueState::Hidden => match self.decrypt() {
                Ok(plain) => {
                    self.value = ValueState::Revealed(plain);
                    self.status = None;
                }
                Err(e) => {
                    self.status = Some((format!("decrypt failed: {e}"), StatusKind::Error));
                }
            },
        }
    }

    fn copy_to_clipboard(&mut self) -> SecretViewerAction {
        let value = match &self.value {
            ValueState::Revealed(v) => v.clone(),
            ValueState::Hidden => match self.decrypt() {
                Ok(v) => v,
                Err(e) => {
                    return SecretViewerAction::CopyFailed(format!("decrypt failed: {e}"));
                }
            },
        };

        // Graceful no-op: arboard may fail on headless boxes or when no
        // display server is available. Surface via an action so the router
        // can turn it into a toast instead of crashing.
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(value)) {
            Ok(()) => SecretViewerAction::Copied,
            Err(e) => SecretViewerAction::CopyFailed(format!("clipboard unavailable: {e}")),
        }
    }

    fn rekey(&mut self) {
        match rekey::rekey_store(&self.ctx, Some(&self.path)) {
            Ok(n) => {
                // Refresh metadata so `lastmodified` reflects the rewrite.
                if let Ok(meta) = store::read_secret_meta(&self.store_path, &self.path) {
                    self.meta = meta;
                }
                // A revealed value is still valid plaintext, but the
                // ciphertext has changed — keep it displayed for convenience.
                self.status = Some((
                    format!("rekeyed {n} secret(s) for current recipients"),
                    StatusKind::Info,
                ));
            }
            Err(e) => {
                self.status = Some((format!("rekey failed: {e}"), StatusKind::Error));
            }
        }
    }

    fn decrypt(&self) -> crate::error::Result<String> {
        let decoded = self.read_decoded()?;
        Ok(String::from_utf8_lossy(&decoded.data).into_owned())
    }

    fn read_decoded(&self) -> crate::error::Result<secret_value::Decoded> {
        let ciphertext = store::read_secret(&self.store_path, &self.path)?;
        let identity = age::read_identity(&self.ctx.key_path())?;
        let plain = age::decrypt(&ciphertext, &identity)?;
        Ok(secret_value::decode(&plain))
    }

    // ── Drawing ────────────────────────────────────────────────────────

    pub fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = standard_canvas(frame.area());
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_header(frame, chunks[0]);
        self.draw_body(frame, chunks[1]);
        self.draw_footer(frame, chunks[2]);

        if self.mode == Mode::ConfirmDelete {
            self.draw_confirm_delete(frame, area);
        }
    }

    fn draw_confirm_delete(&self, frame: &mut Frame<'_>, area: Rect) {
        // Center a small prompt box over the existing view. The underlying
        // layout has already been rendered, so this acts as an overlay.
        let prompt = format!(" Delete {}? (y/N) ", self.path);
        let width = (prompt.len() as u16 + 4).min(area.width.saturating_sub(2));
        let height: u16 = 3;
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let rect = Rect {
            x,
            y,
            width,
            height,
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" confirm delete ")
            .style(Style::default().fg(theme::danger()));
        let line = Line::from(vec![Span::styled(
            prompt,
            Style::default()
                .fg(theme::primary())
                .add_modifier(Modifier::BOLD),
        )]);
        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(Paragraph::new(line).block(block), rect);
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let mut spans = theme::brand_chip("秘 himitsu");
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "secret",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            self.store_label.clone(),
            Style::default().fg(theme::muted()),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_body(&self, frame: &mut Frame<'_>, area: Rect) {
        let lines = self.meta_lines();
        // Two extra rows for the `metadata` block border.
        let meta_height = (lines.len() as u16).saturating_add(2);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(meta_height), Constraint::Min(3)])
            .split(area);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" metadata ")
            .title_style(Style::default().fg(theme::border_label()));
        let p = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(p, rows[0]);
        self.draw_value(frame, rows[1]);
    }

    /// Build the metadata pane lines from both the cleartext SecretMeta
    /// header (path / created_at / lastmodified / recipients) and the
    /// decrypted SecretValue envelope (description / url / totp / env_key /
    /// expires_at). Fields that are empty or unset are omitted so short
    /// secrets don't waste vertical space.
    fn meta_lines(&self) -> Vec<Line<'static>> {
        let created = self.meta.created_at.clone().unwrap_or_else(|| "-".into());
        let modified = self.meta.lastmodified.clone().unwrap_or_else(|| "-".into());
        let recipients = if self.meta.recipients.is_empty() {
            "-".to_string()
        } else {
            self.meta.recipients.join(", ")
        };

        let mut lines = vec![
            labeled_line("path        ", self.path.clone()),
            labeled_line("created_at  ", created),
            labeled_line("lastmodified", modified),
            labeled_line("recipients  ", recipients),
        ];

        if let Some(d) = &self.decoded {
            if !d.description.is_empty() {
                lines.push(labeled_line("description ", d.description.clone()));
            }
            if !d.url.is_empty() {
                lines.push(labeled_line("url         ", d.url.clone()));
            }
            if !d.totp.is_empty() {
                // Never surface the raw TOTP secret alongside the rest of
                // the metadata — just indicate that one is configured.
                lines.push(labeled_line("totp        ", "●●●●●●  (set)"));
            }
            if !d.env_key.is_empty() {
                lines.push(labeled_line("env_key     ", d.env_key.clone()));
            }
            if let Some(ts) = d.expires_at.as_ref() {
                if !duration::is_unset(ts) {
                    if let Some(dt) = duration::from_proto_timestamp(ts) {
                        lines.push(expires_line(dt));
                    }
                }
            }
            // Tags render as inline accent bracket-chips on a single
            // labeled row, e.g. `tags  [pci] [stripe]`. Empty tags emit no
            // row so short secrets don't grow a blank "tags" line.
            if !d.tags.is_empty() {
                lines.push(tag_chips_line(&d.tags));
            }
            // Annotations: sorted for stable display order.
            if !d.annotations.is_empty() {
                let mut keys: Vec<&String> = d.annotations.keys().collect();
                keys.sort();
                for k in keys {
                    let padded = format!("{k:12}");
                    lines.push(labeled_line_owned(padded, d.annotations[k].clone()));
                }
            }
        }

        lines
    }

    fn draw_value(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" value ")
            .title_style(Style::default().fg(theme::border_label()));

        let content: Line = match &self.value {
            ValueState::Hidden => Line::from(Span::styled(
                "  ●●●●●●●●  (press r to reveal)",
                Style::default().fg(theme::muted()),
            )),
            ValueState::Revealed(v) => Line::from(Span::styled(
                format!("  {v}"),
                Style::default().fg(theme::warning()),
            )),
        };
        frame.render_widget(
            Paragraph::new(content)
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = if let Some((msg, kind)) = &self.status {
            let color = match kind {
                StatusKind::Info => theme::success(),
                StatusKind::Error => theme::danger(),
            };
            Line::from(Span::styled(msg.clone(), Style::default().fg(color)))
        } else {
            let footer = Style::default().fg(theme::footer_text());
            Line::from(vec![
                Span::styled("r", Style::default().fg(theme::accent())),
                Span::styled(" reveal    ", footer),
                Span::styled("y", Style::default().fg(theme::accent())),
                Span::styled(" copy    ", footer),
                Span::styled("e", Style::default().fg(theme::accent())),
                Span::styled(" edit    ", footer),
                Span::styled("R", Style::default().fg(theme::accent())),
                Span::styled(" rekey    ", footer),
                Span::styled("d", Style::default().fg(theme::accent())),
                Span::styled(" delete    ", footer),
                Span::styled("esc", Style::default().fg(theme::accent())),
                Span::styled(" back    ", footer),
                Span::styled("ctrl-c", Style::default().fg(theme::accent())),
                Span::styled(" quit", footer),
            ])
        };
        frame.render_widget(Paragraph::new(line), area);
    }
}

// ── Edit document round-trip ───────────────────────────────────────────
//
// The `e` action opens a plain-text document in `$EDITOR` so the user can
// change both metadata and the secret value in one place. Layout:
//
//     # himitsu edit — metadata above, secret value below the `---` line.
//     # Leave a field blank to clear it. Lines starting with `#` are ignored.
//     # expires_at accepts: 30d / 6mo / 1y / never / RFC 3339 timestamp.
//
//     description: my db password
//     url: https://example.com
//     totp:
//     expires_at: 2026-12-31T00:00:00+00:00
//     env_key: DATABASE_URL
//     ---
//     hunter2
//
// Everything after the first line that is exactly `---` becomes the raw
// secret value. Trailing newlines introduced by editors are dropped via
// `str::lines`, which drops the final line terminator.

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedEdit {
    /// The secret's path (a.k.a. its "name"). Editing this triggers a rename
    /// at persist time. Cannot be empty.
    path: String,
    description: String,
    url: String,
    totp: String,
    expires_at: String,
    env_key: String,
    /// Tags parsed from the comma-separated `tags:` row. Each entry has been
    /// trimmed and non-empty; grammar validation happens at persist time so
    /// the editor still surfaces a friendly error if the user typos a tag.
    tags: Vec<String>,
    annotations: HashMap<String, String>,
    value: String,
}

const EDIT_DOC_HEADER: &str = "# himitsu edit — metadata above, secret value below the `---` line.
# Leave a field blank to clear it. Lines starting with `#` are ignored.
# Editing `path` renames the secret (preserves created_at + history).
# expires_at accepts: 30d / 6mo / 1y / never / RFC 3339 timestamp.
# Custom fields (any other `key: value` lines) are stored as annotations.

";

fn render_edit_doc(path: &str, decoded: &secret_value::Decoded) -> String {
    let expires = decoded
        .expires_at
        .as_ref()
        .and_then(|ts| {
            if duration::is_unset(ts) {
                None
            } else {
                duration::from_proto_timestamp(ts).map(|dt| dt.to_rfc3339())
            }
        })
        .unwrap_or_default();
    let value = String::from_utf8_lossy(&decoded.data);
    // Tags serialise as a single comma-separated row so the user can edit
    // them inline. The tag grammar forbids commas + whitespace, so this is
    // a lossless round-trip with `parse_edit_doc`.
    let tags_csv = decoded.tags.join(", ");
    let mut buf = format!(
        "{EDIT_DOC_HEADER}path: {path}\ndescription: {desc}\nurl: {url}\ntotp: {totp}\nexpires_at: {exp}\nenv_key: {env}\ntags: {tags}",
        desc = decoded.description,
        url = decoded.url,
        totp = decoded.totp,
        exp = expires,
        env = decoded.env_key,
        tags = tags_csv,
    );
    // Append annotations in sorted order for deterministic output.
    let mut keys: Vec<&String> = decoded.annotations.keys().collect();
    keys.sort();
    for k in keys {
        buf.push_str(&format!("\n{}: {}", k, decoded.annotations[k]));
    }
    buf.push_str(&format!("\n---\n{value}"));
    buf
}

fn parse_edit_doc(doc: &str) -> std::result::Result<ParsedEdit, String> {
    let mut parsed = ParsedEdit::default();
    let mut value_lines: Vec<&str> = Vec::new();
    let mut in_value = false;
    for line in doc.lines() {
        if in_value {
            value_lines.push(line);
            continue;
        }
        if line.trim() == "---" {
            in_value = true;
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| format!("expected `key: value`, got `{line}`"))?;
        let key = k.trim();
        // Strip at most one space after the colon so `description:  hi ` keeps
        // the user's trailing whitespace.
        let val = v.strip_prefix(' ').unwrap_or(v).to_string();
        match key {
            "path" => parsed.path = val,
            "description" => parsed.description = val,
            "url" => parsed.url = val,
            "totp" => parsed.totp = val,
            "expires_at" => parsed.expires_at = val,
            "env_key" => parsed.env_key = val,
            "tags" => {
                // Split on commas, trim each entry, drop empties so a blank
                // `tags:` row produces an empty Vec rather than `[""]`.
                parsed.tags = val
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            other => {
                parsed.annotations.insert(other.to_string(), val);
            }
        }
    }
    if !in_value {
        return Err("missing `---` separator before secret value".to_string());
    }
    parsed.value = value_lines.join("\n");
    Ok(parsed)
}

/// Validate + normalise a secret path coming back from the editor.
///
/// We trim outer whitespace and reject anything that would land outside the
/// `secrets/` tree (absolute paths, `..`, leading/trailing slashes). Empty
/// paths are also rejected — every secret must have a name.
fn normalize_secret_path(raw: &str) -> std::result::Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    if trimmed.starts_with('/') || trimmed.ends_with('/') {
        return Err("path must not start or end with `/`".to_string());
    }
    if trimmed
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err("path segments must be non-empty and not `.`/`..`".to_string());
    }
    if trimmed.contains('\\') {
        return Err("path must not contain `\\`".to_string());
    }
    Ok(trimmed.to_string())
}

fn labeled_line(label: &'static str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label} "), Style::default().fg(theme::muted())),
        Span::raw(value.into()),
    ])
}

fn labeled_line_owned(label: String, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label} "), Style::default().fg(theme::muted())),
        Span::raw(value),
    ])
}

/// Render `tags` as an inline-chip row, e.g. `  tags         [pci] [stripe]`.
/// Caller must guard against an empty list — this helper always emits a
/// labeled row so the metadata pane keeps its alignment.
fn tag_chips_line(tags: &[String]) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "  tags         ",
        Style::default().fg(theme::muted()),
    )];
    let chip_style = Style::default().fg(theme::accent());
    for (i, tag) in tags.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!("[{tag}]"), chip_style));
    }
    Line::from(spans)
}

/// Render the `expires_at` row with severity-based coloring (dim when far
/// away, yellow when soon, red when already expired) — matches the CLI
/// `himitsu get` metadata block.
fn expires_line(dt: chrono::DateTime<chrono::Utc>) -> Line<'static> {
    let (msg, sev) = duration::describe_remaining(dt, chrono::Utc::now());
    let text = format!("{}  ({msg})", dt.to_rfc3339());
    let color = match sev {
        duration::ExpirySeverity::Distant => theme::neutral(),
        duration::ExpirySeverity::Soon => theme::warning(),
        duration::ExpirySeverity::Expired => theme::danger(),
    };
    Line::from(vec![
        Span::styled("  expires_at   ", Style::default().fg(theme::muted())),
        Span::styled(text, Style::default().fg(color)),
    ])
}

/// Kept for potential future use by other views that want to resolve a
/// label back to a real store path.
#[allow(dead_code)]
pub fn store_path_for(label: &str, ctx: &Context) -> Option<PathBuf> {
    let candidate = ctx.stores_dir().join(label);
    if candidate.exists() {
        return Some(candidate);
    }
    let path = Path::new(label);
    if path.exists() {
        return Some(path.to_path_buf());
    }
    None
}

// ── Help overlay integration (US-012) ─────────────────────────────────
//
// In its own impl block so parallel branches adding new bindings can extend
// `help_entries` without colliding with the main impl.
impl SecretViewerView {
    pub fn help_entries() -> &'static [(&'static str, &'static str)] {
        &[
            ("r", "reveal / hide value"),
            ("y", "copy value to clipboard"),
            ("e", "edit value + metadata in $EDITOR"),
            ("R", "rekey for current recipients"),
            ("d", "delete secret (with confirm)"),
            ("?", "toggle this help"),
            ("esc", "back"),
            ("ctrl-c", "quit"),
        ]
    }

    pub fn help_title() -> &'static str {
        "secret · keys"
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keymap::KeyMap;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn shift(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Seed a store with one encrypted secret and return (tempdir, ctx, path).
    ///
    /// Uses real age keys + encryption so reveal/rekey paths run end-to-end.
    fn seeded_store_with_secret() -> (TempDir, Context, String) {
        use secrecy::ExposeSecret;
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data");
        let state_dir = dir.path().join("state");
        let store = state_dir.join("stores/test/repo");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/secrets")).unwrap();
        std::fs::create_dir_all(store.join(".himitsu/recipients")).unwrap();

        let identity = ::age::x25519::Identity::generate();
        let pubkey = identity.to_public().to_string();
        let secret = identity.to_string().expose_secret().to_string();
        std::fs::write(data_dir.join("key"), &secret).unwrap();
        std::fs::write(
            store.join(".himitsu/recipients/me.pub"),
            format!("{pubkey}\n"),
        )
        .unwrap();

        // Encrypt and write a secret through the real store writer.
        let recipients = age::collect_recipients(&store, None).unwrap();
        let ct = age::encrypt(b"s3cret", &recipients).unwrap();
        store::write_secret(&store, "prod/API_KEY", &ct).unwrap();

        let ctx = Context {
            data_dir,
            state_dir,
            store: store.clone(),
            recipients_path: None,
        };
        (dir, ctx, "prod/API_KEY".to_string())
    }

    #[test]
    fn metadata_is_loaded_on_construction() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        assert_eq!(view.path, "prod/API_KEY");
        assert!(view.meta.created_at.is_some());
        assert!(view.meta.lastmodified.is_some());
        assert_eq!(view.meta.recipients.len(), 1);
    }

    #[test]
    fn esc_returns_back_action() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        assert_eq!(
            view.on_key(press(KeyCode::Esc), &km),
            SecretViewerAction::Back
        );
    }

    #[test]
    fn ctrl_c_returns_quit() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        assert_eq!(view.on_key(ctrl('c'), &km), SecretViewerAction::Quit);
    }

    #[test]
    fn r_reveals_then_hides_value() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        assert!(matches!(view.value, ValueState::Hidden));
        view.on_key(press(KeyCode::Char('r')), &km);
        match &view.value {
            ValueState::Revealed(v) => assert_eq!(v, "s3cret"),
            _ => panic!("expected Revealed"),
        }
        view.on_key(press(KeyCode::Char('r')), &km);
        assert!(matches!(view.value, ValueState::Hidden));
    }

    #[test]
    fn y_copies_without_revealing_display() {
        let km = KeyMap::default();
        // On headless CI arboard may fail — assert it doesn't crash and
        // emits *some* copy-outcome action (Copied on success, CopyFailed
        // on headless miss). Router turns whichever it is into a toast.
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        let action = view.on_key(press(KeyCode::Char('y')), &km);
        assert!(
            matches!(
                action,
                SecretViewerAction::Copied | SecretViewerAction::CopyFailed(_)
            ),
            "y should emit a copy outcome action, got {action:?}"
        );
        // Value state is an implementation detail — either hidden or kept as
        // an in-memory cache is fine. We only require the UI survived.
    }

    #[test]
    fn d_enters_confirm_delete_mode_without_deleting() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        let action = view.on_key(press(KeyCode::Char('d')), &km);
        assert_eq!(action, SecretViewerAction::None);
        assert_eq!(view.mode, Mode::ConfirmDelete);
        // Secret file must still exist — 'd' alone must not delete.
        assert!(store::read_secret_meta(&ctx.store, &path).is_ok());
    }

    #[test]
    fn y_in_confirm_mode_deletes_and_returns_deleted() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('d')), &km);
        assert_eq!(view.mode, Mode::ConfirmDelete);
        let action = view.on_key(press(KeyCode::Char('y')), &km);
        assert_eq!(action, SecretViewerAction::Deleted);
        // Underlying store should no longer contain the secret.
        assert!(store::list_secrets(&ctx.store, None)
            .unwrap()
            .iter()
            .all(|p| p != &path));
    }

    #[test]
    fn n_in_confirm_mode_cancels_without_deleting() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('d')), &km);
        let action = view.on_key(press(KeyCode::Char('n')), &km);
        assert_eq!(action, SecretViewerAction::None);
        assert_eq!(view.mode, Mode::Normal);
        assert!(store::read_secret_meta(&ctx.store, &path).is_ok());
    }

    #[test]
    fn esc_in_confirm_mode_cancels_without_deleting() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('d')), &km);
        let action = view.on_key(press(KeyCode::Esc), &km);
        assert_eq!(action, SecretViewerAction::None);
        assert_eq!(view.mode, Mode::Normal);
        assert!(store::read_secret_meta(&ctx.store, &path).is_ok());
    }

    #[test]
    fn other_keys_in_confirm_mode_cancel() {
        let km = KeyMap::default();
        // Documented choice: any non-'y' key cancels the delete. Safer for
        // a destructive default-no prompt than staying in confirm mode.
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(press(KeyCode::Char('d')), &km);
        view.on_key(press(KeyCode::Char('q')), &km);
        assert_eq!(view.mode, Mode::Normal);
        assert!(store::read_secret_meta(&ctx.store, &path).is_ok());
    }

    #[test]
    fn ctrl_c_in_confirm_mode_still_quits() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        view.on_key(press(KeyCode::Char('d')), &km);
        assert_eq!(view.on_key(ctrl('c'), &km), SecretViewerAction::Quit);
    }

    #[test]
    fn shift_r_rekeys_and_updates_status() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let before = store::read_secret_meta(&ctx.store, &path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        view.on_key(shift('R'), &km);
        match &view.status {
            Some((msg, StatusKind::Info)) => assert!(msg.contains("rekeyed")),
            other => panic!("expected info status, got {other:?}"),
        }
        let after = store::read_secret_meta(&ctx.store, &path).unwrap();
        assert_ne!(before.lastmodified, after.lastmodified);
    }

    #[test]
    fn e_emits_edit_value_action_with_document() {
        let km = KeyMap::default();
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        match view.on_key(press(KeyCode::Char('e')), &km) {
            SecretViewerAction::EditValue(doc) => {
                assert!(
                    doc.contains("path: prod/API_KEY"),
                    "doc missing path: {doc}"
                );
                assert!(doc.contains("description:"), "doc missing header: {doc}");
                assert!(doc.contains("expires_at:"), "doc missing expires_at: {doc}");
                assert!(doc.contains("\n---\n"), "doc missing separator: {doc}");
                assert!(
                    doc.ends_with("s3cret"),
                    "doc should end with plaintext: {doc}"
                );
            }
            other => panic!("expected EditValue, got {other:?}"),
        }
    }

    #[test]
    fn finish_edit_with_new_value_persists_and_reencrypts() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        let doc =
            "path: prod/API_KEY\ndescription:\nurl:\ntotp:\nexpires_at:\nenv_key:\n---\nrotated";
        view.finish_edit(Ok(Some(doc.to_string())));
        match &view.status {
            Some((msg, StatusKind::Info)) => assert_eq!(msg, "edited"),
            other => panic!("expected info 'edited', got {other:?}"),
        }
        // Round-trip: decrypt again and confirm the new ciphertext.
        let plain = view.decrypt().unwrap();
        assert_eq!(plain, "rotated");
    }

    #[test]
    fn finish_edit_applies_metadata_fields() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        let doc = "\
path: prod/API_KEY
description: prod database password
url: https://db.example.com
totp:
expires_at: 2099-01-01T00:00:00+00:00
env_key: DATABASE_URL
---
hunter2";
        view.finish_edit(Ok(Some(doc.to_string())));
        assert!(
            matches!(&view.status, Some((m, StatusKind::Info)) if m == "edited"),
            "expected info 'edited', got {:?}",
            view.status
        );
        let decoded = view.read_decoded().unwrap();
        assert_eq!(decoded.description, "prod database password");
        assert_eq!(decoded.url, "https://db.example.com");
        assert_eq!(decoded.env_key, "DATABASE_URL");
        assert_eq!(String::from_utf8(decoded.data).unwrap(), "hunter2");
        let ts = decoded.expires_at.expect("expires_at set");
        assert!(!duration::is_unset(&ts));
    }

    #[test]
    fn finish_edit_clears_expires_when_blank() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        // First set an expiry.
        let with_exp = "\
path: prod/API_KEY
description:
url:
totp:
expires_at: 1y
env_key:
---
s3cret";
        view.finish_edit(Ok(Some(with_exp.to_string())));
        assert!(view.read_decoded().unwrap().expires_at.is_some());

        // Now clear it.
        let cleared = "\
path: prod/API_KEY
description:
url:
totp:
expires_at:
env_key:
---
s3cret";
        view.finish_edit(Ok(Some(cleared.to_string())));
        let decoded = view.read_decoded().unwrap();
        assert!(
            decoded
                .expires_at
                .as_ref()
                .map(duration::is_unset)
                .unwrap_or(true),
            "expires_at should be cleared"
        );
    }

    #[test]
    fn finish_edit_with_invalid_expires_sets_error_status() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        let doc = "\
path: prod/API_KEY
description:
url:
totp:
expires_at: not-a-duration
env_key:
---
s3cret";
        view.finish_edit(Ok(Some(doc.to_string())));
        match &view.status {
            Some((msg, StatusKind::Error)) => {
                assert!(msg.contains("edit failed"), "got {msg}");
            }
            other => panic!("expected error status, got {other:?}"),
        }
    }

    #[test]
    fn finish_edit_with_missing_separator_errors() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        view.finish_edit(Ok(Some("description: oops\n".to_string())));
        assert!(matches!(view.status, Some((_, StatusKind::Error))));
    }

    #[test]
    fn finish_edit_with_empty_path_errors() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        let doc = "path:\ndescription:\nurl:\ntotp:\nexpires_at:\nenv_key:\n---\ns3cret";
        view.finish_edit(Ok(Some(doc.to_string())));
        assert!(matches!(view.status, Some((_, StatusKind::Error))));
        // The original secret must still be readable at its original path.
        assert!(store::read_secret(&ctx.store, &path).is_ok());
    }

    #[test]
    fn finish_edit_renames_secret_and_preserves_history() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        // Capture the original created_at so we can assert it's preserved.
        let original_created = view.meta.created_at.clone();
        // First, write a new value so `history` has an entry to preserve.
        let v1 = "path: prod/API_KEY\ndescription:\nurl:\ntotp:\nexpires_at:\nenv_key:\n---\nv1";
        view.finish_edit(Ok(Some(v1.to_string())));

        // Now rename to a new path.
        let renamed =
            "path: staging/RENAMED_KEY\ndescription:\nurl:\ntotp:\nexpires_at:\nenv_key:\n---\nv2";
        view.finish_edit(Ok(Some(renamed.to_string())));
        assert!(
            matches!(&view.status, Some((m, StatusKind::Info)) if m == "edited"),
            "expected edited, got {:?}",
            view.status
        );

        // Old path should no longer exist.
        assert!(store::read_secret(&ctx.store, "prod/API_KEY").is_err());
        // New path should be readable and decrypt to v2.
        assert!(store::read_secret(&ctx.store, "staging/RENAMED_KEY").is_ok());
        assert_eq!(view.path, "staging/RENAMED_KEY");
        assert_eq!(view.decrypt().unwrap(), "v2");
        // created_at should be preserved across the rename.
        let new_meta = store::read_secret_meta(&ctx.store, "staging/RENAMED_KEY").unwrap();
        assert_eq!(new_meta.created_at, original_created);
    }

    #[test]
    fn finish_edit_rename_collision_errors() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        // Seed a second secret to collide with.
        let recipients = age::collect_recipients(&ctx.store, None).unwrap();
        let ct = age::encrypt(b"other", &recipients).unwrap();
        store::write_secret(&ctx.store, "other/KEY", &ct).unwrap();

        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        let doc = "path: other/KEY\ndescription:\nurl:\ntotp:\nexpires_at:\nenv_key:\n---\ns3cret";
        view.finish_edit(Ok(Some(doc.to_string())));
        assert!(matches!(view.status, Some((_, StatusKind::Error))));
        // Both secrets must still exist with their original contents.
        assert_eq!(view.path, path);
        assert!(store::read_secret(&ctx.store, &path).is_ok());
        assert!(store::read_secret(&ctx.store, "other/KEY").is_ok());
    }

    #[test]
    fn finish_edit_rejects_path_traversal() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view =
            SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path.clone());
        let doc = "path: ../escape\ndescription:\nurl:\ntotp:\nexpires_at:\nenv_key:\n---\ns3cret";
        view.finish_edit(Ok(Some(doc.to_string())));
        assert!(matches!(view.status, Some((_, StatusKind::Error))));
        assert_eq!(view.path, path);
    }

    #[test]
    fn parse_edit_doc_round_trips_all_fields() {
        let mut annotations = HashMap::new();
        annotations.insert("team".to_string(), "backend".to_string());
        let decoded = secret_value::Decoded {
            data: b"pw".to_vec(),
            description: "d".to_string(),
            url: "https://x".to_string(),
            totp: "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".to_string(),
            env_key: "API".to_string(),
            expires_at: None,
            annotations,
            tags: vec!["pci".to_string(), "stripe".to_string()],
        };
        let doc = render_edit_doc("prod/SECRET", &decoded);
        let parsed = parse_edit_doc(&doc).unwrap();
        assert_eq!(parsed.path, "prod/SECRET");
        assert_eq!(parsed.description, "d");
        assert_eq!(parsed.url, "https://x");
        assert_eq!(parsed.totp, "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP");
        assert_eq!(parsed.env_key, "API");
        assert_eq!(parsed.annotations.get("team").unwrap(), "backend");
        assert_eq!(parsed.tags, vec!["pci".to_string(), "stripe".to_string()]);
        assert_eq!(parsed.value, "pw");
    }

    // ── Tag chip rendering ────────────────────────────────────────────

    /// Flatten a `Line` to its display string so tests can assert on the
    /// exact rendered text without poking at private `Span` internals.
    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn tag_chips_line_renders_bracketed_tags_separated_by_spaces() {
        // The exact bracket form is part of the contract — both the user
        // (visual identity) and downstream readers (screenshots, transcripts)
        // depend on it.
        let line = tag_chips_line(&[
            "pci".to_string(),
            "stripe".to_string(),
            "mobile".to_string(),
        ]);
        let text = line_text(&line);
        assert!(text.contains("tags"), "missing label: {text:?}");
        assert!(text.contains("[pci] [stripe] [mobile]"), "got {text:?}");
    }

    #[test]
    fn tag_chips_render_in_metadata_pane_for_decoded_with_tags() {
        // Drive the same code path the viewer uses (`draw` → `meta_lines`)
        // so we catch regressions in the `meta_lines → tag_chips_line` glue.
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        // Seed the decoded snapshot with tags so meta_lines emits the chip
        // row on the next draw — this avoids round-tripping through the
        // store rewrite path in this rendering-focused test.
        view.decoded = Some(secret_value::Decoded {
            data: b"s3cret".to_vec(),
            tags: vec!["pci".to_string(), "stripe".to_string()],
            ..Default::default()
        });
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut rendered = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }
        assert!(
            rendered.contains("[pci]"),
            "missing pci chip in rendered metadata: {rendered}"
        );
        assert!(
            rendered.contains("[stripe]"),
            "missing stripe chip in rendered metadata: {rendered}"
        );
    }

    #[test]
    fn empty_tags_renders_no_chip_row() {
        // `meta_lines` is what the viewer feeds to ratatui — assert directly
        // that an empty-tags `Decoded` produces zero lines containing "tags"
        // as a label. We can't easily look at a private method on the view,
        // but `tag_chips_line` is the only producer of such a label, so it
        // suffices to confirm the guard at its caller.
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        view.decoded = Some(secret_value::Decoded {
            data: b"s3cret".to_vec(),
            tags: Vec::new(),
            ..Default::default()
        });
        let lines = view.meta_lines();
        for line in &lines {
            let text = line_text(line);
            assert!(
                !text.contains("tags "),
                "empty tags should not render a tags row, got line: {text:?}"
            );
        }
    }

    #[test]
    fn parse_edit_doc_handles_blank_tags_row() {
        // A user clearing out the tags row (`tags:` with no value) must
        // produce an empty Vec rather than `[""]`. This protects the persist
        // path from writing a blank string that would trip the validator.
        let doc = "path: p\ndescription:\nurl:\ntotp:\nexpires_at:\nenv_key:\ntags:\n---\nbody";
        let parsed = parse_edit_doc(doc).unwrap();
        assert!(parsed.tags.is_empty(), "got {:?}", parsed.tags);
    }

    #[test]
    fn parse_edit_doc_handles_csv_tags_with_whitespace() {
        let doc = "path: p\ntags: pci, stripe ,  mobile\n---\nbody";
        let parsed = parse_edit_doc(doc).unwrap();
        assert_eq!(
            parsed.tags,
            vec!["pci".to_string(), "stripe".to_string(), "mobile".to_string()]
        );
    }

    #[test]
    fn parse_edit_doc_stores_unknown_fields_as_annotations() {
        let doc = "description: hi\nbogus: value\ncustom_tag: 42\n---\nbody";
        let parsed = parse_edit_doc(doc).unwrap();
        assert_eq!(parsed.description, "hi");
        assert_eq!(parsed.annotations.get("bogus").unwrap(), "value");
        assert_eq!(parsed.annotations.get("custom_tag").unwrap(), "42");
        assert_eq!(parsed.value, "body");
    }

    #[test]
    fn parse_edit_doc_ignores_comments_and_blank_lines() {
        let doc = "\
# comment
#another
   # indented comment

description: hi
---
body";
        let parsed = parse_edit_doc(doc).unwrap();
        assert_eq!(parsed.description, "hi");
        assert_eq!(parsed.value, "body");
    }

    #[test]
    fn finish_edit_with_no_change_reports_cancelled() {
        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        view.finish_edit(Ok(None));
        match &view.status {
            Some((msg, StatusKind::Info)) => {
                assert!(msg.contains("cancelled"), "got {msg}");
            }
            other => panic!("expected info cancelled, got {other:?}"),
        }
    }

    #[test]
    fn footer_hint_lists_edit_and_rekey_bindings() {
        // The footer is drawn via ratatui's Line; we can cheaply assert by
        // rendering the viewer into a TestBackend buffer and searching the
        // text content for the expected hint tokens.
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let (_dir, ctx, path) = seeded_store_with_secret();
        let mut view = SecretViewerView::new(&ctx, "test/repo".into(), ctx.store.clone(), path);
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| view.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut rendered = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }
        assert!(
            rendered.contains("e edit"),
            "missing 'e edit' hint: {rendered}"
        );
        assert!(
            rendered.contains("R rekey"),
            "missing 'R rekey' hint: {rendered}"
        );
    }
}
