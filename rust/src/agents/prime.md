<!-- himitsu prime — append this to your AGENTS.md -->
## Himitsu — Age-based Secret Manager

Himitsu encrypts secrets with [age](https://github.com/FiloSottile/age) x25519 keys, stores one `.age` file per secret, and injects them into child processes. Zero plaintext at rest. Transport is untrusted.

### Commands

```bash
himitsu set <path> <value> --tag <tag>    # Encrypt and store
himitsu get <path>                         # Decrypt and print
himitsu ls [prefix]                        # Browse secrets by path
himitsu search <query>                     # Fuzzy search across stores
himitsu exec <ref> -- <cmd>                # Run with secrets as env vars
```

### Exec Ref Formats

| Format | Example | Behavior |
|--------|---------|----------|
| Tag selector | `tag:pci` | All secrets carrying tag `pci` |
| Output label | `my-env` | Resolved from project config `outputs:` (local-store secrets only) |
| Prefix glob | `prod/*` | Every secret under `prod/` |
| Trailing slash | `prod/` | Same as `prod/*` (avoids shell expansion) |
| Concrete path | `prod/API_KEY` | Single secret |

**Tip**: Use trailing-slash (`prod/`) instead of `prod/*` to avoid the shell expanding the glob before himitsu sees it.

### Test Isolation

Integration tests use `HIMITSU_HOME` (not `HOME`) and `--store` to isolate:

```rust
use assert_cmd::Command;
use tempfile::TempDir;

fn himitsu() -> Command { Command::cargo_bin("himitsu").unwrap() }

fn setup() -> (TempDir, TempDir) {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    himitsu()
        .env("HIMITSU_HOME", home.path())
        .args(["--store", &project.path().join(".himitsu").to_string_lossy(), "init"])
        .assert().success();
    (home, project)
}
```

### Constraints

- All errors are typed `HimitsuError` — no `anyhow` or `Box<dyn Error>` in library code.
- Secrets are always encrypted at rest. No bulk decrypt.
- Transport (GitHub, Nostr) is never trusted — only envelope signatures and age encryption protect secrets.
