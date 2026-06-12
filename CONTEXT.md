# Himitsu — Domain & Architecture Context

## Domain Terms

These are the concepts the codebase uses. When naming modules, prefer these terms.

- **Store** — a git repo containing encrypted secrets and metadata at `.himitsu/`
- **Remote** — a named store slug (`org/repo`) registered in the user's global config
- **Secret** — one encrypted value stored as `.himitsu/secrets/<path>.yaml`
- **Recipient** — an age public key that can decrypt secrets in a store
- **Output** — a named group of secrets defined in project config (`codegen:` block)
- **Selector** — a query over secrets: `tag:pci`, `prod/*`, `tag:A+tag:B`
- **Reference** — a string that identifies a secret: path, qualified ref (`github:org/repo/path`), or selector
- **Identity** — an age x25519 private key, loaded from disk or macOS Keychain

## Architecture Terms (from 2026-06-08 review)

These name the deepened modules introduced by the architecture review.

- **GitAdapter** — the seam for git operations. Production: `CliGitAdapter` (shells out). Tests: `InMemoryGitAdapter`. Absorbs the commit/push/pull orchestration that was previously inline in Context.
- **SecretStore** — the deepened `remote::store` module. A struct that owns the store root and resolves `recipients_path` once at construction. Narrow interface: `read`, `write`, `list`, `recipients`.
- **SecretResolver** — the deepened secret-resolution pipeline. One module owns the full path from reference string to decrypted `DecodedSecret`. Absorbs the duplicated ref→store→decrypt→decode pipeline that was spread across 5 CLI modules.

## Architecture Terms (from 2026-06-09 review)

- **Context · project config** — Context owns "which project config applies to this invocation": `ctx.project_config()`, lazy and memoized, selecting `--project` root over cwd walk internally. The raw loaders are private to `config`; `migrate`'s multi-root scan is the one sanctioned direct caller of the explicit-root loader. The legacy-`envs:` warning firing at most once per process is a property of this module, not of call-site discipline.
- **OutputResolver** — the deepened Output pipeline: project `codegen:` map → candidates with decrypted tags → selector/alias resolution → decoded entries, consumed by exec, generate, codegen (and later the TUI outputs view). `open(ctx)` performs the single decrypt-once scan (zero I/O when no outputs are defined); `env_map(label, tags)` is exec's tri-state strict surface (local-store only, tag-filter before env-key-conflict check, conflict = hard error); `decode(output)` follows cross-store entries and preserves duplicate keys for caller-side collapse. Materialization is channel-free — `DecodedEntry` carries expiry, callers choose where warnings render. Absorbs `resolver_candidates_with_tags`, exec's inline candidate loop, and `SecretResolver::resolve_candidates`.
- **KeyRegistry** — the deepened TUI keybinding module: one exhaustive-match row per `KeyAction` (a missing row is a compile error) owning the config-field accessor, help text, view scope, and palette link. `KeyMap::entries()`, view help screens (rendered from the *live* KeyMap, so rebinds show up), and palette shortcut display all derive from the registry; non-rebindable navigation rows (arrows/esc) stay static per view. The serde `KeyMap` struct remains the user-facing config format; the keybinding config key is `codegen` with serde aliases for the legacy `outputs` and `envs` names.
- **StoreOps** — the deepened mutation seam between presentation and the Store: one central module of silent mutation cores (set, delete, rekey, join, recipient add/rm, remote add). Each core owns the full side-effect chain — validate → encrypt → write → commit → completions-cache refresh — with no stdin/stdout. CLI commands are presentation wrappers; TUI views call the same cores, so the two fronts cannot drift. Generalizes the `add_core`/`rm_core` precedent from hm-by7.
- **PathFolding · ResultSort · StoreHealth** — three modules graduated from the Search view (`tui/model/path_folding.rs`, `tui/model/result_sort.rs`, `tui/widgets/store_health.rs`), following the autocomplete-widget precedent. PathFolding (store-bucket partitioning + collapse/expand of Secret paths to prefix groups) and ResultSort (column + direction ordering) are pure state modules — results in, rows out, no ratatui imports — unit-tested without a terminal. StoreHealth is a drawable widget owning health fetch + pill rendering; its project-config read goes through Context · project config. The Outputs view was evaluated for adoption and deliberately left out: its list is a one-line name sort, so sharing the column-sort machinery would have been a hypothetical seam.
