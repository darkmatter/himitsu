use std::io::{self, IsTerminal, Write};

use clap::Args;

use super::Context;
use super::duration::{self, ExpirySeverity};
use crate::crypto::secret_value;
use crate::error::Result;

// ANSI color escape sequences used when stderr is a tty.
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RED: &str = "\x1b[31m";

/// Get a secret value.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// Secret path. Accepts a bare path (`prod/API_KEY`) or a provider-prefixed
    /// qualified reference (`github:org/repo/prod/API_KEY`) that overrides the
    /// default store.
    pub path: String,
}

pub fn run(args: GetArgs, ctx: &Context) -> Result<()> {
    let decoded = get_decoded(ctx, &args.path)?;
    io::stdout().write_all(&decoded.data)?;
    emit_metadata_block(&decoded);
    Ok(())
}

/// Decrypt and return only the raw plaintext bytes for a secret reference.
/// Used by `himitsu read` and other scripting paths that must not emit metadata.
pub fn get_plaintext(ctx: &Context, path: &str) -> Result<Vec<u8>> {
    Ok(get_decoded(ctx, path)?.data)
}

/// Decrypt and return the full decoded SecretValue for a secret reference.
pub(crate) fn get_decoded(ctx: &Context, path: &str) -> Result<secret_value::Decoded> {
    super::resolver::SecretResolver::resolve(ctx, path)
}

/// Same as [`get_decoded`] but reuses pre-loaded identities. Use this when
/// decrypting many secrets in a loop so key files aren't re-parsed per
/// iteration (e.g. `himitsu exec` over a glob or env label).
pub(crate) fn get_decoded_with_identities(
    ctx: &Context,
    path: &str,
    identities: &[::age::x25519::Identity],
) -> Result<secret_value::Decoded> {
    super::resolver::SecretResolver::resolve_with_identities(ctx, path, identities)
}

/// Map file pubkeys to named recipients using the store's recipients directory.
pub(crate) fn named_recipients(store: &std::path::Path, file_pubkeys: &[String]) -> Vec<String> {
    use crate::remote::store as rstore;
    let rdir = rstore::recipients_dir(store);
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    collect_recipient_map(&rdir, &rdir, &mut map);
    file_pubkeys
        .iter()
        .map(|pk| {
            let short = short_key(pk);
            if let Some(name) = map.get(pk.trim()) {
                format!("{name} ({short})")
            } else {
                format!("(unknown) {short}")
            }
        })
        .collect()
}

pub(crate) fn collect_recipient_map(
    base: &std::path::Path,
    dir: &std::path::Path,
    map: &mut std::collections::HashMap<String, String>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_recipient_map(base, &path, map);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("pub") {
            continue;
        }
        let Ok(key) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .with_extension("")
            .to_string_lossy()
            .to_string();
        map.insert(key.trim().to_string(), rel);
    }
}

pub(crate) fn short_key(pk: &str) -> String {
    let s = pk.trim();
    if s.len() <= 12 {
        s.to_string()
    } else {
        format!("{}...", &s[..12])
    }
}

fn emit_metadata_block(decoded: &secret_value::Decoded) {
    let stderr = io::stderr();
    let is_tty = stderr.is_terminal();
    let mut out = stderr.lock();

    if !decoded.url.is_empty() {
        let _ = writeln!(out, "url:         {}", decoded.url);
    }
    if !decoded.totp.is_empty() {
        let _ = writeln!(out, "totp:        {}", decoded.totp);
    }
    if !decoded.description.is_empty() {
        let _ = writeln!(out, "description: {}", decoded.description);
    }
    if !decoded.env_key.is_empty() {
        let _ = writeln!(out, "env_key:     {}", decoded.env_key);
    }
    if !decoded.tags.is_empty() {
        let _ = writeln!(out, "tags:        {}", decoded.tags.join(", "));
    }

    if let Some(ref ts) = decoded.expires_at {
        if !duration::is_unset(ts) {
            if let Some(dt) = duration::from_proto_timestamp(ts) {
                let now = chrono::Utc::now();
                let (msg, sev) = duration::describe_remaining(dt, now);
                let rfc = dt.to_rfc3339();
                let line = format!("expires:     {rfc}  ({msg})");
                let _ = writeln!(out, "{}", colorize(&line, sev, is_tty));
            }
        }
    }
}

pub(crate) fn warn_if_expired(path: &str, decoded: &secret_value::Decoded) {
    if let Some(msg) = expiry_message(path, decoded.expires_at.as_ref()) {
        eprintln!("{msg}");
    }
}

/// Canonical expired-secret message, or `None` when no expiry is set, the
/// timestamp is unset, or it hasn't passed yet. Channel-free: callers decide
/// where it renders (CLI stderr, TUI badge).
pub(crate) fn expiry_message(
    path: &str,
    expires_at: Option<&pbjson_types::Timestamp>,
) -> Option<String> {
    let ts = expires_at?;
    if duration::is_unset(ts) {
        return None;
    }
    let dt = duration::from_proto_timestamp(ts)?;
    (dt <= chrono::Utc::now())
        .then(|| format!("warning: secret '{path}' expired at {}", dt.to_rfc3339()))
}

fn colorize(s: &str, sev: ExpirySeverity, is_tty: bool) -> String {
    if !is_tty {
        return s.to_string();
    }
    match sev {
        ExpirySeverity::Distant => format!("{ANSI_DIM}{s}{ANSI_RESET}"),
        ExpirySeverity::Soon => format!("{ANSI_YELLOW}{s}{ANSI_RESET}"),
        ExpirySeverity::Expired => format!("{ANSI_RED}{s}{ANSI_RESET}"),
    }
}
