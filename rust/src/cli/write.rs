use std::io::{self, Read};

use clap::Args;

use super::Context;
use crate::crypto::tags::validate_tag;
use crate::error::{HimitsuError, Result};

/// Write a secret's plaintext from an argument or stdin without any decoration.
#[derive(Debug, Args)]
pub struct WriteArgs {
    /// Secret path. Accepts a bare path (`prod/API_KEY`) or a provider-prefixed
    /// qualified reference (`github:org/repo/prod/API_KEY`).
    pub path: String,
    /// Plaintext value. When omitted, stdin is read instead.
    pub value: Option<String>,
    /// Force reading the value from stdin even if a positional value is given.
    #[arg(long)]
    pub stdin: bool,
    /// Skip git commit and push.
    #[arg(long)]
    pub no_push: bool,
    /// Attach a tag to the secret. Repeat for multiple tags. Tags must match
    /// `[A-Za-z0-9_.-]+` (1–64 chars, case-sensitive, no whitespace).
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
}

pub fn run(args: WriteArgs, ctx: &Context) -> Result<()> {
    // Validate tags BEFORE any I/O so users see grammar errors before we read
    // stdin or hit the store.
    for tag in &args.tags {
        validate_tag(tag).map_err(HimitsuError::InvalidReference)?;
    }

    let plaintext: Vec<u8> = match (args.stdin, args.value) {
        (false, Some(value)) => value.into_bytes(),
        _ => {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf)?;
            buf
        }
    };

    super::set::set_plaintext(ctx, &args.path, &plaintext, args.tags)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Minimal harness so we can drive `WriteArgs` through clap without
    /// pulling in the full top-level CLI tree.
    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        args: WriteArgs,
    }

    fn parse(argv: &[&str]) -> WriteArgs {
        let mut full = vec!["write"];
        full.extend_from_slice(argv);
        Harness::parse_from(full).args
    }

    /// Replays the validation step that `run()` performs, without needing a
    /// `Context`. Returns the first error string, mirroring `run()`'s
    /// short-circuit behavior.
    fn validate_all(args: &WriteArgs) -> std::result::Result<(), String> {
        args.tags
            .iter()
            .try_for_each(|t| validate_tag(t).map(|_| ()))
    }

    #[test]
    fn rejects_invalid_tag() {
        let args = parse(&["prod/API_KEY", "v", "--tag", "has space"]);
        let err = validate_all(&args).unwrap_err();
        assert!(err.contains("invalid character"), "got: {err}");
    }

    #[test]
    fn rejects_empty_tag() {
        let args = parse(&["prod/API_KEY", "v", "--tag", ""]);
        let err = validate_all(&args).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn multiple_tags_accumulate() {
        let args = parse(&[
            "prod/API_KEY",
            "v",
            "--tag",
            "pci",
            "--tag",
            "rotate-2026",
            "--tag",
            "team_backend",
        ]);
        assert_eq!(args.tags, vec!["pci", "rotate-2026", "team_backend"]);
        // All three must validate.
        for t in &args.tags {
            assert!(validate_tag(t).is_ok(), "{t} should validate");
        }
    }

    #[test]
    fn no_tag_flag_yields_empty_vec() {
        let args = parse(&["prod/API_KEY", "v"]);
        assert!(args.tags.is_empty());
    }
}
