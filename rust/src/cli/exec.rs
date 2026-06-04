//! `himitsu exec <REF> -- <CMD>...` — run a command with secrets injected
//! into its environment.
//!
//! `<REF>` resolves to one or more secrets:
//!   1. `tag:NAME` — inject all secrets carrying that tag (e.g. `tag:pci`).
//!      Tag names are validated by [`crypto::tags::validate_tag`].
//!   2. an env label from `.himitsu.yaml` `envs:` (e.g. `pci-prod`) — uses the
//!      env DSL resolver; the env-DSL alias key (or path-derived key) becomes
//!      the env-var name.
//!   3. a path glob ending in `/*` (e.g. `prod/*`) — every secret under the
//!      prefix; env-var name comes from `SecretValue.env_key` if set, else
//!      derived from the path's last segment via [`config::env_dsl::derive_env_key`].
//!   4. a trailing-slash prefix (e.g. `prod/`) — equivalent to `prod/*` but
//!      avoids the shell glob-expansion pitfall.
//!   5. a concrete secret path (e.g. `prod/API_KEY`, optionally
//!      `github:org/repo/prod/API_KEY`).
//!
//! Conflicts (two secrets resolving to the same env-var name) are a hard
//! error: a half-injected env is more confusing than a clear failure.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Args;

use super::Context;
use crate::config::{self, env_resolver, validate_env_label};
use crate::crypto::{secret_value, tags as tag_grammar};
use crate::error::{HimitsuError, Result};
use crate::reference::SecretRef;
use crate::remote::store;

/// Run a command with secrets injected as environment variables.
#[derive(Debug, Args)]
pub struct ExecArgs {
    /// Secret reference. One of:
    ///   * tag selector (e.g. `tag:pci`) — inject all secrets with that tag
    ///   * env label from project `envs:` map (e.g. `pci-prod`)
    ///   * path glob ending in `/*` (e.g. `prod/*`)
    ///   * trailing-slash prefix (e.g. `prod/`) — equivalent to `prod/*`
    ///   * concrete secret path (e.g. `prod/API_KEY`)
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

    let resolved = resolve_ref(&args.r#ref, ctx)?;
    if resolved.is_empty() {
        let hint = if args.r#ref.ends_with("/*") || args.r#ref.ends_with('/') {
            "\n  Hint: use a trailing slash without quotes (e.g. 'prod/') to avoid shell glob expansion,\n  or pass --remote <slug> to select a specific store."
        } else {
            "\n  Hint: pass --remote <slug> to select a specific store, or check 'himitsu ls' for available secrets."
        };
        return Err(HimitsuError::SecretNotFound(format!(
            "ref {:?} matched no secrets{hint}",
            args.r#ref
        )));
    }

    // Load age identities once so we don't re-parse key files per resolved
    // secret. `exec` is the first hot loop of decrypts and the win is real.
    let identities = ctx.load_identities()?;
    let decrypted = decrypt_resolved(ctx, &identities, resolved)?;
    let env_map = build_env_map(decrypted, &args.tags)?;

    spawn_and_wait(cmd, cmd_args, env_map, args.clean)
}

/// One pre-decryption hit: a secret path plus an optional explicit env-var
/// name carried in by the env DSL.
struct ResolvedRef {
    secret_path: String,
    /// The store checkout path that holds this secret. Needed because glob/tag
    /// resolution searches ALL known stores, not just the active one.
    store_path: PathBuf,
    /// `Some` when the env DSL pinned the name (alias or resolver-derived);
    /// `None` for glob/concrete refs — name picked post-decrypt.
    explicit_key: Option<String>,
}

/// Collect all known store paths. The active store comes first, then any
/// additional stores discovered in the XDG state directory. Mirrors the
/// logic in `ls::collect_stores` but returns only `PathBuf` (no labels).
fn collect_stores(ctx: &Context) -> Vec<PathBuf> {
    let mut stores = Vec::new();

    if !ctx.store.as_os_str().is_empty() && ctx.store.exists() {
        stores.push(ctx.store.clone());
    }

    let stores_dir = ctx.stores_dir();
    if stores_dir.exists() {
        if let Ok(org_entries) = std::fs::read_dir(&stores_dir) {
            for org_entry in org_entries.flatten() {
                if !org_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                if let Ok(repo_entries) = std::fs::read_dir(org_entry.path()) {
                    for repo_entry in repo_entries.flatten() {
                        if !repo_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            continue;
                        }
                        let path = repo_entry.path();
                        if path != ctx.store {
                            stores.push(path);
                        }
                    }
                }
            }
        }
    }

    stores
}

