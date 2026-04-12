You are an experienced, pragmatic software engineering AI agent. Do not over-engineer a solution when a simple one is possible. Keep edits minimal. If you want an exception to ANY rule, you MUST stop and get permission first.

# Himitsu — Agent Guide

`himitsu` (秘密, "secret") is an **age-based secret management CLI** with transport-agnostic sharing. Secrets are encrypted with [age](https://github.com/FiloSottile/age) x25519 keys, stored one-file-per-key (`.himitsu/secrets/<path>.age`) in git-backed stores, and shared via signed envelopes over GitHub PR inboxes or Nostr — never as plaintext.

The project is undergoing a **full Rust rewrite** from a legacy shell implementation. See `docs/IMPLEMENTATION_PLAN.md` for the current phase status and open work.

---

## Project Overview

| Area | Detail |
|---|---|
| **Language** | Rust (CLI binary), TypeScript/Bun (TUI) |
| **Crypto** | `age` crate (x25519 encryption), Ed25519 (envelope signing) |
| **Storage** | Local filesystem (XDG: `~/.local/share/himitsu/`, `~/.local/state/himitsu/`), SQLite index (`rusqlite`) |
| **Serialization** | `serde_json`, `serde_yaml`, `prost` (protobuf for config schema) |
| **CLI framework** | `clap` v4 (derive macros) |
| **Error handling** | `thiserror` |
| **Logging** | `tracing` + `tracing-subscriber` |
| **Dev environment** | Nix flake (`flake.nix`) |
| **CI** | GitHub Actions (Ubuntu + macOS) |
| **Issue tracking** | `bd` (beads) — see [Beads Issue Tracker](#beads-issue-tracker) |

Key design invariants:
- **Zero plaintext at rest** — secrets are always encrypted before hitting the filesystem.
- **Transport is untrusted** — only envelope signatures and age encryption protect secrets; the transport layer (GitHub, Nostr, etc.) is never trusted.
- **One file per secret** — `.himitsu/secrets/<path>.age` keeps diffs readable and listing fast without any decryption.

---

## Reference

### Directory Layout

```
himitsu/
├── rust/src/
│   ├── main.rs               # Entrypoint, CLI dispatch
│   ├── error.rs              # HimitsuError enum (all error variants)
│   ├── git.rs                # git CLI wrapper
│   ├── cli/                  # One file per subcommand
│   │   ├── mod.rs            # Cli struct + command dispatch
│   │   ├── init.rs, set.rs, get.rs, ls.rs
│   │   ├── encrypt.rs, decrypt.rs, sync.rs, search.rs
│   │   ├── recipient.rs, group.rs, remote.rs, share.rs
│   │   ├── inbox.rs, import.rs, schema.rs, codegen.rs
│   │   └── git.rs
│   ├── config/mod.rs         # Mode detection, config loading/validation
│   ├── crypto/               # age encryption/decryption, Ed25519
│   ├── remote/               # Remote resolution, secret file I/O, sync
│   ├── index/mod.rs          # SQLite cross-remote search index
│   ├── keyring/              # OS keychain adapters (macOS, etc.)
│   └── proto/mod.rs          # Protobuf-generated config schema models
├── tests/integration/
│   └── cli_test.rs           # All integration tests (assert_cmd pattern)
├── proto/                    # .proto source files (compiled by build.rs)
├── tui/                      # Bun/TypeScript terminal UI (@opentui/core)
├── docs/
│   ├── ARCHITECTURE.md       # Full system design
│   ├── IMPLEMENTATION_PLAN.md # Phase-by-phase execution plan (update this!)
│   ├── SHARING.md            # Envelope / transport protocol spec
│   ├── BACKENDS.md, SERVER_API.md, USE_CASES.md
├── action/entrypoint.sh      # GitHub Actions entrypoint
├── build.rs                  # Proto compilation (prost-build)
├── flake.nix                 # Nix dev environment + package
└── Cargo.toml                # Single-binary workspace
```

### Key Modules

| Module | Responsibility |
|---|---|
| `cli/` | Command parsing and UX; one file per subcommand |
| `config/` | Project-mode vs user-mode detection; config schema |
| `crypto/` | age encrypt/decrypt; Ed25519 envelope signing |
| `remote/` | Remote discovery, secret file I/O, sync destinations |
| `index/` | SQLite secret index for cross-remote `search` |
| `keyring/` | OS keychain adapters for local age key storage |
| `proto/` | Protobuf models (generated from `proto/*.proto`) |
| `error.rs` | `HimitsuError` — all error variants live here |

### Runtime Paths

```
~/.local/share/himitsu/      # XDG data dir
  key                        # age private key
  key.pub                    # age public key

~/.local/state/himitsu/      # XDG state dir
  himitsu.db                 # Cross-remote search index (SQLite)
  stores/<org>/<repo>/       # Store checkouts
    .himitsu/
      secrets/<path>.age       # Encrypted secret files
      recipients/<group>/*.pub # Recipient age pubkeys
      config.yaml              # Store config (recipients_path override, etc.)
    himitsu.yaml               # Remote policy config
    data.json                  # Group/env metadata
```

---

## Essential Commands

### Rust CLI (run from project root)

```bash
cargo build                              # Debug build
cargo build --release                    # Release build
cargo fmt --all                          # Format code
cargo fmt --all -- --check               # Check formatting (CI gate)
cargo clippy --workspace --all-targets -- -D warnings  # Lint (CI gate)
cargo test --workspace                   # All tests
cargo test --lib                         # Unit tests only
cargo test --test '*'                    # Integration tests only
cargo test --test cli_test <fn_name> -- --nocapture  # Single integration test
cargo insta test                         # Run snapshot tests
cargo insta review                       # Review/accept snapshot changes
```

### Bun / TypeScript TUI (`tui/` directory)

```bash
cd tui
bun install      # Install dependencies
bun run check    # Type-check (tsc --noEmit)
bun run dev      # Run TUI
```

### Nix

```bash
nix develop          # Enter dev shell
nix build            # Build the package
nix flake check      # Verify full Nix package (run after Nix/dep changes)
```

---

## Patterns

### Integration Test Isolation

All integration tests live in `tests/integration/cli_test.rs` and use `assert_cmd` + `tempfile`. The env var `HIMITSU_HOME` (not `HOME`) isolates the himitsu key store; `--store` isolates the project secret store. **Do not** rely on the developer's real home directory.

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn himitsu() -> Command {
    Command::cargo_bin("himitsu").unwrap()
}

/// Canonical setup helper — mirrors the one in cli_test.rs
fn setup() -> (TempDir, TempDir) {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args([
            "--store",
            &project.path().join(".himitsu").to_string_lossy(),
            "init",
        ])
        .assert()
        .success();
    (home, project)
}

#[test]
fn test_new_feature() {
    let (home, project) = setup();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &project.path().join(".himitsu").to_string_lossy(), "mycmd", "--flag"])
        .assert()
        .success()
        .stdout(predicate::str::contains("expected output"));
}
```

### Error Handling

Add all new error variants to `HimitsuError` in `rust/src/error.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum HimitsuError {
    #[error("config not found: {0}")]
    ConfigNotFound(String),
    // ...
}
```

Return `Result<T, HimitsuError>` from all failable functions. Never use `anyhow` or `Box<dyn Error>` in library/core code.

### Implementing a New Subcommand

1. Create `rust/src/cli/<name>.rs` with a `pub struct <Name>Args` (clap derive) and `pub fn run(args: <Name>Args) -> Result<(), HimitsuError>`.
2. Register it in `rust/src/cli/mod.rs` under the `Commands` enum.
3. Dispatch it in `main.rs`.
4. Add integration tests in `tests/integration/cli_test.rs` using the setup pattern above.
5. Check off the matching item in `docs/IMPLEMENTATION_PLAN.md`.

### Implementation Plan Tracking

When completing any planned work:
1. Open `docs/IMPLEMENTATION_PLAN.md`.
2. Change `- [ ]` → `- [x]` for the matching item.
3. If it was the last item in a phase, check the phase header too.

---

## Anti-patterns

- **Do NOT write plaintext secrets to disk.** `bulk decrypt` is intentionally unsupported. Use `himitsu get <path>` to read individual values.
- **Do NOT use `HOME` in tests.** Use `HIMITSU_HOME` to isolate himitsu's key store in integration tests (see pattern above).
- **Do NOT use `anyhow` or `Box<dyn Error>` in library code.** All errors must be typed `HimitsuError` variants.
- **Do NOT manually format code.** Let `rustfmt` handle it; never add `#[rustfmt::skip]` without explicit permission.
- **Do NOT add external dependencies without discussion.** Prefer established crates (`serde`, `clap`, `thiserror`, `tracing`, `rusqlite`); keep the dependency surface minimal.
- **Do NOT add markdown TODO lists.** Use `bd` for all task tracking.

