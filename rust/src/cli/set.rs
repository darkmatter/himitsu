use clap::Args;

use super::duration::{self, ExpiresAt};
use super::Context;
use crate::crypto::{age, secret_value};
use crate::error::{HimitsuError, Result};
use crate::proto::SecretValue;

use crate::reference::SecretRef;
use crate::remote::store;

/// Set a secret value.
#[derive(Debug, Args)]
pub struct SetArgs {
    /// Secret path. Accepts a bare path (`prod/API_KEY`) or a provider-prefixed
    /// qualified reference (`github:org/repo/prod/API_KEY`) that overrides the
    /// default store.
    pub path: String,
    /// Secret value.
    pub value: String,
    /// Skip git commit and push.
    #[arg(long)]
    pub no_push: bool,
    /// TOTP secret — either an `otpauth://` URI or a base32 string (>= 16 chars).
    #[arg(long)]
    pub totp: Option<String>,
    /// Associated website or API URL.
    #[arg(long)]
    pub url: Option<String>,
    /// Human-readable description.
    #[arg(long)]
    pub description: Option<String>,
    /// Optional expiration reminder: RFC 3339 timestamp, relative duration
    /// (`30d`, `6mo`, `1y`), or the literal `never` to clear.
    #[arg(long)]
    pub expires_at: Option<String>,
}

pub fn run(args: SetArgs, ctx: &Context) -> Result<()> {
    let secret_ref = SecretRef::parse(&args.path)?;
    let (effective_store, secret_path, recipients_path_override) = if secret_ref.is_qualified() {
        let resolved = secret_ref.resolve_store()?;
        let path = secret_ref.path.ok_or_else(|| {
            HimitsuError::InvalidReference(
                "qualified reference must include a secret path after org/repo".into(),
            )
        })?;
        // For cross-store writes, use the target store's default recipients layout.
        (resolved, path, None)
    } else {
        let path = secret_ref.path.expect("bare SecretRef always has a path");
        (ctx.store.clone(), path, ctx.recipients_path.as_deref())
    };

    let recipients = age::collect_recipients(&effective_store, recipients_path_override)?;
    if recipients.is_empty() {
        return Err(HimitsuError::Recipient(
            "no recipients found; run `himitsu init` or add recipients first".into(),
        ));
    }

    // Validate TOTP input early so the user gets a good error before we encrypt.
    if let Some(ref totp) = args.totp {
        validate_totp(totp)?;
    }

    // Resolve expires_at expression into a proto timestamp (if any).
    let expires_at_ts = match args.expires_at.as_deref() {
        None => None,
        Some(raw) => match duration::parse(raw)? {
            ExpiresAt::Never => None,
            ExpiresAt::At(dt) => Some(duration::to_proto_timestamp(dt)),
        },
    };

    let sv = SecretValue {
        data: args.value.as_bytes().to_vec(),
        content_type: String::new(),
        annotations: Default::default(),
        totp: args.totp.clone().unwrap_or_default(),
        url: args.url.clone().unwrap_or_default(),
        expires_at: expires_at_ts,
        description: args.description.clone().unwrap_or_default(),
    };

    let plaintext = secret_value::encode(&sv);
    let ciphertext = age::encrypt(&plaintext, &recipients)?;
    store::write_secret(&effective_store, &secret_path, &ciphertext)?;

    if !args.no_push {
        ctx.commit_and_push(&format!("himitsu: set {secret_path}"));
    }

    println!("Set {secret_path}");
    Ok(())
}

/// Validate a TOTP input.
///
/// Accepts either:
///   * an `otpauth://` URI (contents not deeply validated — left to a future
///     dedicated parser), or
///   * a raw base32 secret of at least 16 characters (RFC 4648 alphabet,
///     optional `=` padding and whitespace allowed).
pub(crate) fn validate_totp(input: &str) -> Result<()> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(HimitsuError::InvalidReference(
            "totp value is empty".into(),
        ));
    }

    if trimmed.starts_with("otpauth://") {
        return Ok(());
    }

    let cleaned: String = trimmed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    let body = cleaned.trim_end_matches('=');

    if body.len() < 16 {
        return Err(HimitsuError::InvalidReference(format!(
            "totp secret too short (got {} base32 chars, need >= 16) — pass an otpauth:// URI or a longer base32 string",
            body.len()
        )));
    }

    let is_base32 = body
        .chars()
        .all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '2'..='7'));
    if !is_base32 {
        return Err(HimitsuError::InvalidReference(
            "totp secret must be base32 (A–Z, 2–7) or an otpauth:// URI".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_accepts_otpauth_uri() {
        assert!(validate_totp("otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP&issuer=Example").is_ok());
    }

    #[test]
    fn totp_accepts_32_char_base32() {
        assert!(validate_totp("JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP").is_ok());
    }

    #[test]
    fn totp_rejects_short_input() {
        let err = validate_totp("abc").unwrap_err();
        assert!(err.to_string().contains("too short"));
    }

    #[test]
    fn totp_rejects_non_base32() {
        let err = validate_totp("1111111111111111!!!").unwrap_err();
        assert!(err.to_string().contains("base32"));
    }

    #[test]
    fn totp_rejects_empty() {
        assert!(validate_totp("").is_err());
    }
}