fn resolve_ref(ref_str: &str, ctx: &Context) -> Result<Vec<ResolvedRef>> {
    // Check for `tag:` prefix — intercept before `SecretRef::parse` treats
    // ':' as a provider separator. Env labels can't contain ':' so this
    // never collides with the env-label namespace.
    if let Some(tag_name) = ref_str.strip_prefix("tag:") {
        if tag_name.is_empty() {
            return Err(HimitsuError::InvalidReference(
                "tag selector requires a tag name after 'tag:' (e.g. tag:pci)".into(),
            ));
        }
        tag_grammar::validate_tag(tag_name).map_err(|reason| {
            HimitsuError::InvalidReference(format!("invalid tag in selector: {reason}"))
        })?;
        let stores = collect_stores(ctx);
        let identities = ctx.load_identities()?;
        let mut results = Vec::new();
        for store_path in &stores {
            let available = match store::list_secrets(store_path, None) {
                Ok(a) => a,
                Err(_) => continue,
            };
            for secret_path in available {
                let store_ctx = Context {
                    store: store_path.clone(),
                    ..ctx.clone()
                };
                if let Ok(decoded) =
                    super::get::get_decoded_with_identities(&store_ctx, &secret_path, &identities)
                {
                    if decoded.tags.iter().any(|t| t == tag_name) {
                        results.push(ResolvedRef {
                            secret_path,
                            store_path: store_path.clone(),
                            explicit_key: None,
                        });
                    }
                }
            }
        }
        return Ok(results);
    }

    // Env labels live in their own namespace and always win when they match
    // exactly: project authoring intent beats path coincidence.
    let envs = config::load_effective_envs()?;
    if envs.contains_key(ref_str) {
        if config::is_wildcard_label(ref_str) {
            return Err(HimitsuError::NotSupported(format!(
                "exec does not support wildcard env labels ({ref_str:?}); \
                     pass a concrete env or use `himitsu codegen` for templated output"
            )));
        }
        validate_env_label(ref_str)?;
        let available = store::list_secrets(&ctx.store, None)?;
        let identities = ctx.load_identities()?;
        let tag_lookup = |path: &str| {
            super::get::get_decoded_with_identities(ctx, path, &identities)
                .map(|decoded| decoded.tags)
        };
        let tree = env_resolver::resolve_with_tags(&envs, ref_str, &available, &tag_lookup)?;
        let leaves = collect_env_leaves(&tree);
        return Ok(leaves
            .into_iter()
            .map(|(key, secret_path)| ResolvedRef {
                secret_path,
                store_path: ctx.store.clone(),
                explicit_key: Some(key),
            })
            .collect());
    }

    // Glob and prefix branches search ALL known stores, not just the active
    // one. This matches `ls` behavior and fixes the case where secrets live
    // in a non-default store.
    if let Some(prefix) = ref_str.strip_suffix("/*") {
        if prefix.is_empty() {
            return Err(HimitsuError::InvalidReference(
                "bare `/*` is not a valid ref; specify a prefix (e.g. `prod/*`)".into(),
            ));
        }
        let needle = format!("{prefix}/");
        let stores = collect_stores(ctx);
        let mut results = Vec::new();
        for store_path in &stores {
            let available = match store::list_secrets(store_path, None) {
                Ok(a) => a,
                Err(_) => continue,
            };
            for secret_path in available {
                if secret_path.starts_with(&needle) {
                    results.push(ResolvedRef {
                        secret_path,
                        store_path: store_path.clone(),
                        explicit_key: None,
                    });
                }
            }
        }
        return Ok(results);
    }

    // Trailing slash = prefix glob (same as `prefix/*` but without the star).
    if ref_str.ends_with('/') {
        let prefix = ref_str.trim_end_matches('/');
        if prefix.is_empty() {
            return Err(HimitsuError::InvalidReference(
                "bare '/' is not a valid ref; specify a prefix (e.g. 'prod/')".into(),
            ));
        }
        let needle = format!("{prefix}/");
        let stores = collect_stores(ctx);
        let mut results = Vec::new();
        for store_path in &stores {
            let available = match store::list_secrets(store_path, None) {
                Ok(a) => a,
                Err(_) => continue,
            };
            for secret_path in available {
                if secret_path.starts_with(&needle) {
                    results.push(ResolvedRef {
                        secret_path,
                        store_path: store_path.clone(),
                        explicit_key: None,
                    });
                }
            }
        }
        return Ok(results);
    }

    let parsed = SecretRef::parse(ref_str)?;
    if parsed.path.is_none() {
        return Err(HimitsuError::InvalidReference(format!(
            "ref {ref_str:?} has no secret path"
        )));
    }
    Ok(vec![ResolvedRef {
        secret_path: ref_str.to_string(),
        store_path: ctx.store.clone(),
        explicit_key: None,
    }])
}

