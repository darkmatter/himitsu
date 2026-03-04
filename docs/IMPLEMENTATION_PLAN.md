# Himitsu Implementation Plan (Detailed)

This document defines an execution plan for the vNext architecture:

- centralized store (`~/.himitsu`)
- `age`-only secret model (`vars/<env>/<KEY>.age`)
- transport-agnostic sharing protocol
- GitHub PR inbox + Nostr send/receive
- full Rust rewrite of current shell implementation

---

## Phase 0 - Freeze and Baseline

### Goals

- Freeze docs and expected command behavior.
- Capture baseline tests and golden fixtures before rewrite.

### Deliverables

- Finalized docs in `docs/`.
- Snapshot fixtures for representative repositories.
- Legacy command behavior matrix.

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

- [ ] `init` creates expected directory tree and files
- [ ] `set` / `get` roundtrip produces correct plaintext
- [ ] `recipient add --self` writes correct key file
- [ ] `group add` / `group rm` updates data.json
- [ ] `encrypt` / `decrypt` roundtrip is lossless
- [ ] `ls` output format captured as golden fixture

### Acceptance Criteria

- Team agrees on command compatibility goals.
- Golden fixtures can be replayed against new implementation.

### Risks

- Hidden assumptions in shell scripts.
- Incomplete fixture coverage.

---

## Phase 1 - Rust Project Scaffold

### Goals

- Create Rust crate and executable `himitsu`.
- Implement command parsing and logging framework.

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

- [ ] `himitsu --help` prints full command tree with all subcommands
- [ ] `himitsu --version` prints version string
- [ ] `himitsu <subcommand> --help` works for every subcommand stub
- [ ] Binary builds on macOS (aarch64-apple-darwin)
- [ ] Binary builds on Linux (x86_64-unknown-linux-gnu)
- [ ] `nix build` produces both shell and Rust binaries

### Acceptance Criteria

- `himitsu --help` works with planned command tree.
- Project builds on macOS and Linux.

### Risks

- Slow startup due to heavy crate selection.
- Over-designing module boundaries too early.

---

## Phase 2 - Core Runtime Parity (No Sharing Yet)

### Goals

- Implement config loading and mode detection.
- Implement remote resolution and local secret operations.

### Modules / files

```
rust/src/
├── config/
│   ├── mod.rs                    # detect_mode() → ProjectMode | UserMode
│   ├── global.rs                 # GlobalConfig: parse ~/.himitsu/config.yaml
│   ├── project.rs                # ProjectConfig: parse <repo>/.himitsu.yaml
│   └── remote.rs                 # RemoteConfig: parse remote himitsu.yaml
├── remote/
│   ├── mod.rs                    # Remote discovery, resolution, list known remotes
│   └── store.rs                  # Secret file I/O: read/write vars/<env>/<KEY>.age
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

### Test cases

```bash
cargo test                        # Unit + integration
cargo test --test '*'             # Integration tests only
```

#### Unit tests (inline `#[cfg(test)]`)

- [ ] `config::detect_mode` returns `ProjectMode` when `.git` + `.himitsu.yaml` exist
- [ ] `config::detect_mode` returns `UserMode` when `.git` exists without `.himitsu.yaml`
- [ ] `config::detect_mode` returns `UserMode` when no `.git` found
- [ ] `config::global::parse` loads valid config.yaml
- [ ] `config::global::parse` rejects malformed YAML with clear error
- [ ] `config::project::parse` reads `remote:` field
- [ ] `config::remote::parse` loads policies, identity_sources
- [ ] `crypto::age::keygen` produces valid x25519 keypair
- [ ] `crypto::age::encrypt` → `decrypt` roundtrip preserves plaintext
- [ ] `crypto::age::encrypt` with multiple recipients succeeds
- [ ] `crypto::age::decrypt` with wrong key fails with clear error
- [ ] `remote::store::write_secret` creates `vars/<env>/<KEY>.age`
- [ ] `remote::store::read_secret` reads and decrypts `.age` file
- [ ] `remote::store::list_secrets` returns all keys for an env
- [ ] `remote::store::list_secrets` handles nested subdirectories
- [ ] `git::run` executes git commands and captures output
- [ ] `git::run` returns error for non-zero exit codes
- [ ] `index::SecretIndex::upsert` inserts new entry
- [ ] `index::SecretIndex::upsert` updates existing entry (same remote+path)
- [ ] `index::SecretIndex::search` matches partial key names
- [ ] `index::SecretIndex::search` returns results across multiple remotes

