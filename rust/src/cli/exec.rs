//! `himitsu exec <REF> -- <CMD>...` — run a command with secrets injected
//! into its environment.
//!
//! `<REF>` uses the selector grammar:
//!   1. `tag:NAME` — all secrets carrying that tag
//!   2. `tag:A+tag:B` — AND across tags (both required)
//!   3. `prod/*` — glob against secret paths
//!   4. `prod/*+tag:pci` — glob AND tag
//!   5. `prod/API_KEY` — concrete secret path
//!   6. `tag:A,tag:B` — OR across groups (union)
//!
//! `--tag <T>` flags are treated as additional AND filters on top of the
//! selector, providing backward-compatible tag filtering.
//!
//! Conflicts (two secrets resolving to the same env-var name) are a hard
//! error: a half-injected env is more confusing than a clear failure.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Args;

use super::Context;
use crate::cli::export::glob_match;
use crate::config;
use crate::config::outputs::selector::{SecretMatch, Selector, Token};
use crate::crypto::{secret_value, tags as tag_grammar};
use crate::error::{HimitsuError, Result};
use crate::remote::store;

/// Run a command with secrets injected as environment variables.
#[derive(Debug, Args)]
pub struct ExecArgs {
    /// Secret reference — selector grammar:
    ///   * `tag:NAME` — all secrets tagged NAME
    ///   * `tag:A+tag:B` — AND-combined tags
    ///   * `prod/*` — path glob
    ///   * `prod/*+tag:pci` — glob AND tag
    ///   * `prod/API_KEY` — concrete path
    #[arg(value_name = "REF")]
    pub r#ref: String,

    /// Filter resolved secrets by tag. Repeat for AND-semantics. Secrets
    /// missing any required tag are dropped before injection.
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,

    /// Start the child with an empty environment, then inject the resolved
    /// secrets plus a minimal baseline (`PATH`, `HOME`, `TERM`).
    #[arg(long, short = 'i')]
    pub clean: bool,

    /// Command and arguments to run. Pass after `--` so `himitsu` does not
    /// try to interpret the command's own flags.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub command: Vec<String>,
}