/// Walk an [`env_resolver::EnvNode`] tree and return every `(parent_key,
/// secret_path)` pair where a leaf sits one level beneath a branch.
///
/// The outer label-prefix branch (e.g. `dev` in `Branch{dev: Branch{API_KEY:
/// Leaf}}`) is collapsed because it carries the env name, not a variable.
fn collect_env_leaves(node: &env_resolver::EnvNode) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

fn walk(node: &env_resolver::EnvNode, out: &mut Vec<(String, String)>) {
    if let env_resolver::EnvNode::Branch(map) = node {
        for (key, child) in map {
            match child {
                env_resolver::EnvNode::Leaf { secret_path } => {
                    out.push((key.clone(), secret_path.clone()));
                }
                env_resolver::EnvNode::Branch(_) => walk(child, out),
            }
        }
    }
}

/// Decrypt every resolved ref into `(ResolvedRef, Decoded)` pairs using
/// shared identities. Pure I/O — no filtering.
fn decrypt_resolved(
    ctx: &Context,
    identities: &[::age::x25519::Identity],
    refs: Vec<ResolvedRef>,
) -> Result<Vec<(ResolvedRef, secret_value::Decoded)>> {
    refs.into_iter()
        .map(|r| {
            // Use the per-ref store_path so secrets found in non-active
            // stores decrypt correctly.
            let ref_ctx = Context {
                store: r.store_path.clone(),
                ..ctx.clone()
            };
            let decoded =
                super::get::get_decoded_with_identities(&ref_ctx, &r.secret_path, identities)?;
            super::get::warn_if_expired(&r.secret_path, &decoded);
            Ok((r, decoded))
        })
        .collect()
}

/// Apply the tag filter, derive env-var names, detect conflicts, and return
/// the final injection map. Pure on its inputs so unit tests can drive it
/// without touching the filesystem.
fn build_env_map(
    items: Vec<(ResolvedRef, secret_value::Decoded)>,
    want_tags: &[String],
) -> Result<BTreeMap<String, String>> {
    // Map keyed by env-var name → (value, source path). The source path is
    // kept so a collision message can name both offenders.
    let mut env_map: BTreeMap<String, (String, String)> = BTreeMap::new();

    for (r, decoded) in items {
        if !want_tags.is_empty()
            && !want_tags
                .iter()
                .all(|t| decoded.tags.iter().any(|d| d == t))
        {
            continue;
        }

        let key = pick_env_key(&r, &decoded)?;
        super::set::validate_env_key(&key).map_err(|e| {
            HimitsuError::InvalidReference(format!("{e} (from {:?})", r.secret_path))
        })?;

        if let Some((_, prev_path)) = env_map.get(&key) {
            return Err(HimitsuError::InvalidConfig(format!(
                "env-var {key:?} would be set by both {prev_path:?} and {:?}; \
                 rename one via `set --env-key` or an env-DSL alias",
                r.secret_path
            )));
        }

        let value = String::from_utf8(decoded.data).map_err(|e| {
            HimitsuError::InvalidReference(format!(
                "secret {:?} contains non-UTF-8 bytes — exec can only inject text values: {e}",
                r.secret_path
            ))
        })?;

        env_map.insert(key, (value, r.secret_path));
    }

    Ok(env_map.into_iter().map(|(k, (v, _))| (k, v)).collect())
}

/// Decide the env-var name for a resolved ref:
/// 1. explicit key from the env DSL,
/// 2. `SecretValue.env_key` set on the secret itself,
/// 3. `derive_env_key(last_segment_of_path)`.
fn pick_env_key(r: &ResolvedRef, decoded: &secret_value::Decoded) -> Result<String> {
    if let Some(k) = &r.explicit_key {
        return Ok(k.clone());
    }
    if !decoded.env_key.is_empty() {
        return Ok(decoded.env_key.clone());
    }
    let tail = config::env_dsl::last_component(&r.secret_path);
    if tail.is_empty() {
        return Err(HimitsuError::InvalidReference(format!(
            "secret path {:?} has no final segment to derive an env-var name from",
            r.secret_path
        )));
    }
    Ok(config::env_dsl::derive_env_key(tail))
}

