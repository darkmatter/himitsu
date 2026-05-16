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
pub(crate) fn get_decoded(ctx: &Context, path: &str) -> Result<secret_value::Decoded> {
    let identities = ctx.load_identities()?;
    get_decoded_with_identities(ctx, path, &identities)
}

/// Same as [`get_decoded`] but reuses pre-loaded identities. Use this when
/// decrypting many secrets in a loop so key files aren't re-parsed per
/// iteration (e.g. `himitsu exec` over a glob or env label).
pub(crate) fn get_decoded_with_identities(
    ctx: &Context,
    path: &str,
    identities: &[::age::x25519::Identity],
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

    let meta = store::read_secret_meta(&effective_store, &secret_path)?;
    let ciphertext = store::read_secret(&effective_store, &secret_path)?;

    match age::decrypt_with_identities(&ciphertext, identities) {
        Ok(plaintext) => Ok(secret_value::decode(&plaintext)),
        Err(_) => {
            let named = named_recipients(&effective_store, &meta.recipients);
            let loaded: Vec<String> = identities
                .iter()
                .map(|id| id.to_public().to_string())
                .collect();
            let mut msg = String::from("no matching key\n  encrypted for:\n");
            for n in &named {
                msg.push_str(&format!("    {n}\n"));
            }
            msg.push_str("  loaded identities:\n");
            if loaded.is_empty() {
                msg.push_str("    (none)\n");
            }
            for id in &loaded {
                msg.push_str(&format!("    {id}\n"));
            }
            msg.push_str(
                "  hint: run 'himitsu rekey' if your current identity should have access",
            );
            Err(HimitsuError::DecryptionFailed(msg))
        }
    }
}

/// Map file pubkeys to named recipients using the store's recipients directory.
fn named_recipients(store: &std::path::Path, file_pubkeys: &[String]) -> Vec<String> {
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

fn collect_recipient_map(
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

fn short_key(pk: &str) -> String {
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
    let Some(ts) = decoded.expires_at.as_ref() else {
        return;
    };
    if duration::is_unset(ts) {
        return;
    }
    let Some(dt) = duration::from_proto_timestamp(ts) else {
        return;
    };
    if dt <= chrono::Utc::now() {
        eprintln!("warning: secret '{path}' expired at {}", dt.to_rfc3339());
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
