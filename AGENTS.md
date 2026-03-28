# Himitsu Agent Instructions (AGENTS.md)

Welcome! This file provides essential guidelines, commands, and project conventions for AI coding agents operating in the `himitsu` repository. `himitsu` is an age-based secret management tool with transport-agnostic sharing.

---

## 1. Build, Lint, and Test Commands

### Rust CLI (Root directory)
The primary implementation is a Rust binary. Ensure you are in the project root when executing `cargo` commands.

- **Build**:
  - Debug: `cargo build`
  - Release: `cargo build --release`
- **Lint & Format**:
  - Format Check: `cargo fmt --all -- --check`
  - Format Fix: `cargo fmt --all`
  - Clippy (Strict): `cargo clippy --workspace --all-targets -- -D warnings`
- **Testing Suites**:
  - Run all tests: `cargo test --workspace`
  - Run unit tests only: `cargo test --lib`
  - Run integration tests only: `cargo test --test '*'`
- **Running Specific Tests**:
  - Single test function: `cargo test <test_function_name> -- --nocapture`
  - Specific integration file test: `cargo test --test <file_name> <test_function_name> -- --nocapture`
  - *Example*: `cargo test --test cli_test test_init_command -- --nocapture`
- **Snapshot Tests**: 
  - Run snapshots: `cargo insta test`
  - Review and accept changes: `cargo insta review`

### Bun / TypeScript TUI (`/tui` directory)
The TUI is a Bun-based TypeScript application. Change directory to `tui/` before running these commands.

- **Install**: `bun install`
- **Type Check**: `bun run check` (Executes `tsc --noEmit`)
- **Run Dev**: `bun run dev`

---

## 2. Code Style & Architecture Guidelines

### Rust Conventions
- **Imports**: Group imports by `std`, external crates, and internal modules. Separate these groups with empty lines.
- **Formatting**: Strictly rely on `rustfmt`. Do not manually override formatting.
- **Naming Conventions**: 
  - `snake_case` for variables, functions, and modules.
  - `PascalCase` for structs, traits, and enums.
  - `SCREAMING_SNAKE_CASE` for constants and statics.
- **Error Handling**: 
  - Use the `thiserror` crate to define specific error variants.
  - Do NOT use `anyhow` or `Box<dyn Error>` in core library code.
  - Define custom errors in `rust/src/error.rs` as variants of `pub enum HimitsuError`.
  - Use `Result<T, HimitsuError>` for all failable operations.
  - Include relevant context in error messages (e.g., `#[error("config file not found: {0}")]`).
- **Logging & Output**: 
  - Use the `tracing` crate (`trace!`, `debug!`, `info!`, `warn!`, `error!`) for internal diagnostics and telemetry.
  - Reserve `println!` and `eprintln!` strictly for CLI output intended for the user (e.g., structured output of a command).
- **Security Mandate**: 
  - **Zero Plaintext**: Never write unencrypted secrets to the filesystem. `himitsu` strictly relies on an `age`-encrypted at-rest model.

### TypeScript / TUI Conventions
- **Type Safety**: Avoid `any`. Define strict interfaces for data structures.
- **Module System**: Ensure the use of ES modules (`"type": "module"`).
- **UI Logic**: Favor functional components and immutable state updates where applicable. 

---

## 3. Mandatory Agent Workflows

### Rule 1: Implementation Plan Tracking (Cursor Rules)
We track feature completion strictly through `docs/IMPLEMENTATION_PLAN.md`.
When you finish a task, you **MUST** update the plan:
1. **Locate the Item**: Open `docs/IMPLEMENTATION_PLAN.md` and find the corresponding Goal, Deliverable, Acceptance Criteria, or Test case.
2. **Mark as Done**: Change `- [ ]` to `- [x]`.
3. **Phase Completion**: If this was the last item in a Phase, check off the phase header (e.g., `- [x] **Phase 1 complete**`).

### Rule 2: Integration Testing Pattern
When writing new CLI integration tests in `tests/integration/`, strictly adhere to the `assert_cmd` and `tempfile` isolation pattern. Always mock `HOME` and execute within a temporary directory to avoid mutating the developer's actual machine or relying on external state.

```rust
use assert_cmd::Command;
use tempfile::TempDir;

fn himitsu() -> Command {
    Command::cargo_bin("himitsu").unwrap()
}

#[test]
fn test_new_feature_isolation() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();

    // Isolated execution
    himitsu()
        .env("HOME", home.path())
        .current_dir(project.path())
        .args(["mycmd", "--flag"])
        .assert()
        .success()
        .stdout(predicates::str::contains("expected output"));
}
```

### Rule 3: File Modifications & Dependencies
- Write idiomatic Rust code and rely on compiler warnings (`cargo check`).
- Ensure `Cargo.lock` is updated automatically when modifying `Cargo.toml`.
- If modifying the TUI, ensure `bun.lock` is consistent by running `bun install`.
- Limit external dependencies. Rely on established crates like `serde`, `clap`, `thiserror`, `tracing`, and `rusqlite`.

### Rule 4: Nix and Environment
- The repository provides a `flake.nix` for declarative environments. When working on dependencies, ensure they can still build in the isolated Nix sandbox.
- Use `nix build` or `nix flake check` to verify the complete package if making significant infrastructure changes.

---

## 4. Subsystem Locations

- **CLI Subcommands**: `rust/src/cli/*.rs`
- **Crypto / Encryption**: `rust/src/crypto/`
- **Config & Discovery**: `rust/src/config/`
- **Keyring Adapters**: `rust/src/keyring/`
- **Local SQLite Index**: `rust/src/index/`
- **Integration Tests**: `tests/integration/`
- **Legacy Shell / Bats Tests**: `tests/bats/`
- **GitHub Actions**: `action/`
- **Terminal UI**: `tui/`

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