/// Spawn the child with the resolved env, wait, and propagate its exit
/// status via `std::process::exit`. Does not return on the success path.
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
    use crate::crypto::secret_value::Decoded;

    fn decoded(data: &str, env_key: &str, tags: &[&str]) -> Decoded {
        Decoded {
            data: data.as_bytes().to_vec(),
            env_key: env_key.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn rref(path: &str, explicit: Option<&str>) -> ResolvedRef {
        ResolvedRef {
            secret_path: path.to_string(),
            store_path: PathBuf::new(),
            explicit_key: explicit.map(String::from),
        }
    }

    #[test]
    fn pick_env_key_priority_explicit_then_env_key_then_derive() {
        let k = pick_env_key(
            &rref("dev/whatever", Some("STRIPE")),
            &decoded("v", "IGNORED", &[]),
        )
        .unwrap();
        assert_eq!(k, "STRIPE");

        let k = pick_env_key(&rref("dev/whatever", None), &decoded("v", "API_TOKEN", &[])).unwrap();
        assert_eq!(k, "API_TOKEN");

        let k = pick_env_key(&rref("dev/api-key", None), &decoded("v", "", &[])).unwrap();
        assert_eq!(k, "API_KEY");

        let k = pick_env_key(&rref("dev/group/item-name", None), &decoded("v", "", &[])).unwrap();
        assert_eq!(k, "ITEM_NAME");
    }

    #[test]
    fn collect_env_leaves_pulls_every_leaf_with_its_parent_key() {
        let mut leaves = BTreeMap::new();
        leaves.insert(
            "API_KEY".to_string(),
            env_resolver::EnvNode::Leaf {
                secret_path: "dev/API_KEY".to_string(),
            },
        );
        leaves.insert(
            "DB".to_string(),
            env_resolver::EnvNode::Leaf {
                secret_path: "dev/DB_PASS".to_string(),
            },
        );
        let mut prefix = BTreeMap::new();
        prefix.insert("dev".to_string(), env_resolver::EnvNode::Branch(leaves));
        let tree = env_resolver::EnvNode::Branch(prefix);

        let mut got = collect_env_leaves(&tree);
        got.sort();
        assert_eq!(
            got,
            vec![
                ("API_KEY".to_string(), "dev/API_KEY".to_string()),
                ("DB".to_string(), "dev/DB_PASS".to_string()),
            ]
        );
    }

    #[test]
    fn build_env_map_filters_by_tag_and_picks_keys() {
        let items = vec![
            (rref("a/api-key", None), decoded("v1", "", &["pci"])),
            (rref("a/db", Some("DB_URL")), decoded("v2", "", &["pci"])),
            (rref("a/other", None), decoded("v3", "", &["mobile"])),
        ];
        let map = build_env_map(items, &["pci".to_string()]).unwrap();
        assert_eq!(map.get("API_KEY").map(String::as_str), Some("v1"));
        assert_eq!(map.get("DB_URL").map(String::as_str), Some("v2"));
        assert!(!map.contains_key("OTHER"));
    }

    #[test]
    fn build_env_map_empty_tag_filter_keeps_everything() {
        let items = vec![
            (rref("a/x", None), decoded("v1", "", &[])),
            (rref("a/y", None), decoded("v2", "", &["any"])),
        ];
        let map = build_env_map(items, &[]).unwrap();
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn build_env_map_collision_errors_with_both_paths() {
        // Two secrets resolving to the same env-var name.
        let items = vec![
            (rref("a/api-key", None), decoded("first", "", &[])),
            (
                rref("b/API_KEY", Some("API_KEY")),
                decoded("second", "", &[]),
            ),
        ];
        let err = build_env_map(items, &[]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("API_KEY"), "msg should name the key: {msg}");
        assert!(
            msg.contains("a/api-key") && msg.contains("b/API_KEY"),
            "msg should name both source paths: {msg}"
        );
    }

    #[test]
    fn build_env_map_rejects_invalid_posix_env_name() {
        // env_key override that violates POSIX env-name grammar.
        let items = vec![(rref("a/x", None), decoded("v", "1FOO", &[]))];
        let err = build_env_map(items, &[]).unwrap_err();
        assert!(err.to_string().contains("1FOO"));
    }

    #[test]
    fn build_env_map_rejects_non_utf8_value() {
        let mut d = decoded("", "", &[]);
        d.data = vec![0xff, 0xfe, 0xfd];
        let err = build_env_map(vec![(rref("a/x", None), d)], &[]).unwrap_err();
        assert!(err.to_string().contains("non-UTF-8"));
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
