use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Args, CommandFactory};
use clap_complete::{generate, Shell};

use crate::error::Result;
use crate::remote::store;

/// Subcommands whose positional `<PATH>` argument should be completed with
/// secret paths from the active store rather than with files from the CWD.
///
/// Keep this list in sync with the subcommand names in [`super::Command`].
const SECRET_PATH_SUBCOMMANDS: &[&str] = &["get", "read", "set", "write", "ls", "rekey"];

/// Generate shell completion script and print it to stdout.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Target shell for completion output.
    pub shell: Option<Shell>,

    /// Rebuild the SQLite completions cache for all known stores.
    ///
    /// Run this after manually editing the store or after adding new stores.
    /// Mutations (set, write, import, etc.) automatically refresh the cache,
    /// so this flag is mainly useful for recovery or initial population.
    #[arg(long)]
    pub refresh_cache: bool,
}

pub fn run(args: CompletionsArgs, ctx: &super::Context) -> Result<()> {
    if args.refresh_cache {
        let stores_dir = ctx.stores_dir();
        match crate::completions_cache::refresh_all(&ctx.state_dir, &stores_dir) {
            Ok(n) => println!("Completions cache refreshed: {n} secret path(s) indexed"),
            Err(e) => eprintln!("warning: cache refresh failed: {e}"),
        }
        if !ctx.store.as_os_str().is_empty() {
            let _ = crate::completions_cache::refresh_store(&ctx.state_dir, &ctx.store);
        }
        return Ok(());
    }

    let shell = args.shell.ok_or_else(|| {
        crate::error::HimitsuError::NotSupported(
            "shell argument required (e.g. `himitsu completions bash`), \
             or pass --refresh-cache to rebuild the completions cache"
                .into(),
        )
    })?;

    let mut cmd = super::Cli::command();
    let mut buf: Vec<u8> = Vec::new();
    generate(shell, &mut cmd, "himitsu", &mut buf);
    let script = String::from_utf8(buf).expect("clap_complete emits valid UTF-8");
    let patched = patch_script(shell, &script);
    io::stdout().write_all(patched.as_bytes())?;
    Ok(())
}

/// Hidden helper subcommand: print newline-separated secret paths from the
/// active store. Used by shell completion scripts to offer dynamic candidates
/// for the `<PATH>` positional on `get`, `read`, `set`, etc.
///
/// This must be fast (< ~100ms) and must never fail: if no store resolves,
/// we print nothing and exit 0 so the shell just shows "no matches".
#[derive(Debug, Args)]
pub struct CompletePathsArgs {
    /// Optional prefix filter. Only secret paths that start with this string
    /// are emitted. Empty string matches everything.
    #[arg(default_value = "")]
    pub prefix: String,
}

pub fn run_complete_paths(args: CompletePathsArgs, ctx: &super::Context) -> Result<()> {
    let stores = resolve_completion_stores(ctx);

    // Fast path: serve from the SQLite cache when warm.
    if crate::completions_cache::is_warm(&ctx.state_dir, &stores) {
        if let Ok(paths) =
            crate::completions_cache::lookup(&ctx.state_dir, &stores, &args.prefix)
        {
            let mut out = io::stdout().lock();
            for p in paths {
                let _ = writeln!(out, "{p}");
            }
            return Ok(());
        }
    }

    // Slow path fallback: live filesystem scan (cache absent, empty, or corrupt).
    let mut out = io::stdout().lock();
    for store_path in stores {
        let Ok(paths) = store::list_secrets(&store_path, None) else {
            continue;
        };
        for p in paths {
            if args.prefix.is_empty() || p.starts_with(&args.prefix) {
                let _ = writeln!(out, "{p}");
            }
        }
    }
    Ok(())
}