---

## Code Style

### Rust

- Formatting: `rustfmt` (enforced in CI — zero tolerance for format violations).
- Naming: `snake_case` functions/variables/modules, `PascalCase` types/traits/enums, `SCREAMING_SNAKE_CASE` constants.
- Imports: group `std` → external crates → internal modules, separated by blank lines.
- Logging: `tracing` macros for internal diagnostics; `println!`/`eprintln!` only for user-facing CLI output.

### TypeScript (TUI)

- Strict types — no `any`. Define interfaces for all data shapes.
- ES modules only (`"type": "module"` in `package.json`).
- Favor immutable state updates.

---

## Commit and Pull Request Guidelines

### Pre-commit Checklist

```bash
cargo fmt --all -- --check             # Must pass
cargo clippy --workspace --all-targets -- -D warnings  # Must pass
cargo test --workspace                 # Must pass
cd tui && bun run check                # If TUI was changed
```

CI enforces all three Rust gates on every push to `main` and every PR.

### Commit Message Convention

Use `type: short description` (≤72 chars), e.g.:

```
feat: add recipient rm subcommand
fix: handle missing config.yaml gracefully
test: add integration tests for group lifecycle
chore: update clap to 4.5
docs: update implementation plan phase 1 status
```

Types: `feat`, `fix`, `test`, `chore`, `docs`, `refactor`, `perf`.

