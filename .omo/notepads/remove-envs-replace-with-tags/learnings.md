# Learnings — remove-envs-replace-with-tags

## 2026-05-28 Session Start

### Verified codebase facts
- Proto field numbers: SecretEntry.environment=2, SecretEnvelope.environment=3, StoreManifest.environments=3
- SecretValue.env_key=8 (per-secret OS env-var name, OUT OF SCOPE)
- env_cache.rs uses dedicated SQLite (NOT shared himitsu.db); has tables `envs` and `env_entries`
- envs_text.rs is a TextBuffer widget — rename not delete
- cli/alias.rs aliases STORES not envs — docstring fix only
- Config DSL files: env_dsl.rs(339), env_cache.rs(673), env_resolver.rs(843), envs_mut.rs(485)
- TUI env views: envs.rs(1977), envs_text.rs(273), envs_dsl_editor.rs(285)
- CLI consumers: exec.rs(469), generate.rs(309), codegen.rs(1240), alias.rs(85)

### Key guardrails
- DO NOT add reserved keyword yet (follow-up release)
- DO NOT delete old modules until transplant verified green
- DO NOT rename TUI files without updating ALL import sites simultaneously
- cargo clippy -D warnings must pass between EVERY wave
- Integration tests written BEFORE deleting corresponding env code paths

### Task 1 notes
- Marked three proto env fields deprecated with migration-cite comments; field numbers unchanged.
- Verified `SecretValue.env_key` remained untouched.

## env_cache path

- `EnvCache::open()` builds the SQLite cache path with `data_dir().join("envs.db")`.
- This is the dedicated env cache DB, separate from `himitsu.db`.

## Task 7 inventory

- Captured 10 H2 sections in `.omo/evidence/task-7-refs/inventory.md`.
- Verified 111 file:line citation bullets across EnvEntry, EnvNode, EnvCache, cache module surface, env_index, TUI env actions, proto env access, config module exports, YAML `envs:` keys, and extra CLI/TUI consumers.

## proto accessibility from integration tests

- `himitsu-cli` is binary-only in `Cargo.toml`, so integration tests cannot import `crate::proto` directly.
- The test crate can still reuse generated proto types by path-including `rust/src/proto/mod.rs` with `#[path = "../../rust/src/proto/mod.rs"] mod proto;`.
- That makes `proto::SecretEnvelope` and `proto::SecretValue` usable in fixtures without adding a library target.

## Task 10 output resolver transplant

- Legacy `env_resolver` wildcard `$1` capture substitutes only bare `$1` occurrences and leaves other numeric captures literal; the outputs resolver preserves that scanner for brace-expanded output names.
- Output brace-expansion is name-driven: `web-{dev,staging}` emits `web-dev` and `web-staging`, and each brace segment becomes the `$1` value for selectors and aliases in that expanded output.
- Cross-store output refs should split store and secret path at `#`, delegate store grammar to `SecretRef::parse_store_ref`, then parse the full ref with `SecretRef::parse` so canonical normalization stays centralized.

## Task 8 selector parser

- Existing glob matching lives in `rust/src/cli/export.rs` as `glob_match`; T8 reused that matcher for selector `Token::Glob` and extended it to support `?` segment wildcards without adding dependencies.

## Task 11 output mutation API

- `envs_mut.rs` exposes scope resolution plus upsert/delete/read over a lossy `serde_yaml` round-trip; comments and custom formatting are not preserved, while map ordering is deterministic through `BTreeMap`.
- `outputs/outputs_mut.rs` mirrors that style for `OutputsMap`: in-memory `add_output_entry` / `remove_output`, plus file-backed `upsert_output_entry` / `delete_output` / `read_outputs` using the same project/global scope resolution.
- `add_output_entry` is idempotent for identical name+definition calls and uses upsert semantics when the same output name receives a different `OutputDef`; `remove_output` is a no-op for absent names.

## Task 9 outputs DSL schema

- `config::outputs::dsl::OutputDef` is a fixed YAML map with `selectors: Vec<SelectorEntry>` and `aliases: BTreeMap<String, String>`.
- `selectors` and `aliases` default independently when omitted; explicit empty list/map are also valid.
- `#[serde(deny_unknown_fields)]` on `OutputDef` keeps the new `outputs:` schema strict while leaving brace expansion and `$1` substitution for resolution-time work.
- A small `OutputEntry` string-or-single-key-map serde shim coexists with `OutputDef` for one-entry migration/editor normalization without touching legacy `env_dsl.rs`.

## Task 12 env_cache verdict

- VERDICT: DROP `env_cache.rs` rather than transplant it to outputs.
- The cache mirrors user-authored `envs:` YAML definitions into `data_dir().join("envs.db")`; it does not cache decrypted values or the full secret store walk.
- Current production callers refresh the cache after env mutations, while readback is only evident in tests; TUI usage imports the `Scope` enum for labels/toasts rather than querying SQLite on a hot path.
- T19 should delete the dedicated cache file with `std::fs::remove_file(data_dir().join("envs.db"))`, treating missing files as harmless.

## Task 13 auto-fold legacy envelope env