/// Collect every store path we should search for completion candidates.
///
/// Order of preference:
///   1. The resolved `ctx.store` if one was provided (via -s/-r/config).
///   2. All store checkouts under `ctx.stores_dir()` (so completion works
///      even when no default store is configured).
///
/// Never errors: a missing stores dir just yields an empty list.
fn resolve_completion_stores(ctx: &super::Context) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !ctx.store.as_os_str().is_empty() && ctx.store.exists() {
        out.push(ctx.store.clone());
    }

    let stores_dir = ctx.stores_dir();
    let Ok(entries) = std::fs::read_dir(&stores_dir) else {
        return out;
    };
    for org in entries.flatten() {
        let Ok(ft) = org.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let Ok(repos) = std::fs::read_dir(org.path()) else {
            continue;
        };
        for repo in repos.flatten() {
            let Ok(rft) = repo.file_type() else { continue };
            if !rft.is_dir() {
                continue;
            }
            let path = repo.path();
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

// ── Shell-script post-processing ─────────────────────────────────────────────
//
// clap_complete's static generator has no hook for per-arg dynamic completion,
// so we patch the generated script in-place to route the `<PATH>` positional
// on each `SECRET_PATH_SUBCOMMANDS` entry through a shell helper that calls
// `himitsu __complete-paths <prefix>`.

fn patch_script(shell: Shell, script: &str) -> String {
    match shell {
        Shell::Bash => patch_bash(script),
        Shell::Zsh => patch_zsh(script),
        Shell::Fish => patch_fish(script),
        // PowerShell / Elvish are left untouched — users of those shells still
        // get the default (file) completion they had before. This is not a
        // regression, just an absent improvement.
        _ => script.to_string(),
    }
}

/// For bash we inject a helper function `_himitsu_complete_paths` and, for
/// each target subcommand case block, replace the terminal fallback
/// `COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )` with a call to the
/// helper, but only when the current word is not a flag.
fn patch_bash(script: &str) -> String {
    let mut patched = String::with_capacity(script.len() + 1024);
    patched.push_str(BASH_HELPER);
    patched.push('\n');
    patched.push_str(script);

    for sub in SECRET_PATH_SUBCOMMANDS {
        let marker = format!("himitsu__subcmd__{sub})\n");
        let Some(block_start) = patched.find(&marker) else {
            continue;
        };
        // A subcommand case block ends at the *next* `himitsu__subcmd__`
        // line — that's the opening of the following arm.
        let search_after = block_start + marker.len();
        let block_end = patched[search_after..]
            .find("        himitsu__subcmd__")
            .map(|rel| search_after + rel)
            .unwrap_or(patched.len());
        let block = &patched[block_start..block_end];
        let needle = "COMPREPLY=()\n                    ;;";
        if let Some(rel) = block.find(needle) {
            let abs = block_start + rel;
            let replacement =
                "COMPREPLY=( $(compgen -W \"$(himitsu __complete-paths \"${cur}\" 2>/dev/null)\" -- \"${cur}\") )\n                    return 0\n                    ;;";
            patched.replace_range(abs..abs + needle.len(), replacement);
        }
    }
    patched
}

const BASH_HELPER: &str =
    "# himitsu: dynamic completion helper\n# (injected by `himitsu completions bash`)";

/// For zsh we replace the `_default` action on the `path` positional with
/// our custom `_himitsu_secrets` function and prepend its definition.
fn patch_zsh(script: &str) -> String {
    let mut patched = String::with_capacity(script.len() + 512);
    patched.push_str(ZSH_HELPER);
    patched.push('\n');
    patched.push_str(script);

    // Match every line of the form
    //   ':path -- ... :_default' \
    //   '::path -- ... :_default' \
    // and swap `_default` for `_himitsu_secrets`. Descriptions are distinctive
    // enough that we can scope by the `path --` prefix.
    let mut out = String::with_capacity(patched.len());
    for line in patched.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let is_path_positional = (trimmed.starts_with("':path -- ")
            || trimmed.starts_with("'::path -- "))
            && line.trim_end().ends_with(":_default' \\");
        if is_path_positional {
            out.push_str(&line.replace(":_default'", ":_himitsu_secrets'"));
        } else {
            out.push_str(line);
        }
    }
    out
}

const ZSH_HELPER: &str = r#"# himitsu: dynamic completion helper
# (injected by `himitsu completions zsh`)
_himitsu_secrets() {
    local -a secrets
    secrets=(${(f)"$(himitsu __complete-paths "${words[CURRENT]}" 2>/dev/null)"})
    if (( ${#secrets} )); then
        compadd -a secrets
    else
        _default
    fi
}"#;

/// Fish already uses `complete -f` (no file completion) for clap-generated
/// scripts when ValueHint is unset, but it also has no built-in dynamic hook
/// that we can swap without touching every line. For each target subcommand
/// we append a `complete -c himitsu -n "__fish_seen_subcommand_from <sub>" -f
/// -a "(himitsu __complete-paths)"` directive at the end of the script.
fn patch_fish(script: &str) -> String {
    let mut out = String::with_capacity(script.len() + 512);
    out.push_str(script);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("# himitsu: dynamic completion for secret-path positionals\n");
    for sub in SECRET_PATH_SUBCOMMANDS {
        out.push_str(&format!(
            "complete -c himitsu -n \"__fish_seen_subcommand_from {sub}\" -f -a \"(himitsu __complete-paths 2>/dev/null)\"\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_complete::Shell;

    fn generate_for(shell: Shell) -> String {
        let mut cmd = super::super::Cli::command();
        let mut buf: Vec<u8> = Vec::new();
        generate(shell, &mut cmd, "himitsu", &mut buf);
        let script = String::from_utf8(buf).unwrap();
        patch_script(shell, &script)
    }

    #[test]
    fn bash_completions_are_generated() {
        let text = generate_for(Shell::Bash);
        assert!(!text.is_empty());
        assert!(text.contains("himitsu"));
    }

    #[test]
    fn bash_completions_route_path_positional_through_helper() {
        let text = generate_for(Shell::Bash);
        // The `get` subcommand's positional should now invoke __complete-paths.
        // Isolate the whole `himitsu__subcmd__get)` arm — from its opening to
        // the next `himitsu__subcmd__` line.
        let marker = "himitsu__subcmd__get)\n";
        let start = text
            .find(marker)
            .expect("get subcommand arm present in generated bash script");
        let rest = &text[start + marker.len()..];
        let end = rest.find("        himitsu__subcmd__").unwrap_or(rest.len());
        let get_block = &rest[..end];
        assert!(
            get_block.contains("himitsu __complete-paths"),
            "expected get block to call __complete-paths, got:\n{get_block}"
        );
    }

    #[test]
    fn zsh_completions_route_path_positional_through_helper() {
        let text = generate_for(Shell::Zsh);
        assert!(text.contains("_himitsu_secrets"), "helper missing");
        // Every `:path --` positional in a target subcommand should reference
        // the helper. We only assert on `get` and `read` since those are the
        // canonical cases called out by the bug.
        assert!(
            text.contains(":_himitsu_secrets'"),
            "expected :path -- ... :_himitsu_secrets', got:\n{text}"
        );
        // And the original _default for those path args should be gone from
        // the `path --` lines (STORE/REMOTE still use _default, which is fine).
        let path_lines_with_default: Vec<&str> = text
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                (t.starts_with("':path -- ") || t.starts_with("'::path -- "))
                    && l.trim_end().ends_with(":_default' \\")
            })
            .collect();
        assert!(
            path_lines_with_default.is_empty(),
            "still-default path positionals: {path_lines_with_default:?}"
        );
    }

    #[test]
    fn fish_completions_append_dynamic_directives() {
        let text = generate_for(Shell::Fish);
        for sub in SECRET_PATH_SUBCOMMANDS {
            let expected = format!("__fish_seen_subcommand_from {sub}");
            assert!(
                text.contains(&expected),
                "fish: missing directive for `{sub}`"
            );
        }
        assert!(text.contains("himitsu __complete-paths"));
    }
}