### Pull Request Requirements

- All CI checks green (fmt, clippy, tests on Ubuntu + macOS).
- Include a brief description of what changed and why.
- Reference the relevant `bd` issue ID if one exists (e.g., `closes bd-42`).
- Update `docs/IMPLEMENTATION_PLAN.md` checkboxes if the PR completes planned work.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->

<!-- bv-agent-instructions-v2 -->

---

## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`) for issue tracking and [beads_viewer](https://github.com/Dicklesworthstone/beads_viewer) (`bv`) for graph-aware triage. Issues are stored in `.beads/` and tracked in git.

### Using bv as an AI sidecar

bv is a graph-aware triage engine for Beads projects (.beads/beads.jsonl). Instead of parsing JSONL or hallucinating graph traversal, use robot flags for deterministic, dependency-aware outputs with precomputed metrics (PageRank, betweenness, critical path, cycles, HITS, eigenvector, k-core).

**Scope boundary:** bv handles *what to work on* (triage, priority, planning). `br` handles creating, modifying, and closing beads.

**CRITICAL: Use ONLY --robot-* flags. Bare bv launches an interactive TUI that blocks your session.**

#### The Workflow: Start With Triage

**`bv --robot-triage` is your single entry point.** It returns everything you need in one call:
- `quick_ref`: at-a-glance counts + top 3 picks
- `recommendations`: ranked actionable items with scores, reasons, unblock info
- `quick_wins`: low-effort high-impact items
- `blockers_to_clear`: items that unblock the most downstream work
- `project_health`: status/type/priority distributions, graph metrics
- `commands`: copy-paste shell commands for next steps

```bash
bv --robot-triage        # THE MEGA-COMMAND: start here
bv --robot-next          # Minimal: just the single top pick + claim command

# Token-optimized output (TOON) for lower LLM context usage:
bv --robot-triage --format toon
```

#### Other bv Commands

| Command | Returns |
|---------|---------|
| `--robot-plan` | Parallel execution tracks with unblocks lists |
| `--robot-priority` | Priority misalignment detection with confidence |
| `--robot-insights` | Full metrics: PageRank, betweenness, HITS, eigenvector, critical path, cycles, k-core |
| `--robot-alerts` | Stale issues, blocking cascades, priority mismatches |
| `--robot-suggest` | Hygiene: duplicates, missing deps, label suggestions, cycle breaks |
| `--robot-diff --diff-since <ref>` | Changes since ref: new/closed/modified issues |
| `--robot-graph [--graph-format=json\|dot\|mermaid]` | Dependency graph export |

#### Scoping & Filtering

```bash
bv --robot-plan --label backend              # Scope to label's subgraph
bv --robot-insights --as-of HEAD~30          # Historical point-in-time
bv --recipe actionable --robot-plan          # Pre-filter: ready to work (no blockers)
bv --recipe high-impact --robot-triage       # Pre-filter: top PageRank scores
```

### br Commands for Issue Management

```bash
br ready              # Show issues ready to work (no blockers)
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br create --title="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason="Completed"
br close <id1> <id2>  # Close multiple issues at once
br sync --flush-only  # Export DB to JSONL
```

### Workflow Pattern

1. **Triage**: Run `bv --robot-triage` to find the highest-impact actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Always run `br sync --flush-only` at session end

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready` shows only unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers 0-4, not words)
- **Types**: task, bug, feature, epic, chore, docs, question
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Session Protocol

```bash
git status              # Check what changed
git add <files>         # Stage code changes
br sync --flush-only    # Export beads changes to JSONL
git commit -m "..."     # Commit everything
git push                # Push to remote
```

<!-- end-bv-agent-instructions -->