pub fn run(args: ExecArgs, ctx: &Context) -> Result<()> {
    for t in &args.tags {
        tag_grammar::validate_tag(t).map_err(|reason| {
            HimitsuError::InvalidReference(format!("invalid tag {t:?}: {reason}"))
        })?;
    }

    if let Some(warning) = warn_if_shell_expanded(&args.r#ref) {
        eprintln!("{warning}");
    }

    let (cmd, cmd_args) = args
        .command
        .split_first()
        .expect("clap enforces required = true on `command`");

    let selector = Selector::parse(&args.r#ref)?;

    let all_paths = store::list_secrets(&ctx.store, None)?;
    let candidates: Vec<String> = all_paths
        .into_iter()
        .filter(|p| is_path_candidate(&selector, p))
        .collect();

    if candidates.is_empty() {
        return Err(HimitsuError::SecretNotFound(format!(
            "ref {:?} matched no secrets{hint}",
            args.r#ref
        )));
    }

    let identities = ctx.load_identities()?;
    let decrypted = decrypt_paths(ctx, &identities, candidates)?;
    let env_map = build_env_map(decrypted, &selector, &args.tags)?;

    if env_map.is_empty() {
        return Err(HimitsuError::SecretNotFound(format!(
            "ref {:?} matched no secrets",
            args.r#ref
        )));
    }

    spawn_and_wait(cmd, cmd_args, env_map, args.clean)
}

/// Returns true if the path could match any group in the selector based on
/// path/glob tokens alone. Tag tokens always pass (need decryption to check).
fn is_path_candidate(selector: &Selector, path: &str) -> bool {
    selector.0.iter().any(|group| {
        group.0.iter().all(|token| match token {
            Token::Tag(_) => true,
            Token::Glob(pattern) => glob_match(pattern, path),
            Token::Path(literal) => path == literal.as_str(),
        })
    })
}

fn decrypt_paths(
    ctx: &Context,
    identities: &[::age::x25519::Identity],
    paths: Vec<String>,
) -> Result<Vec<(String, secret_value::Decoded)>> {
    paths
        .into_iter()
        .map(|path| {
            let decoded = super::get::get_decoded_with_identities(ctx, &path, identities)?;
            super::get::warn_if_expired(&path, &decoded);
            Ok((path, decoded))
        })
        .collect()
}

/// Apply full selector match (including tag tokens), then `--tag` AND filter,
/// derive env-var names, detect conflicts, and return the injection map.
fn build_env_map(
    items: Vec<(String, secret_value::Decoded)>,
    selector: &Selector,
    want_tags: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut env_map: BTreeMap<String, (String, String)> = BTreeMap::new();

    for (path, decoded) in items {
        if !selector.matches(&SecretMatch {
            path: &path,
            tags: &decoded.tags,
        }) {
            continue;
        }

        if !want_tags.is_empty()
            && !want_tags
                .iter()
                .all(|t| decoded.tags.iter().any(|d| d == t))
        {
            continue;
        }

        let key = pick_env_key(&path, &decoded)?;
        super::set::validate_env_key(&key)
            .map_err(|e| HimitsuError::InvalidReference(format!("{e} (from {:?})", path)))?;

        if let Some((_, prev_path)) = env_map.get(&key) {
            return Err(HimitsuError::InvalidConfig(format!(
                "env-var {key:?} would be set by both {prev_path:?} and {:?}; \
                 rename one via `set --env-key` or a selector alias",
                path
            )));
        }

        let value = String::from_utf8(decoded.data).map_err(|e| {
            HimitsuError::InvalidReference(format!(
                "secret {:?} contains non-UTF-8 bytes — exec can only inject text values: {e}",
                path
            ))
        })?;

        env_map.insert(key, (value, path));
    }

    Ok(env_map.into_iter().map(|(k, (v, _))| (k, v)).collect())
}

/// Decide the env-var name for a decrypted secret:
/// 1. `SecretValue.env_key` set on the secret itself,
/// 2. `derive_env_key(last_segment_of_path)`.
fn pick_env_key(path: &str, decoded: &secret_value::Decoded) -> Result<String> {
    if !decoded.env_key.is_empty() {
        return Ok(decoded.env_key.clone());
    }
    let tail = config::env_dsl::last_component(path);
    if tail.is_empty() {
        return Err(HimitsuError::InvalidReference(format!(
            "secret path {:?} has no final segment to derive an env-var name from",
            path
        )));
    }
    Ok(config::env_dsl::derive_env_key(tail))
}

fn spawn_and_wait(
    cmd: &str,
    cmd_args: &[String],
    env_map: BTreeMap<String, String>,
    clean: bool,
) -> Result<()> {
    let mut child = std::process::Command::new(cmd);
    child.args(cmd_args);

    if clean {
        child.env_clear();
        for var in ["PATH", "HOME", "TERM"] {
            if let Ok(v) = std::env::var(var) {
                child.env(var, v);
            }
        }
    }
    child.envs(env_map);

    match child.status() {
        Ok(status) => {
            if let Some(code) = status.code() {
                std::process::exit(code);
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = status.signal() {
                    std::process::exit(128 + sig);
                }
            }
            std::process::exit(1);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(HimitsuError::External(format!(
            "command {cmd:?} not found on PATH"
        ))),
        Err(e) => Err(HimitsuError::Io(e)),
    }
}

/// Detect refs that look like shell-expanded filesystem paths and return a
/// warning message if so. When the user types `himitsu exec prod/*` without
/// quoting, the shell expands the glob before himitsu sees it. We can't fix
/// this but we can warn before the inevitable "secret not found" error.
fn warn_if_shell_expanded(ref_str: &str) -> Option<String> {
    // Absolute paths are never valid himitsu refs (they're always relative
    // store paths).
    if ref_str.starts_with('/') || ref_str.starts_with('~') {
        return Some(format!(
            "warning: ref {ref_str:?} looks like an absolute filesystem path — \
             the shell likely expanded a glob before himitsu saw it.\n  \
             Hint: quote the glob (e.g. 'prod/*') or just use the prefix (e.g. 'prod/' or 'prod')"
        ));
    }
    // Paths with common file extensions are almost certainly shell-expanded
    // filesystem paths, not secret store paths.
    let lowered = ref_str.to_ascii_lowercase();
    let has_ext = [
        ".yaml", ".age", ".json", ".yml", ".txt", ".env", ".toml", ".cfg", ".conf",
    ]
    .iter()
    .any(|ext| lowered.ends_with(ext));
    if has_ext {
        return Some(format!(
            "warning: ref {ref_str:?} has a file extension — the shell likely expanded \
             a glob before himitsu saw it.\n  \
             Hint: quote the glob (e.g. 'prod/*') or just use the prefix (e.g. 'prod/' or 'prod')"
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::outputs::selector::Group;
    use crate::crypto::secret_value::Decoded;

    fn decoded(data: &str, env_key: &str, tags: &[&str]) -> Decoded {
        Decoded {
            data: data.as_bytes().to_vec(),
            env_key: env_key.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn match_all() -> Selector {
        Selector(vec![Group(vec![])])
    }

    #[test]
    fn pick_env_key_priority_env_key_then_derive() {
        let k = pick_env_key("dev/whatever", &decoded("v", "API_TOKEN", &[])).unwrap();
        assert_eq!(k, "API_TOKEN");

        let k = pick_env_key("dev/api-key", &decoded("v", "", &[])).unwrap();
        assert_eq!(k, "API_KEY");

        let k = pick_env_key("dev/group/item-name", &decoded("v", "", &[])).unwrap();
        assert_eq!(k, "ITEM_NAME");
    }

    #[test]
    fn build_env_map_filters_by_tag_and_picks_keys() {
        let items = vec![
            ("a/api-key".to_string(), decoded("v1", "", &["pci"])),
            ("a/db".to_string(), decoded("v2", "DB_URL", &["pci"])),
            ("a/other".to_string(), decoded("v3", "", &["mobile"])),
        ];
        let map = build_env_map(items, &match_all(), &["pci".to_string()]).unwrap();
        assert_eq!(map.get("API_KEY").map(String::as_str), Some("v1"));
        assert_eq!(map.get("DB_URL").map(String::as_str), Some("v2"));
        assert!(!map.contains_key("OTHER"));
    }

    #[test]
    fn build_env_map_empty_tag_filter_keeps_everything() {
        let items = vec![
            ("a/x".to_string(), decoded("v1", "", &[])),
            ("a/y".to_string(), decoded("v2", "", &["any"])),
        ];
        let map = build_env_map(items, &match_all(), &[]).unwrap();
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn build_env_map_collision_errors_with_both_paths() {
        let items = vec![
            ("a/api-key".to_string(), decoded("first", "", &[])),
            ("b/API_KEY".to_string(), decoded("second", "API_KEY", &[])),
        ];
        let err = build_env_map(items, &match_all(), &[]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("API_KEY"), "msg should name the key: {msg}");
        assert!(
            msg.contains("a/api-key") && msg.contains("b/API_KEY"),
            "msg should name both source paths: {msg}"
        );
    }

    #[test]
    fn build_env_map_rejects_invalid_posix_env_name() {
        let items = vec![("a/x".to_string(), decoded("v", "1FOO", &[]))];
        let err = build_env_map(items, &match_all(), &[]).unwrap_err();
        assert!(err.to_string().contains("1FOO"));
    }

    #[test]
    fn build_env_map_rejects_non_utf8_value() {
        let mut d = decoded("", "", &[]);
        d.data = vec![0xff, 0xfe, 0xfd];
        let err = build_env_map(vec![("a/x".to_string(), d)], &match_all(), &[]).unwrap_err();
        assert!(err.to_string().contains("non-UTF-8"));
    }

    #[test]
    fn build_env_map_selector_filters_by_tag() {
        let selector = Selector::parse("tag:pci").unwrap();
        let items = vec![
            ("a/key".to_string(), decoded("v1", "PCI_KEY", &["pci"])),
            ("a/other".to_string(), decoded("v2", "OTHER", &["mobile"])),
        ];
        let map = build_env_map(items, &selector, &[]).unwrap();
        assert_eq!(map.get("PCI_KEY").map(String::as_str), Some("v1"));
        assert!(!map.contains_key("OTHER"));
    }

    #[test]
    fn build_env_map_and_tag_selector_requires_all_tags() {
        let selector = Selector::parse("tag:pci+tag:prod").unwrap();
        let items = vec![
            (
                "a/both".to_string(),
                decoded("v1", "BOTH", &["pci", "prod"]),
            ),
            (
                "a/pci-only".to_string(),
                decoded("v2", "PCI_ONLY", &["pci"]),
            ),
            (
                "a/prod-only".to_string(),
                decoded("v3", "PROD_ONLY", &["prod"]),
            ),
        ];
        let map = build_env_map(items, &selector, &[]).unwrap();
        assert_eq!(map.get("BOTH").map(String::as_str), Some("v1"));
        assert!(!map.contains_key("PCI_ONLY"));
        assert!(!map.contains_key("PROD_ONLY"));
    }

    #[test]
    fn is_path_candidate_tag_only_passes_all_paths() {
        let selector = Selector::parse("tag:pci").unwrap();
        assert!(is_path_candidate(&selector, "prod/key"));
        assert!(is_path_candidate(&selector, "dev/other"));
    }

    #[test]
    fn is_path_candidate_glob_filters_by_path() {
        let selector = Selector::parse("prod/*").unwrap();
        assert!(is_path_candidate(&selector, "prod/key"));
        assert!(!is_path_candidate(&selector, "dev/key"));
    }

    #[test]
    fn is_path_candidate_path_filters_exact() {
        let selector = Selector::parse("prod/api-key").unwrap();
        assert!(is_path_candidate(&selector, "prod/api-key"));
        assert!(!is_path_candidate(&selector, "prod/other"));
    }

    #[test]
    fn is_path_candidate_glob_and_tag_only_checks_glob() {
        let selector = Selector::parse("prod/*+tag:pci").unwrap();
        assert!(is_path_candidate(&selector, "prod/key"));
        assert!(!is_path_candidate(&selector, "dev/key"));
    }

    #[test]
    fn parses_command_after_double_dash() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct Cli {
            #[command(flatten)]
            args: ExecArgs,
        }

        let cli =
            Cli::try_parse_from(["test", "prod/API_KEY", "--", "node", "-e", "console.log(1)"])
                .unwrap();
        assert_eq!(cli.args.r#ref, "prod/API_KEY");
        assert_eq!(cli.args.command, vec!["node", "-e", "console.log(1)"]);
        assert!(!cli.args.clean);
        assert!(cli.args.tags.is_empty());

        let cli = Cli::try_parse_from([
            "test", "prod/*", "--tag", "pci", "--tag", "rotate", "-i", "--", "env",
        ])
        .unwrap();
        assert_eq!(cli.args.r#ref, "prod/*");
        assert_eq!(cli.args.tags, vec!["pci", "rotate"]);
        assert!(cli.args.clean);
        assert_eq!(cli.args.command, vec!["env"]);

        let cli = Cli::try_parse_from(["test", "tag:pci", "--", "env"]).unwrap();
        assert_eq!(cli.args.r#ref, "tag:pci");

        let cli = Cli::try_parse_from(["test", "tag:pci+tag:prod", "--", "env"]).unwrap();
        assert_eq!(cli.args.r#ref, "tag:pci+tag:prod");
    }

    #[test]
    fn parses_rejects_missing_command() {
        use clap::Parser;

        #[derive(Parser, Debug)]
        struct Cli {
            #[command(flatten)]
            args: ExecArgs,
        }

        let res = Cli::try_parse_from(["test", "prod/API_KEY"]);
        assert!(res.is_err());
    }

    #[test]
    fn warn_if_shell_expanded_absolute_path() {
        let w = warn_if_shell_expanded("/home/user/.himitsu/secrets/prod/API_KEY.yaml")
            .expect("absolute path should warn");
        assert!(w.contains("absolute filesystem path"), "got: {w}");
    }

    #[test]
    fn warn_if_shell_expanded_tilde_path() {
        let w = warn_if_shell_expanded("~/secrets/prod").expect("tilde path should warn");
        assert!(w.contains("absolute filesystem path"), "got: {w}");
    }

    #[test]
    fn warn_if_shell_expanded_file_extension() {
        let w = warn_if_shell_expanded("prod/API_KEY.yaml").expect("extension should warn");
        assert!(w.contains("file extension"), "got: {w}");
    }

    #[test]
    fn warn_if_shell_expanded_normal_ref_silent() {
        assert!(warn_if_shell_expanded("prod/API_KEY").is_none());
        assert!(warn_if_shell_expanded("prod/*").is_none());
        assert!(warn_if_shell_expanded("prod/").is_none());
        assert!(warn_if_shell_expanded("tag:pci").is_none());
    }
}
