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
