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

### Acceptance Criteria

- Team agrees on command compatibility goals.
- Golden fixtures can be replayed against new implementation.

### Risks

- Hidden assumptions in shell scripts.
- Incomplete fixture coverage.

---

## Phase 1 - Rust Project Scaffold

### Goals

- Create Rust workspace and executable `himitsu`.
- Implement command parsing and logging framework.

### Deliverables

- `Cargo.toml` workspace and crates.
- `src/main.rs` with command stubs.
- CI pipeline running `cargo fmt`, `cargo clippy`, `cargo test`.

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

### Deliverables

- Global + project config loader.
- CWD to HOME mode detection (with fallback: `.git` without `.himitsu.yaml`
  falls through to user mode).
- Remote management: `add`, `push`, `pull`, `status`.
- Git CLI wrapper for clone/commit/push/pull operations.
- `set/get/ls/encrypt/decrypt` using `age` Rust crate (native, no subprocess).
- `group add|rm|ls` and `recipient add|rm|ls`.
- SQLite secret index (`~/.himitsu/state/index.db`): incremental updates on
  mutations, `himitsu search` across all remotes.

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

### Deliverables

- Policy parser for `himitsu.yaml`.
- Longest-prefix policy merge algorithm.
- Recipient ref expansion (`group`, `remote`, `email`, `ens`, `nostr`).
- Deterministic recipient ordering and deduping.

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

### Deliverables

- Rust structs for profile/payload/envelope.
- Canonical JSON signing and verification (JCS, RFC 8785).
- GitHub transport adapter:
  - send envelope to `.himitsu/inbox/<id>.json` in PR
  - list/fetch pending envelopes
- Inbox acceptance pipeline with replay DB.

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

### Deliverables

- Relay config support in global config.
- Nostr adapter for publish/list/fetch.
- Envelope parsing from event content.
- Replay-safe `inbox accept` path for Nostr events.

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

### Deliverables

- Email resolver:
  - `https://<domain>/.well-known/himitsu.json`
- ENS resolver:
  - text records `himitsu_public_key`, `himitsu_inbox`
- Nostr identity resolver:
  - npub normalization + optional profile metadata fetch
- Local cache and lockfile pinning.

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

### Deliverables

- Static schema: `schemas/himitsu.schema.json`
- Generated schema: `schemas/recipients.schema.json`
- `himitsu schema refresh`
- Better command diagnostics and actionable errors.

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

### Deliverables

- `sync` command writes encrypted `.age` files to declared project destinations.
- Autosync configuration (`autosync: true`, trigger on `set`/`commit`/`push`).
- Context isolation enforcement (project mode only sees its own remote).
- `codegen` command with TypeScript, Go, and Python output.
- `import --sops <path>` for SOPS-encrypted YAML/JSON files.
- `import --op <ref>` for 1Password items via `op` CLI.

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

### Deliverables

- Remove or archive `src/lib/*.sh` runtime path.
- Update Nix/flake packaging for Rust binary.
- Update README and migration instructions.
- Final compatibility and regression report.

### Acceptance Criteria

- All planned commands operate from Rust binary.
- CI green on supported platforms.
- Migration guide validated on sample repos.

### Risks

- Missed command parity edge cases.
- Packaging regressions for existing users.

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

### Test Strategy

- **Unit tests** (`#[cfg(test)]`): config parsing, policy engine, protocol
  structs, crypto round-trips, canonical JSON determinism.
- **Snapshot tests** (`insta`): CLI output format, schema generation, config
  serialization, codegen output.
- **Integration tests** (`assert_cmd`): full CLI commands against temp dirs,
  transport adapter flows.
- **Golden fixtures**: captured shell outputs for parity verification.
- **HTTP mocks** (`wiremock`): GitHub API, `.well-known` endpoints, ENS RPC.
- **E2E sharing**: cross-repo sharing with real git repos and optional Nostr
  relay (Docker-based in CI).

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
