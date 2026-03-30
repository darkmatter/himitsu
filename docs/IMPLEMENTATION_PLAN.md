# Himitsu Implementation Plan (Detailed)

This document defines an execution plan for the vNext architecture:

- XDG-based store layout (`~/.local/share/himitsu/`, `~/.local/state/himitsu/`)
- `age`-only secret model (`.himitsu/secrets/<env>/<KEY>.age`)
- transport-agnostic sharing protocol
- GitHub PR inbox + Nostr send/receive
- full Rust rewrite of current shell implementation

---

## Phase 0 - Freeze and Baseline

- [x] **Phase 0 complete**

### Goals

- [x] Freeze docs and expected command behavior.
- [x] Capture baseline tests and golden fixtures before rewrite.

### Deliverables

- [x] Finalized docs in `docs/`.
- [x] Snapshot fixtures for representative repositories.
- [x] Legacy command behavior matrix.

### Modules / files

No Rust modules. Fixture and doc work only.

```
tests/
├── fixtures/
│   ├── golden/
│   │   ├── init-output.txt
│   │   ├── set-get-roundtrip.txt
│   │   ├── ls-output.txt
│   │   ├── recipient-add-self.txt
│   │   └── group-lifecycle.txt
│   ├── configs/
│   │   ├── minimal-himitsu.yaml
│   │   ├── full-himitsu.yaml
│   │   └── project-binding.yaml
│   └── remotes/
│       ├── single-env/           # Minimal remote layout (one env)
│       └── multi-env/            # Full remote layout (common/dev/prod)
docs/
└── COMMAND_MATRIX.md             # Shell command → expected behavior mapping
```

### Test cases

Run existing bats suite and capture outputs:

```bash
bats tests/bats/              # Run all existing shell tests
```

- [x] `init` creates expected directory tree and files
- [x] `set` / `get` roundtrip returns correct value without writing plaintext-at-rest files
- [x] `recipient add --self` writes correct key file
- [x] `group add` / `group rm` updates data.json
- [x] encryption flows are lossless while preserving no-plaintext-at-rest guarantees
- [x] `ls` output format captured as golden fixture

### Acceptance Criteria

- [ ] Team agrees on command compatibility goals.
- [x] Golden fixtures can be replayed against new implementation.

### Risks

- Hidden assumptions in shell scripts.
- Incomplete fixture coverage.
- Legacy shell + current `sops` version incompatibility on encrypt/decrypt/set paths.

---

## Phase 1 - Rust Project Scaffold

- [x] **Phase 1 complete**

### Goals

- [x] Create Rust crate and executable `himitsu`.
- [x] Implement command parsing and logging framework.

### Modules / files

```
Cargo.toml                        # Single crate, all dependencies
rust/src/
├── main.rs                       # Entry point, tracing init, clap dispatch
├── error.rs                      # HimitsuError enum (thiserror)
└── cli/
    ├── mod.rs                    # Top-level Cli enum + subcommand dispatch
    ├── init.rs                   # Stub
    ├── set.rs                    # Stub
    ├── get.rs                    # Stub
    ├── ls.rs                     # Stub
    ├── encrypt.rs                # Stub
    ├── decrypt.rs                # Stub
    ├── sync.rs                   # Stub
    ├── search.rs                 # Stub
    ├── recipient.rs              # Stub
    ├── group.rs                  # Stub
    ├── remote.rs                 # Stub
    ├── share.rs                  # Stub
    ├── inbox.rs                  # Stub
    ├── schema.rs                 # Stub
    ├── codegen.rs                # Stub
    └── import.rs                 # Stub
flake.nix                         # Updated: build Rust binary alongside shell
.github/workflows/rust.yml        # CI: fmt, clippy, test on macOS + Linux
```

### Test cases

```bash
cargo test                        # All unit tests
cargo clippy -- -D warnings       # Lint
cargo fmt -- --check              # Format check
```

- [x] `himitsu --help` prints full command tree with all subcommands
- [x] `himitsu --version` prints version string
- [x] `himitsu <subcommand> --help` works for every subcommand stub
- [x] Binary builds on macOS (aarch64-apple-darwin)
- [x] Binary builds on Linux (x86_64-unknown-linux-gnu)
- [ ] `nix build` produces both shell and Rust binaries

### Acceptance Criteria

