use std::io::{self, IsTerminal, Write};

use clap::Args;

use super::duration::{self, ExpirySeverity};
use super::Context;
use crate::crypto::{age, secret_value};
use crate::error::{HimitsuError, Result};
use crate::reference::SecretRef;
use crate::remote::store;

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
fn get_decoded(ctx: &Context, path: &str) -> Result<secret_value::Decoded> {
    let identity = age::read_identity(&ctx.key_path())?;
    get_decoded_with_identity(ctx, path, &identity)
}

/// Same as [`get_decoded`] but reuses a pre-loaded identity. Use this when
/// decrypting many secrets in a loop so the key file isn't re-parsed per
/// iteration (e.g. `himitsu exec` over a glob or env label).
pub(crate) fn get_decoded_with_identity(
    ctx: &Context,
    path: &str,
    identity: &::age::x25519::Identity,
) -> Result<secret_value::Decoded> {
    let secret_ref = SecretRef::parse(path)?;

    let (effective_store, secret_path) = if secret_ref.is_qualified() {
        let resolved = secret_ref.resolve_store()?;
        let path = secret_ref.path.ok_or_else(|| {
            HimitsuError::InvalidReference(
                "qualified reference must include a secret path after org/repo".into(),
            )
        })?;
        (resolved, path)
    } else {
        let path = secret_ref.path.expect("bare SecretRef always has a path");
        (ctx.store.clone(), path)
    };

    let ciphertext = store::read_secret(&effective_store, &secret_path)?;
    let plaintext = age::decrypt(&ciphertext, identity)?;
    Ok(secret_value::decode(&plaintext))
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
