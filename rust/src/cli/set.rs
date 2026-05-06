use clap::Args;

use super::duration::{self, ExpiresAt};
use super::Context;
use crate::crypto::{age, secret_value, tags as tag_grammar};
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
    /// Default environment variable name used when this secret is injected
    /// into a process environment (e.g. `himitsu exec`). When unset, the
    /// callers derive one from the last path segment.
    #[arg(long = "env-key", value_name = "NAME")]
    pub env_key: Option<String>,
    /// Optional expiration reminder: RFC 3339 timestamp, relative duration
    /// (`30d`, `6mo`, `1y`), or the literal `never` to clear.
    #[arg(long)]
    pub expires_at: Option<String>,
    /// Attach a tag to this secret. Repeat the flag to attach multiple
    /// (`--tag pci --tag rotate-2026-q1`). Tags follow the grammar
    /// `[A-Za-z0-9_.-]+`, 1-64 chars, case-sensitive.
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
}

pub fn run(args: SetArgs, ctx: &Context) -> Result<()> {
    if let Some(ref totp) = args.totp {
        validate_totp(totp)?;
    }
    if let Some(ref env_key) = args.env_key {
        validate_env_key(env_key)?;
    }
    let tags = validate_tags(&args.tags)?;

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
        env_key: args.env_key.clone().unwrap_or_default(),
        tags,
    };

    let secret_path = encrypt_and_write(ctx, &args.path, &sv)?;
    println!("Set {secret_path}");
    Ok(())
}

/// Encrypt raw `plaintext` bytes and persist them at `path`. Used by
/// `himitsu write` and other scripting paths that carry only minimal metadata.
///
/// `tags` are stored verbatim on the resulting `SecretValue`. Callers that
/// don't want tags should pass `Vec::new()`. Tags are NOT validated here —
/// validation must happen at the CLI/UI layer so users see errors before any
/// I/O.
pub fn set_plaintext(
    ctx: &Context,
    path: &str,
    plaintext: &[u8],
    tags: Vec<String>,
) -> Result<String> {
    let sv = SecretValue {
        data: plaintext.to_vec(),
        content_type: String::new(),
        annotations: Default::default(),
        totp: String::new(),
        url: String::new(),
        expires_at: None,
        description: String::new(),
        env_key: String::new(),
        tags,
    };
    encrypt_and_write(ctx, path, &sv)
}

fn encrypt_and_write(ctx: &Context, path: &str, sv: &SecretValue) -> Result<String> {
    let secret_ref = SecretRef::parse(path)?;
    let (effective_store, secret_path, recipients_path_override) = if secret_ref.is_qualified() {
        let resolved = secret_ref.resolve_store()?;
        let path = secret_ref.path.ok_or_else(|| {
            HimitsuError::InvalidReference(
                "qualified reference must include a secret path after org/repo".into(),
            )
        })?;
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

    let plaintext = secret_value::encode(sv);
    let ciphertext = age::encrypt(&plaintext, &recipients)?;
    store::write_secret(&effective_store, &secret_path, &ciphertext)?;

    Ok(secret_path)
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
        return Err(HimitsuError::InvalidReference("totp value is empty".into()));
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

/// Validate a `--env-key` override.
///
/// Must be a legal POSIX environment variable name: a letter or underscore
/// followed by letters, digits, or underscores. We're deliberately strict so
/// a typo like `--env-key db url` fails loudly instead of silently injecting
/// under a name no shell can read.
pub(crate) fn validate_env_key(input: &str) -> Result<()> {
    if input.is_empty() {
        return Err(HimitsuError::InvalidReference("env-key is empty".into()));
    }
    let mut chars = input.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(HimitsuError::InvalidReference(format!(
            "env-key must start with a letter or underscore (got {input:?})"
        )));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(HimitsuError::InvalidReference(format!(
            "env-key may only contain letters, digits, or underscores (got {input:?})"
        )));
    }
    Ok(())
}

/// Validate every `--tag` value against the shared tag grammar before any
/// I/O happens. Returns the owned, validated list so the caller can move it
/// into `SecretValue.tags`.
fn validate_tags(raw: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(raw.len());
    for t in raw {
        tag_grammar::validate_tag(t).map_err(|reason| {
            HimitsuError::InvalidReference(format!("invalid tag {t:?}: {reason}"))
        })?;
        out.push(t.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn totp_accepts_otpauth_uri() {
        assert!(validate_totp(
            "otpauth://totp/Example:alice?secret=JBSWY3DPEHPK3PXP&issuer=Example"
        )
        .is_ok());
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

    #[test]
    fn env_key_accepts_uppercase_with_underscores() {
        assert!(validate_env_key("DATABASE_URL").is_ok());
        assert!(validate_env_key("_PRIVATE").is_ok());
        assert!(validate_env_key("API_KEY_2").is_ok());
    }

    #[test]
    fn env_key_rejects_leading_digit() {
        let err = validate_env_key("1FOO").unwrap_err();
        assert!(err.to_string().contains("letter or underscore"));
    }

    #[test]
    fn env_key_rejects_hyphen_and_space() {
        assert!(validate_env_key("FOO-BAR").is_err());
        assert!(validate_env_key("FOO BAR").is_err());
    }

    #[test]
    fn env_key_rejects_empty() {
        assert!(validate_env_key("").is_err());
    }

    // --- `--tag` flag --------------------------------------------------

    /// Wrap `SetArgs` in a tiny CLI so we can drive clap's parser the same
    /// way the real binary does.
    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(subcommand)]
        cmd: TestCmd,
    }

    #[derive(Debug, clap::Subcommand)]
    enum TestCmd {
        Set(SetArgs),
    }

    fn parse(args: &[&str]) -> SetArgs {
        let mut full = vec!["test", "set"];
        full.extend_from_slice(args);
        let TestCli {
            cmd: TestCmd::Set(a),
        } = TestCli::try_parse_from(full).expect("parse ok");
        a
    }

    #[test]
    fn tag_flag_accumulates_multiple_invocations() {
        let a = parse(&["prod/API_KEY", "secret-value", "--tag", "pci", "--tag", "rotate-2026-q1"]);
        assert_eq!(a.tags, vec!["pci".to_string(), "rotate-2026-q1".to_string()]);
    }

    #[test]
    fn tag_flag_defaults_to_empty_vec() {
        let a = parse(&["prod/API_KEY", "secret-value"]);
        assert!(a.tags.is_empty());
        // And `validate_tags` happily round-trips an empty list — preserving
        // the pre-tags behaviour for callers that never pass `--tag`.
        assert_eq!(validate_tags(&a.tags).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn validate_tags_rejects_invalid_before_io() {
        // `validate_tags` is the same code path `run()` hits before any
        // encryption or filesystem work, so a failure here proves bad input
        // is rejected before I/O.
        let raw = vec!["ok".to_string(), "bad tag".to_string()];
        let err = validate_tags(&raw).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bad tag"), "error mentions offending tag: {msg}");
        assert!(msg.contains("invalid tag"), "uses canonical prefix: {msg}");
    }

    #[test]
    fn validate_tags_passes_through_valid_list() {
        let raw = vec!["pci".to_string(), "team_backend".to_string(), "v1.2.3".to_string()];
        assert_eq!(validate_tags(&raw).unwrap(), raw);
    }
}