- [x] `himitsu --help` works with planned command tree.
- [x] Project builds on macOS and Linux.

### Risks

- Slow startup due to heavy crate selection.
- Over-designing module boundaries too early.

---

## Phase 2 - Core Runtime Parity (No Sharing Yet)

- [ ] **Phase 2 complete**

### Goals

- [ ] Implement config loading and mode detection.
- [ ] Implement remote resolution and local secret operations.
- [ ] Add optional macOS Keychain storage for generated age private keys.
- [ ] Add `SOPS_AGE_KEY_CMD` key resolution that checks Keychain first, then file fallback.

### Modules / files

```
rust/src/
├── config/
│   ├── mod.rs                    # detect_mode() → ProjectMode | UserMode
│   ├── global.rs                 # GlobalConfig: parse ~/.himitsu/config.yaml
│   ├── project.rs                # ProjectConfig: parse <repo>/.himitsu.yaml
│   └── remote.rs                 # RemoteConfig: parse remote himitsu.yaml
├── keyring/
│   ├── mod.rs                    # KeyProvider trait + scope/fingerprint mapping
│   └── macos.rs                  # macOS Keychain adapter via `security` CLI
├── remote/
│   ├── mod.rs                    # Remote discovery, resolution, list known remotes
│   └── store.rs                  # Secret file I/O: read/write .himitsu/secrets/<env>/<KEY>.age
├── git.rs                        # Git CLI wrapper: clone, commit, push, pull, status
├── crypto/
│   ├── mod.rs                    # Trait defs: Encryptor, Decryptor
│   └── age.rs                    # age crate: keygen, encrypt, decrypt, parse recipients
├── index/
│   ├── mod.rs                    # SecretIndex: open db, query, upsert
│   └── schema.sql                # CREATE TABLE statements (embedded via include_str!)
├── cli/
│   ├── init.rs                   # Full implementation
│   ├── set.rs                    # Full implementation
│   ├── get.rs                    # Full implementation
│   ├── ls.rs                     # Full implementation
│   ├── encrypt.rs                # Full implementation
│   ├── decrypt.rs                # Full implementation
│   ├── search.rs                 # Full implementation
│   ├── recipient.rs              # Full implementation
│   ├── group.rs                  # Full implementation
│   └── remote.rs                 # Full implementation (add/push/pull/status)
src/lib/
├── common.sh                     # `SOPS_AGE_KEY_CMD` keychain-first lookup helper
└── init.sh                       # Optional keychain save when generating age key
tests/
├── integration/
│   ├── init_test.rs              # CLI integration tests for init
│   ├── set_get_test.rs           # set → get roundtrip
│   ├── ls_test.rs                # ls output format
│   ├── encrypt_decrypt_test.rs   # encrypt → decrypt roundtrip
│   ├── recipient_test.rs         # recipient add/rm/ls
│   ├── group_test.rs             # group add/rm/ls
│   ├── remote_test.rs            # remote add/push/pull/status
│   └── search_test.rs            # cross-remote search
└── fixtures/                     # (from Phase 0)
```

### Keychain indexing (when enabled)

- Scope pointer item: `service=io.darkmatter.himitsu.agekey.scope.v1`, `account=gh:<org>:<repo>:<group>` with value `<fingerprint>`.
- Key material item: `service=io.darkmatter.himitsu.agekey.byfp.v1`, `account=<fingerprint>` with value `AGE-SECRET-KEY-...`.
- Resolution order for `SOPS_AGE_KEY_CMD`: scope pointer → fingerprint key item → `SOPS_AGE_KEY_FILE` fallback.
- Scope values are normalized (`org`/`repo` lowercase, `group` escaped) to ensure unique matching across all org/repo/group combos.

### Test cases

```bash
cargo test                        # Unit + integration
cargo test --test '*'             # Integration tests only
```

#### Unit tests (inline `#[cfg(test)]`)