#### Integration tests (`tests/integration/`)

- [ ] `init` creates `~/.himitsu/` with keys/, config.yaml, state/
- [ ] `init` is idempotent (running twice doesn't error or overwrite keys)
- [ ] `set prod API_KEY "secret"` creates `vars/prod/API_KEY.age`
- [ ] `get prod API_KEY` returns `"secret"` after set
- [ ] `set` then `get` with multiline values preserves newlines
- [ ] `set` then `get` with special characters (quotes, backslashes, unicode)
- [ ] `ls` with no args lists all envs
- [ ] `ls prod` lists keys in prod env
- [ ] `encrypt` re-encrypts all secrets for current recipients
- [ ] `decrypt` is not implemented / errors (no plaintext at rest)
- [ ] `recipient add --self --group team` writes pubkey file to recipients/team/
- [ ] `recipient add` with explicit `--age-key` writes correct .pub file
- [ ] `recipient rm` removes the key file
- [ ] `recipient ls` shows all recipients, optionally filtered by group
- [ ] `group add mygroup` creates directory + updates data.json
- [ ] `group rm mygroup` removes directory + updates data.json
- [ ] `group rm common` is rejected (reserved)
- [ ] `group ls` lists groups with recipient counts
- [ ] `remote add <org/repo>` clones repo into `~/.himitsu/data/`
- [ ] `remote push` commits and pushes changes
- [ ] `remote pull` fetches latest from origin
- [ ] `remote status` shows clean/dirty state
- [ ] `search <query>` matches key names across remotes
- [ ] `search` with no matches returns empty output, exit 0
- [ ] Golden fixture parity: outputs match captured shell fixtures

### Acceptance Criteria

- Core local commands produce expected filesystem results.
- Equivalent flows succeed on baseline fixtures.
- `himitsu search` returns results across multiple remotes.

### Risks

- Edge cases with path expansion and symlinked directories.
- Value quoting/newline handling in `set`.

---

## Phase 3 - Recipient Policy Engine

### Goals

- Replace env-only recipient model with path policy resolution.
- Support local groups and remote refs in one normalized pipeline.

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

- Recipient resolution snapshots are deterministic.
- Policy tests cover include/exclude precedence.

### Risks

- Policy complexity creep.
- Ambiguous behavior at path boundaries.

---

## Phase 4 - Protocol + GitHub PR Inbox

### Goals

- Implement signed envelope protocol.
- Ship first complete external sharing path via GitHub PR inbox.

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

- Sender can share to external inbox repo via PR.
- Receiver can verify, decrypt, and apply encrypted output.
- Duplicate envelope IDs are rejected.

### Risks

- GitHub auth/token scope complexity.
- Canonicalization bugs causing signature mismatch.

---

## Phase 5 - Nostr Send and Receive

### Goals

- Add full Nostr roundtrip delivery.

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

- `share send --to nostr:...` publishes valid event.
- `inbox list --transport nostr` returns envelopes.
- `inbox accept` succeeds end-to-end.

### Risks

- Relay reliability and event propagation latency.
- Metadata inconsistencies between relays.

---

## Phase 6 - Identity Resolvers (Email + ENS + Nostr)

### Goals

- Implement external identity resolution beyond GitHub.

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

- `share send --to email:...` resolves profile and sends.
- `share send --to ens:...` resolves profile and sends.
- Resolver output is cached and reproducible.

### Risks

- Resolver trust and TOFU pitfalls.
- Network timeout behavior degrading CLI UX.

---

## Phase 7 - Schema and Developer UX

### Goals

- Add schema-backed validation and autocomplete support.

### Modules / files

```
rust/src/
├── schema/
│   ├── mod.rs                    # Orchestration: generate static + dynamic schemas
│   ├── static_schema.rs          # Generate schemas/himitsu.schema.json
│   └── dynamic.rs                # Generate schemas/recipients.schema.json from live data
├── cli/
│   └── schema.rs                 # Full implementation
```

### Test cases

```bash
cargo test schema                 # Schema generation tests
```

- [ ] Static schema is valid JSON Schema draft 2020-12
- [ ] Static schema validates a correct `himitsu.yaml`
- [ ] Static schema rejects missing required fields
- [ ] Dynamic schema includes local group names as enum values
- [ ] Dynamic schema includes remote team refs as enum values
- [ ] `schema refresh` regenerates dynamic schema from current state
- [ ] Config validation errors include path + field + clear message

### Acceptance Criteria

- YAML editors can autocomplete groups/remote refs.
- Invalid config points to path+field with clear message.

### Risks

- Dynamic schema becoming stale without refresh.
- Large remote identity sets impacting schema size.

---

## Phase 8 - Sync, Codegen, and Import

### Goals

- Implement sync destinations for project-level encrypted secret delivery.
- Implement typed codegen for downstream consumers.
- Implement import from external secret stores.

### Modules / files

```
rust/src/
├── cli/
│   ├── sync.rs                   # Full implementation (sync destinations + autosync)
│   ├── codegen.rs                # Full implementation
│   └── import.rs                 # Full implementation
├── codegen/
│   ├── mod.rs                    # Orchestration: detect lang, merge envs, write output
│   ├── typescript.rs             # TypeScript interface + const generation
│   ├── golang.rs                 # Go struct generation
│   └── python.rs                 # Python dataclass generation
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

- [ ] `sync` writes encrypted `.age` files to project directory
- [ ] `sync` does not write plaintext anywhere
- [ ] `sync` is idempotent (running twice produces same result)
- [ ] `autosync_on: set` triggers sync after `himitsu set`
- [ ] `autosync_on: push` triggers sync after `himitsu remote push`
- [ ] Context isolation: `set` in project mode only writes to project's remote

#### Codegen

- [ ] TypeScript output is syntactically valid (parseable by tsc)
- [ ] Go output is syntactically valid (parseable by go vet)
- [ ] Python output is syntactically valid (parseable by python -c)
- [ ] Codegen merges common env with specific env (common first, env overrides)
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

- `sync` writes encrypted files to project without plaintext.
- Autosync triggers correctly based on configured event.
- Codegen produces valid typed output for each supported language.
- SOPS import decrypts and re-encrypts all keys into `vars/<env>/<KEY>.age`.
- 1Password import fetches and encrypts items into remote's format.

### Risks

- SOPS format variations (YAML vs JSON, nested vs flat keys).
- 1Password CLI version/auth differences across platforms.
- Autosync timing edge cases (concurrent mutations from multiple devices).

---

## Phase 9 - Cutover and Legacy Removal

### Goals

- Make Rust implementation the default.
- Remove shell implementation and update packaging/docs.

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

- All planned commands operate from Rust binary.
- CI green on supported platforms.
- Migration guide validated on sample repos.

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

- Envelope signature verification tests.
- Replay DB integrity checks.
- Sender allowlist enforcement.
- Secret redaction in logs and errors.

### Observability

- Structured logs for send/accept operations.
- Debug mode with trace IDs per envelope.

### Performance

- Cache remote identity lookups.
- Batch decrypt/encrypt where safe.
- Avoid blocking relay/network calls on unrelated commands.

---

## Milestone Checklist

- [ ] M0: Docs frozen, golden fixtures captured (Phase 0)
- [ ] M1: Rust scaffold builds, `--help` works (Phase 1)
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

1. Rust CLI fully replaces shell runtime.
2. Secrets remain `age` encrypted end-to-end.
3. GitHub PR inbox and Nostr roundtrip both work in production-like tests.
4. Replay protection and signature verification are enforced by default.
5. Config autocomplete and schema validation are available.
6. Import from SOPS and 1Password is functional and tested.
7. Shell implementation is archived and Nix packaging updated.