- Current writes use the YAML envelope in `remote/store.rs`; legacy proto `SecretEnvelope` support belongs to `.age` fallback reads.
- The decode seam is `crypto::secret_value::decode_with_legacy_environment`, called after `age::decrypt_with_identities` and given `SecretPayload.legacy_environment` from `store::read_secret_payload`.
- Folding is read-only: valid legacy env values are appended to in-memory `Decoded.tags` if absent, invalid values warn and skip, and `himitsu get/search/ls/tag/export/generate/rekey` do not rewrite `.age` merely by reading.
- The T6 helper must remove the current `.yaml` file before writing its legacy `.age` fixture because the store reader prefers YAML over `.age` when both exist.

## Task 15 exec.rs dispatch pattern

- Replaced the three-branch `resolve_ref` dispatch (env-label → dead; glob → old; concrete path → old) with a single `Selector::parse(&args.r#ref)?` call.
- Dead env-label branch (using `load_effective_envs()` which returns empty map since T14), `collect_env_leaves`, `walk`, and `ResolvedRef` struct all removed.
- Pre-filter optimization: `is_path_candidate` checks path/glob tokens only before decrypting; tag tokens always pass (need decryption to verify).
- Post-decrypt: full `Selector::matches(&SecretMatch { path, tags })` applied on decoded values.
- `--tag` flags remain as additional AND filters on top of the selector (backward compat).
- `pick_env_key` simplified: `SecretValue.env_key` > derived from path tail — no outputs-block alias lookup.
- Unit tests updated: `match_all()` helper uses `Selector(vec![Group(vec![])])` (empty AND group → matches all via `Iterator::all` on empty).
- Integration tests cover: `tag:X`, `tag:A+tag:B`, `glob/*+tag:X`, bare glob, concrete path, `--tag` backward compat.
- Error for no-match: `HimitsuError::SecretNotFound` (T16 will handle exit-1 semantics).

## Task 14 Config struct migration

- `EnvEntry` enum + serde impls moved from `config/mod.rs` to `config/env_dsl.rs`; re-exported from `mod.rs` as `pub use self::env_dsl::{EnvEntry, validate_envs}` so all existing callers remain valid.
- `validate_envs` likewise moved to `env_dsl.rs` (where it logically belongs with `EnvEntry`) and re-exported.
- Hard-error on `envs:` key implemented via `#[serde(rename = "envs", default, deserialize_with = "reject_envs_field", skip_serializing)] _envs_deprecated: ()` on both `Config` and `ProjectConfig`; clippy `manual_non_exhaustive` suppressed with `#[allow]` on both structs since the field serves serde rejection not construction-prevention.
- `load_effective_envs()` now returns `Ok(BTreeMap::new())`; callers (exec, codegen) fall through to glob/path matching. T17/T18 will migrate them to outputs.
- `envs_mut.rs` updated to use `serde_yaml::Value` for raw YAML read/write of the `envs:` key, bypassing the `Config`/`ProjectConfig` struct rejection. This keeps `envs_mut` tests green while `Config::load()` still correctly rejects config files with `envs:`.
- `tui/views/search.rs::build_env_index` simplified to return empty map; will be retargeted in T20.
- Integration tests that called generate with `envs:` project configs updated to expect failure with "outputs" in stderr (generate now returns "no `outputs` defined..." since `load_effective_envs` is empty).
- Discovery tests (`project_config_discovers_*`) restructured to use `--project` + `init_git_repo` and verify "no `default_store` set" error (proves config was found in alternate location).

## Task 19 migrate envs command

- `himitsu migrate envs` is explicit-only and wires through the normal CLI dispatcher as a store-touching command; `--dry-run` is read-only and is excluded from mutation commits.
- Legacy `.age` proto envelopes are rewritten in-place atomically with `environment` cleared and the old environment folded into encrypted `SecretValue.tags`; legacy plaintext fixture payloads are accepted by falling back to the proto ciphertext bytes when age decrypt fails.
- `.himitsu.yaml` migration is raw `serde_yaml::Value` based so it can read legacy `envs:` even though typed config loading rejects that field; it writes `.himitsu.yaml.bak` before replacing `envs:` with `outputs:`.

## Task 17 generate.rs → outputs resolver

- `generate.rs` was completely rewritten: `--env` flag replaced by `--output`; `--env` now returns a hard error "–-env flag has been removed; use --output instead" before any config is loaded.
- Resolution goes through `config::outputs::resolver::resolve_outputs(&project_cfg.outputs, &ResolverContext { available_secrets })` where available_secrets is populated by `store::list_secrets` (paths only, no tags — tag resolution requires decryption which is too expensive for generate).
- `ResolvedEntry.env_key` replaces the old `last_component(path)` derivation; `ResolvedEntry.store_slug` replaces the old `SecretRef` qualified-ref parsing in `resolve_entries`.
- Output file naming is stable: `resolved_output.name` matches the old env name key, producing `pci-prod.sops.yaml` from `outputs.pci-prod`.
- Old `resolve_entries` function (which handled `EnvEntry::Alias/Single/Glob/Tag` variants) was removed; `Tag` entries are now handled naturally by the resolver if context has tags.
- TUI test fixtures in `tui/views/outputs.rs` and `tui/views/envs_dsl_editor.rs` used old `envs:` sequence format; these were updated to `outputs:` `OutputDef` format as part of this task since they blocked `cargo test --workspace`.
- The dedicated env cache deletion remains `ctx.state_dir.join("envs.db")`, matching T3 evidence and avoiding the unrelated `himitsu.db`.