- [ ] `config::detect_mode` returns `ProjectMode` when `.git` + `.himitsu.yaml` exist
- [ ] `config::detect_mode` returns `UserMode` when `.git` exists without `.himitsu.yaml`
- [ ] `config::detect_mode` returns `UserMode` when no `.git` found
- [x] `config::Config::parse` loads valid unified `.himitsu.yaml`
- [x] `config::Config::parse` rejects malformed YAML with clear error
- [x] `config::Config::parse` reads `identity.public_keys`
- [x] `config::Config::parse` loads `policies` with `path_pattern`, `include`, `exclude`
- [x] `config::Config::parse` loads `imports` (`type`, `ref`, `path`)
- [x] `config::Config::parse` loads optional `codegen` (`lang`, `path`)
- [x] `crypto::age::keygen` produces valid x25519 keypair
- [x] `crypto::age::encrypt` → `decrypt` roundtrip preserves plaintext
- [x] `crypto::age::encrypt` with multiple recipients succeeds
- [x] `crypto::age::decrypt` with wrong key fails with clear error
- [x] `keyring::scope::account_for` normalizes `org/repo/group` and yields deterministic account ids
- [x] `keyring::scope::account_for` avoids collisions across similar org/repo/group combos
- [x] `keyring::mapping::scope_to_fingerprint` stores and reads pointer values correctly
- [x] `keyring::mapping::scope_to_fingerprint` updates cleanly on key rotation
- [ ] `keyring::macos::store_private_key` and `load_private_key` roundtrip via mocked `security` CLI
- [x] `crypto::age::resolve_private_key` prefers keychain when enabled and falls back to file key
- [x] `remote::store::write_secret` creates `.himitsu/secrets/<env>/<KEY>.age`
- [x] `remote::store::read_secret` reads and decrypts `.age` file
- [x] `remote::store::list_secrets` returns all keys for an env
- [x] `remote::store::list_secrets` handles nested subdirectories
- [x] `git::run` executes git commands and captures output
- [x] `git::run` returns error for non-zero exit codes
- [x] `index::SecretIndex::upsert` inserts new entry
- [x] `index::SecretIndex::upsert` updates existing entry (same remote+path)
- [x] `index::SecretIndex::search` matches partial key names
- [x] `index::SecretIndex::search` returns results across multiple remotes

#### Integration tests (`tests/integration/`)

- [x] `init` creates `~/.himitsu/` with keys/, `.himitsu.yaml`, state/
- [x] `init` is idempotent (running twice doesn't error or overwrite keys)
- [x] `init` wizard output shows checkmarks and public key on first run
- [x] `init` shows "Already initialized." with public key on subsequent runs
- [x] `init --name <org/repo>` registers a named store and sets it as default
- [x] First-use auto-initialization (no prompt — runs silently on first command)
- [x] Lazy store cloning via `ensure_store`: `--remote <slug>` triggers clone if not present
- [x] Project-level config (`himitsu.yaml`) discovery by walking CWD upward
- [x] Project-level config also discovered at `.config/himitsu.yaml` and `.himitsu/config.yaml` (incl. `.yml` variants)
- [x] `ProjectConfig` expanded with `envs`, `generate`, and `store` sections
- [x] `EnvEntry` supports three YAML shapes: scalar string (Single/Glob with `/*`), single-key map (Alias)
- [x] `load_project_config()` convenience function combining discovery + deserialization
- [x] `generate --stdout --env <env>` decrypts and outputs YAML for an env definition
- [x] `generate --stdout` (no `--env`) generates all envs from project config
- [x] `generate` resolves glob entries (`dev/*`) to all matching secret paths
- [x] `generate` resolves alias entries (`MY_KEY: dev/DB_PASSWORD`) to aliased output key
- [x] `generate` errors clearly when no project config found
- [x] `generate` errors clearly when referenced env not defined in config
- [x] `resolve_store` canonical ordering: remote_override → project config → global config → implicit single → error
- [ ] `init` with keychain enabled stores generated private key in Keychain
- [ ] keychain scope pointer is unique for every `<org>/<repo>/<group>` combination
- [ ] `SOPS_AGE_KEY_CMD` resolves keychain key for scope before checking `SOPS_AGE_KEY_FILE`
- [ ] `SOPS_AGE_KEY_CMD` falls back to file-based key when keychain item is missing
- [x] `set prod/API_KEY "secret"` creates `vars/prod/API_KEY.age` (path-based syntax)
- [x] `get prod/API_KEY` returns `"secret"` after set
- [x] `set` then `get` with multiline values preserves newlines
- [x] `set` then `get` with special characters (quotes, backslashes, unicode)
- [x] `ls` with no args lists all envs
- [x] `ls prod` lists keys in prod env
- [x] `rekey` re-encrypts all secrets for current recipients (`encrypt` is a deprecated hidden alias)
- [x] `decrypt` is not implemented / errors (no plaintext at rest)
- [x] `recipient add --self --group team` writes pubkey file to recipients/team/
- [x] `recipient add` with explicit `--age-key` writes correct .pub file
- [x] `recipient rm` removes the key file
- [x] `recipient show <name>` prints the public key for a specific recipient
- [x] `recipient ls` shows all recipients, optionally filtered by group
- [x] `group add mygroup` creates directory + updates data.json
- [x] `group rm mygroup` removes directory + updates data.json
- [x] `group rm common` is rejected (reserved)
- [x] `group ls` lists groups with recipient counts
- [x] `remote add <org/repo>` clones repo into stores_dir
- [x] `remote default` gets/sets default store
- [x] `remote list` shows all known stores
- [x] `remote remove` deletes store checkout
- [x] `search <query>` matches key names across remotes
- [x] `search` with no matches returns empty output, exit 0
- [x] Provider-prefixed qualified references (`github:org/repo/path`) parsed by `SecretRef`
- [x] `get github:org/repo/path` retrieves secret from named store without `--remote` flag
- [x] `set github:org/repo/path val` writes secret to named store, collecting recipients from that store
- [x] `ls github:org/repo` lists all secrets in named store
- [x] `ls github:org/repo/prefix` lists secrets under prefix in named store
- [x] `StoreConfig.recipients_path` wired through `Context` and all recipient-touching code paths
- [x] `check` verifies store checkouts are up to date with their remotes (exit 1 when behind)
- [x] Store-internal `.himitsu/config.yaml` `recipients_path` overrides default recipients directory
- [x] `set` / `rekey` / `recipient add|rm|show|ls` / `group` all respect custom `recipients_path`
- [ ] Golden fixture parity: outputs match captured shell fixtures

### Acceptance Criteria

- [x] Core local commands produce expected filesystem results.
- [x] Equivalent flows succeed on baseline fixtures.
- [ ] `himitsu search` returns results across multiple remotes.
- [ ] Keychain mode stores generated age keys and decrypts via `SOPS_AGE_KEY_CMD` without plaintext key files required.
- [ ] Key lookup remains uniquely addressable for all `<org>/<repo>/<group>` scopes.

### Risks

- Edge cases with path expansion and symlinked directories.
- Value quoting/newline handling in `set`.
- Keychain access prompts/ACL behavior may break CI or non-interactive sessions.
- Scope normalization bugs could cause key lookup misses.

---

## Phase 3 - Recipient Policy Engine

- [ ] **Phase 3 complete**

### Goals

- [ ] Replace env-only recipient model with path policy resolution.
- [ ] Support local groups and remote refs in one normalized pipeline.

### Modules / files

```
rust/src/
├── policy/
│   ├── mod.rs                    # PolicyEngine: load policies, resolve for path
│   └── resolver.rs               # RecipientRef parsing, expansion, dedup, ordering
```

### Test cases

```bash
cargo test policy                 # Run policy module tests only
```

- [ ] Longest `path_prefix` wins: `vars/prod/` beats `vars/`
- [ ] `include: [group:admins]` expands to all keys in `recipients/admins/`
- [ ] `exclude: [group:contractors]` removes matching recipients from result
- [ ] `include` + `exclude` on same policy: exclude takes precedence
- [ ] Multiple policies: each path resolves to correct recipient set
- [ ] No matching policy: falls back to all recipients (or error, TBD)
- [ ] `remote:github:org/keys#team=security` parses correctly
- [ ] `email:user@domain.com` parses correctly
- [ ] `ens:name.eth` parses correctly
- [ ] `nostr:npub1...` parses correctly
- [ ] Duplicate recipients across groups are deduplicated
- [ ] Recipient ordering is deterministic regardless of input order
- [ ] Empty groups produce no recipients (not an error)

### Acceptance Criteria

- [ ] Recipient resolution snapshots are deterministic.
- [ ] Policy tests cover include/exclude precedence.

### Risks

- Policy complexity creep.
- Ambiguous behavior at path boundaries.

---

## Phase 4 - Protocol + GitHub PR Inbox

- [ ] **Phase 4 complete**

### Goals

- [ ] Implement signed envelope protocol.
- [ ] Ship first complete external sharing path via GitHub PR inbox.

### Modules / files

```
rust/src/
├── crypto/
│   └── signing.rs                # Ed25519 keygen, sign, verify
├── protocol/
│   ├── mod.rs                    # Shared types, canonical JSON helpers
│   ├── envelope.rs               # Envelope struct, serde, sign/verify methods
│   ├── payload.rs                # Payload struct, serde
│   └── profile.rs                # Profile struct, serde
├── transport/
│   ├── mod.rs                    # Transport trait: send, list, fetch
│   └── github_pr.rs              # GitHub PR inbox: create PR, list inbox files
├── inbox/
│   ├── mod.rs                    # Accept/reject pipeline orchestration
│   └── replay.rs                 # Replay DB: SQLite envelope-id tracking
├── cli/
│   ├── share.rs                  # Full implementation
│   └── inbox.rs                  # Full implementation
```

### Test cases

```bash
cargo test protocol               # Protocol struct tests
cargo test transport              # Transport adapter tests
cargo test inbox                  # Inbox pipeline tests
```

#### Unit tests

- [ ] `Envelope::new` creates valid envelope with UUIDv7 id
- [ ] `Envelope::sign` produces Ed25519 signature over JCS-canonicalized body
- [ ] `Envelope::verify` succeeds with correct key
- [ ] `Envelope::verify` fails with wrong key
- [ ] `Envelope::verify` fails if any field is tampered after signing
- [ ] JCS canonicalization is deterministic (same input → same bytes)
- [ ] JCS canonicalization handles unicode, nested objects, arrays
- [ ] `Payload` serializes/deserializes with secrets array
- [ ] `Payload` supports both utf8 and base64 encoding
- [ ] `Profile` round-trips through JSON
- [ ] `Profile` rejects missing required fields (ref, age_recipients)
- [ ] `replay::ReplayDb::record` inserts envelope id
- [ ] `replay::ReplayDb::is_seen` returns true for recorded id
- [ ] `replay::ReplayDb::is_seen` returns false for unseen id

#### Integration tests

- [ ] Full send flow: `share send --to github:org/repo --path ... --value ...`
  creates PR with envelope JSON (mocked GitHub API via wiremock)
- [ ] Envelope JSON in PR body is valid and parseable
- [ ] Full receive flow: `inbox list` shows pending envelopes (mocked)
- [ ] `inbox accept <id>` verifies signature, decrypts, writes `.age` file
- [ ] `inbox accept` with duplicate envelope id is rejected
- [ ] `inbox accept` with expired envelope is rejected
- [ ] `inbox reject <id>` records envelope as processed without writing secret

### Acceptance Criteria

- [ ] Sender can share to external inbox repo via PR.
- [ ] Receiver can verify, decrypt, and apply encrypted output.
- [ ] Duplicate envelope IDs are rejected.

### Risks

- GitHub auth/token scope complexity.
- Canonicalization bugs causing signature mismatch.

---

## Phase 5 - Nostr Send and Receive

- [ ] **Phase 5 complete**

### Goals

- [ ] Add full Nostr roundtrip delivery.

### Modules / files

```
rust/src/
├── transport/
│   └── nostr.rs                  # Nostr relay adapter: publish, subscribe, parse
```

### Test cases

```bash
cargo test transport::nostr       # Nostr transport tests
```

- [ ] `NostrTransport::send` publishes kind 30420 event with correct tags
- [ ] Event content is valid JSON envelope
- [ ] `p` tag contains recipient npub hex
- [ ] `d` tag contains envelope id
- [ ] `t` tag is `himitsu-envelope`
- [ ] `expiration` tag mirrors `meta.expires_at` when present
- [ ] `NostrTransport::list` subscribes and returns matching envelopes
- [ ] Received events are parsed into valid `Envelope` structs
- [ ] HSP signature is verified (not just Nostr event signature)
- [ ] Full roundtrip: send via nostr → list → accept (requires local relay)

### Acceptance Criteria

- [ ] `share send --to nostr:...` publishes valid event.
- [ ] `inbox list --transport nostr` returns envelopes.
- [ ] `inbox accept` succeeds end-to-end.

### Risks

- Relay reliability and event propagation latency.
- Metadata inconsistencies between relays.

---

## Phase 6 - Identity Resolvers (Email + ENS + Nostr)

- [ ] **Phase 6 complete**

### Goals

- [ ] Implement external identity resolution beyond GitHub.

### Modules / files

```
rust/src/
├── identity/
│   ├── mod.rs                    # Resolver trait, ResolvedProfile, cache layer
│   ├── github.rs                 # GitHub keys repo: fetch team keys, parse .pub files
│   ├── email.rs                  # HTTP fetch /.well-known/himitsu.json, parse profile
│   ├── ens.rs                    # ENS text record lookup (feature-gated)
│   └── nostr.rs                  # npub normalization, optional profile metadata
```

### Test cases

```bash
cargo test identity               # All resolver tests
```

- [ ] `GithubResolver::resolve` fetches keys from `org/keys` repo structure
- [ ] `GithubResolver::resolve` correctly parses `#team=<name>` fragment
- [ ] `EmailResolver::resolve` fetches `https://domain/.well-known/himitsu.json`
- [ ] `EmailResolver::resolve` parses profile with age_recipients and inbox
- [ ] `EmailResolver::resolve` returns error for 404 / malformed JSON
- [ ] `EnsResolver::resolve` reads `himitsu_public_key` text record
- [ ] `EnsResolver::resolve` reads `himitsu_inbox` text record
- [ ] `NostrResolver::resolve` normalizes npub to hex pubkey
- [ ] Cache layer stores resolved profiles in `~/.himitsu/cache/remote-identities/`
- [ ] Cache hit returns stored profile without network call
- [ ] Cache miss triggers network fetch and stores result
- [ ] Lockfile pinning: resolver verifies fingerprints against `sources.lock.json`
- [ ] Lockfile mismatch produces warning/error (not silent substitution)

All network tests use `wiremock` for HTTP mocking. ENS tests mock the RPC
endpoint.

### Acceptance Criteria

- [ ] `share send --to email:...` resolves profile and sends.
- [ ] `share send --to ens:...` resolves profile and sends.
- [ ] Resolver output is cached and reproducible.

### Risks

- Resolver trust and TOFU pitfalls.
- Network timeout behavior degrading CLI UX.

---

## Phase 7 - Schema and Developer UX

- [ ] **Phase 7 complete**

### Goals

- [ ] Add schema-backed validation and autocomplete support.

### Modules / files

```
proto/
├── config.proto                  # Canonical unified config schema (identity, policies, imports, codegen)
├── secrets.proto                 # Canonical secrets schema (envelope, share, sync)
rust/src/
├── proto/
│   └── mod.rs                    # Generated types, conversions, JSON Schema helpers
├── cli/
│   └── schema.rs                 # Full implementation (dump, dump-all, refresh, list)
build.rs                          # prost compile .proto → Rust types with serde derives
```

### Test cases

```bash
cargo test schema                 # Schema generation tests
cargo test proto                  # Proto round-trip and JSON Schema tests
```

- [x] Static schema is valid JSON Schema draft 2020-12
- [x] Static schema validates a correct `himitsu.yaml`
- [x] Static schema rejects missing required fields
- [ ] Dynamic schema includes local group names as enum values
- [ ] Dynamic schema includes remote team refs as enum values
- [x] `schema refresh` regenerates dynamic schema from current state
- [ ] Config validation errors include path + field + clear message

### Acceptance Criteria

- [ ] YAML editors can autocomplete groups/remote refs.
- [ ] Invalid config points to path+field with clear message.

### Risks

- Dynamic schema becoming stale without refresh.
- Large remote identity sets impacting schema size.

---

## Phase 8 - Sync, Codegen, and Import

- [ ] **Phase 8 complete**

### Goals

- [ ] Implement sync destinations for project-level encrypted secret delivery.
- [ ] Implement typed codegen for downstream consumers.
  **Note:** `codegen` is already implemented and functional (TS/Go/Python/Rust),
  but has been demoted to a hidden command. The canonical user-facing output
  command is `generate` (SOPS-encrypted files). Full Phase 8 codegen refers to
  deeper integration: config-driven codegen, CI hooks, etc.
- [ ] Implement import from external secret stores.

### Modules / files

```
rust/src/
├── cli/
│   ├── sync.rs                   # Full implementation (sync destinations + autosync)
│   ├── codegen.rs                # Full implementation (TS, Go, Python, Rust from store scan)
│   └── import.rs                 # Full implementation
├── import/
│   ├── mod.rs                    # Import trait, dispatch by source type
│   ├── sops.rs                   # SOPS YAML/JSON parser: decrypt via sops binary, extract keys
│   └── onepassword.rs            # 1Password: shell out to `op`, parse item fields
```

### Test cases

```bash
cargo test codegen                # Codegen output tests
cargo test import                 # Import source tests
cargo test --test sync_test       # Sync integration tests
```

#### Sync

- [x] `sync` writes encrypted `.age` files to project directory
- [x] `sync` does not write plaintext anywhere
- [x] `sync` is idempotent (running twice produces same result)
- [ ] `autosync_on: set` triggers sync after `himitsu set`
- [ ] `autosync_on: push` triggers sync after `himitsu remote push`
- [ ] Context isolation: `set` in project mode only writes to project's remote

#### Codegen

- [x] TypeScript output is syntactically valid (parseable by tsc)
- [x] Go output is syntactically valid (parseable by go vet)
- [x] Python output is syntactically valid (parseable by python -c)
- [x] Codegen merges common env with specific env (common first, env overrides)
- [ ] Codegen respects app-scoped extraction from data.json
- [ ] Snapshot tests: output matches expected for each language

#### Import

- [ ] `import --sops file.sops.yaml --env prod` extracts all keys
- [ ] Each extracted key is written as `vars/prod/<KEY>.age`
- [ ] `import --sops` handles nested YAML keys (flattens with `_` separator)
- [ ] `import --sops` with `--overwrite` replaces existing secrets
- [ ] `import --sops` without `--overwrite` skips existing secrets
- [ ] `import --op "op://vault/item/field" --env prod --key TOKEN` writes one secret
- [ ] `import --op "op://vault/item" --env prod` writes all fields from item
- [ ] `import --op` fails gracefully when `op` CLI is not installed

### Acceptance Criteria

- [ ] `sync` writes encrypted files to project without plaintext.
- [ ] Autosync triggers correctly based on configured event.
- [x] Codegen produces valid typed output for each supported language.
- [ ] SOPS import decrypts and re-encrypts all keys into `.himitsu/secrets/<env>/<KEY>.age`.
- [ ] 1Password import fetches and encrypts items into remote's format.

### Risks

- SOPS format variations (YAML vs JSON, nested vs flat keys).
- 1Password CLI version/auth differences across platforms.
- Autosync timing edge cases (concurrent mutations from multiple devices).

---

## Phase 9 - Cutover and Legacy Removal

- [ ] **Phase 9 complete**

### Goals

- [ ] Make Rust implementation the default.
- [ ] Remove shell implementation and update packaging/docs.

### Modules / files

No new Rust modules. Packaging and cleanup work:

```
legacy/                           # Archived shell implementation
├── bin/himitsu
└── lib/*.sh
flake.nix                         # Updated: Rust binary is default package
action/entrypoint.sh              # Updated: use Rust binary
action.yml                        # Updated if inputs changed
README.md                         # Rewritten for Rust CLI
```

### Test cases

```bash
cargo test                        # Full test suite must pass
nix build                         # Nix package builds
nix flake check                   # Flake checks pass
```

- [ ] All Phase 0 golden fixtures pass against Rust binary
- [ ] `nix build` produces working `himitsu` binary (Rust)
- [ ] `nix run .# -- --help` works
- [ ] GitHub Action works with Rust binary
- [ ] No remaining references to shell binary in packaging
- [ ] README accurately reflects Rust CLI

### Acceptance Criteria

- [ ] All planned commands operate from Rust binary.
- [ ] CI green on supported platforms.
- [ ] Migration guide validated on sample repos.

### Risks

- Missed command parity edge cases.
- Packaging regressions for existing users.

---

## Testing Infrastructure

### How to run tests

```bash
# All tests (unit + integration)
cargo test

# Unit tests only (fast, no I/O)
cargo test --lib

# Integration tests only
cargo test --test '*'

# Specific module tests
cargo test config
cargo test crypto
cargo test policy
cargo test protocol
cargo test index

# With output (see println! in tests)
cargo test -- --nocapture

# Snapshot tests (update snapshots after intentional changes)
cargo insta test
cargo insta review

# Lint and format
cargo clippy -- -D warnings
cargo fmt -- --check

# Legacy shell tests (until cutover)
bats tests/bats/
```

### CI pipeline (`.github/workflows/rust.yml`)

```yaml
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo fmt -- --check
      - run: cargo clippy -- -D warnings
      - run: cargo test
```

### Test dependencies

| Crate | Purpose |
|---|---|
| `assert_cmd` | Run CLI binary, assert stdout/stderr/exit code |
| `predicates` | Fluent assertions for CLI output matching |
| `tempfile` | Isolated temp directories per test |
| `insta` | Snapshot testing for output format stability |
| `wiremock` | HTTP mocking for GitHub API, .well-known, ENS RPC |
| `rusqlite` | Already a runtime dep; used directly in index tests |

### Test directory layout

```
tests/
├── integration/
│   ├── helpers/
│   │   └── mod.rs                # Shared setup: temp dir, init himitsu, create remote
│   ├── init_test.rs
│   ├── set_get_test.rs
│   ├── ls_test.rs
│   ├── encrypt_decrypt_test.rs
│   ├── recipient_test.rs
│   ├── group_test.rs
│   ├── remote_test.rs
│   ├── search_test.rs
│   ├── policy_test.rs
│   ├── envelope_test.rs
│   ├── inbox_test.rs
│   ├── sync_test.rs
│   ├── codegen_test.rs
│   └── import_test.rs
├── fixtures/
│   ├── golden/                   # Captured shell outputs for parity
│   ├── configs/                  # Sample YAML configs
│   ├── remotes/                  # Sample remote directory layouts
│   └── envelopes/                # Sample signed envelope JSON files
└── bats/                         # Legacy shell tests (until Phase 9)
    ├── test_helper.bash
    ├── init.bats
    ├── crypto.bats
    ├── recipient.bats
    └── group.bats
```

### Test helper pattern

Every integration test follows this pattern:

```rust
use assert_cmd::Command;
use tempfile::TempDir;

fn himitsu() -> Command {
    Command::cargo_bin("himitsu").unwrap()
}

#[test]
fn set_get_roundtrip() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();

    // init
    himitsu()
        .env("HOME", home.path())
        .arg("init")
        .assert()
        .success();

    // set
    himitsu()
        .env("HOME", home.path())
        .current_dir(project.path())
        .args(["set", "prod", "API_KEY", "secret123"])
        .assert()
        .success();

    // get
    himitsu()
        .env("HOME", home.path())
        .current_dir(project.path())
        .args(["get", "prod", "API_KEY"])
        .assert()
        .success()
        .stdout("secret123\n");
}
```

---

## Cross-cutting Workstreams

### Security

- [ ] Envelope signature verification tests.
- [ ] Replay DB integrity checks.
- [ ] Sender allowlist enforcement.
- [ ] Secret redaction in logs and errors.

### Observability

- [ ] Structured logs for send/accept operations.
- [ ] Debug mode with trace IDs per envelope.

### Performance

- [ ] Cache remote identity lookups.
- [ ] Batch decrypt/encrypt where safe.
- [ ] Avoid blocking relay/network calls on unrelated commands.

---

## Milestone Checklist

- [x] M0: Docs frozen, golden fixtures captured (Phase 0)
- [x] M1: Rust scaffold builds, `--help` works (Phase 1)
- [ ] M2: Local secret parity: init/set/get/ls/encrypt/decrypt/sync/remote/search (Phase 2)
- [ ] M3: Recipient policy engine with include/exclude (Phase 3)
- [ ] M4: GitHub PR inbox send/receive end-to-end (Phase 4)
- [ ] M5: Nostr send/receive end-to-end (Phase 5)
- [ ] M6: Email/ENS/Nostr identity resolvers with caching (Phase 6)
- [ ] M7: Schema validation and autocomplete (Phase 7)
- [ ] M8: Sync + codegen + import (Phase 8)
- [ ] M9: Rust binary is default, shell archived (Phase 9)

---

## Definition of Done

The rewrite is complete when:

- [ ] 1. Rust CLI fully replaces shell runtime.
- [ ] 2. Secrets remain `age` encrypted end-to-end.
- [ ] 3. GitHub PR inbox and Nostr roundtrip both work in production-like tests.
- [ ] 4. Replay protection and signature verification are enforced by default.
- [ ] 5. Config autocomplete and schema validation are available.
- [ ] 6. Import from SOPS and 1Password is functional and tested.
- [ ] 7. Shell implementation is archived and Nix packaging updated.
