<center>
  <h1>himitsu<sup>秘密</sup></h1>
</center>

Age-based secrets management. Secrets are encrypted with [age](https://github.com/FiloSottile/age) x25519 keys, stored one-file-per-key in a git-backed `.himitsu/` store, with group-based recipient control and typed codegen.

![himitsu demo](demo/demo-vhs.gif)

## Features

- **Age-only encryption** — no KMS, no GPG, no SOPS. One `.age` file per secret.
- **One file per secret** — `.himitsu/secrets/<env>/<KEY>.age` keeps diffs readable.
- **Group-based recipients** — organize keys into named groups; re-encrypt for all with `encrypt`.
- **Typed codegen** — generate TypeScript, Go, Python, or Rust type stubs directly from your secret store.
- **Cross-store search** — `himitsu search` queries a local SQLite index across all known stores.
- **JSON schema export** — `himitsu schema` writes machine-readable schemas for your secret structure.
- **Nix integration** — flake library for devShell injection, secret packaging, and container entrypoints.

## Install

```bash
# Run directly
nix run github:darkmatter/himitsu -- <command>

# Add to a devShell
{
  inputs.himitsu.url = "github:darkmatter/himitsu";
  # ...
  devShells.default = pkgs.mkShell {
    packages = [ himitsu.packages.${system}.default ];
  };
}

# Build from source
cargo build --release
```

## Quick Start

```bash
# 1. Initialize — creates age keypair and scaffolds the store
himitsu init

# 2. Store secrets  (env  key  value)
himitsu set prod API_KEY     "sk_live_abc123"
himitsu set prod DB_PASSWORD "hunter2"
himitsu set dev  DB_PASSWORD "devpass"

# 3. Read them back
himitsu get prod API_KEY      # → sk_live_abc123
himitsu get dev  DB_PASSWORD  # → devpass

# 4. List environments / keys
himitsu ls        # → dev, prod
himitsu ls prod   # → API_KEY, DB_PASSWORD

# 5. Re-encrypt for all current recipients
himitsu encrypt

# 6. Generate typed stubs
himitsu codegen --lang typescript --env prod --stdout

# 7. Search across all stores
himitsu search DB --refresh
```

Use `-s <path>` to target a specific store directory instead of the auto-detected one:

```bash
himitsu -s /path/to/.himitsu set prod API_KEY "sk_live_xxx"
```

## TUI

Launch the in-process ratatui interface with:

```bash
himitsu tui
```

Search is the **root view** — the app opens straight into a fuzzy filter over every secret in the active store (grouped by store when more than one is known). Start typing to narrow the list, `↑`/`↓` to move the cursor, and `enter` to open a secret. Every other view pops back to a fresh search on `esc`; `esc` at the root quits.

![browse and drill](demo/tui-us-011.gif)

Press `?` in any view for a modal help overlay populated from that view's own bindings.

### Search (root)

| Key | Action |
|-----|--------|
| _type_ | filter results |
| `↑` / `↓` | move selection |
| `enter` | open selected secret |
| `backspace` | delete filter char |
| `ctrl-n` | new secret (opens form) |
| `ctrl-s` | switch active store (modal picker) |
| `ctrl-y` | copy selected secret value to clipboard |
| `?` | toggle help overlay |
| `esc` / `ctrl-c` | quit |

The `ctrl-s` store picker is a modal overlay: `↑`/`↓` to select, `enter` to switch, `esc` to dismiss.

### Secret viewer

Opened with `enter` on a search result.

| Key | Action |
|-----|--------|
| `r` | reveal / hide value |
| `y` | copy value to clipboard |
| `e` | edit value + metadata in `$EDITOR` (single document, `---` separator) |
| `R` | rekey for current recipients |
| `d` | delete secret (prompts for `y` to confirm, any other key cancels) |
| `?` | toggle help overlay |
| `esc` | back to search |
| `ctrl-c` | quit |

![secret viewer](demo/tui-us-012.gif)

### New-secret form

Opened with `ctrl-n` from search. Fields in order: `path`, `value`, `description`, `url`, `totp`, `env_key`, `expires_at`.

| Key | Action |
|-----|--------|
| `tab` / `enter` | next field (wraps; on `value` `enter` inserts a newline) |
| `shift-tab` | previous field (wraps) |
| `ctrl-s` / `ctrl-w` | save from any field |
| `esc` / `ctrl-c` | cancel |
| `?` | toggle help overlay |

![create secret](demo/tui-us-008.gif)

## Store Layout

```
.himitsu/                      # store root (auto-detected from $GIT_ROOT or ~/.himitsu)
  secrets/
    prod/
      API_KEY.age              # encrypted with all current recipients' age keys
      DB_PASSWORD.age
    dev/
      DB_PASSWORD.age
  recipients/
    common/
      self.pub                 # your age public key (added automatically on init)
      alice.pub
    ops/
      deploy-bot.pub
  schemas/                     # written by `himitsu schema refresh`
    secrets.json
```

The keyring lives separately in `$HIMITSU_HOME` (default: `~/.himitsu`):

```
~/.himitsu/
  key                          # age private key
  keys/
    age.txt                    # alternate key location (some builds)
  cache/                       # search index (SQLite)
```

## Commands

### `himitsu init`

Scaffold the store directory and generate a local age keypair. Adds `self` to the `common` recipient group automatically.

### `himitsu set <env> <key> <value>`

Encrypt a secret and write it to `secrets/<env>/<key>.age`.

```bash
himitsu set prod API_KEY "sk_live_abc123"
himitsu set dev  DB_PASSWORD "devpass" --no-push
```

`--no-push` skips the automatic git commit + push.

### `himitsu get <env> <key>`

Decrypt and print a single secret value.

```bash
himitsu get prod API_KEY
```

### `himitsu ls [env]`

List environments, or list key names within an environment.

```bash
himitsu ls         # → dev, prod
himitsu ls prod    # → API_KEY, DB_PASSWORD
```

### `himitsu encrypt [env]`

Re-encrypt secrets for the current recipient set. Run after adding or removing recipients.

```bash
himitsu encrypt        # re-encrypt all environments
himitsu encrypt prod   # re-encrypt one environment
```

### `himitsu search <query>`

Search key names across all known stores. Uses a local SQLite index.

```bash
himitsu search DB             # query the cached index
himitsu search DB --refresh   # rebuild index first, then query
```

### `himitsu recipient add|rm|ls`

```bash
# Add yourself
himitsu recipient add laptop --self

# Add a teammate by age public key
himitsu recipient add alice --age-key "age1abc..." --group common

# Remove
himitsu recipient rm alice --group common

# List all recipients
himitsu recipient ls
```

### `himitsu group add|rm|ls`

```bash
himitsu group add ops
himitsu group ls
himitsu group rm temp   # 'common' is reserved and cannot be removed
```

### `himitsu remote push|pull|status`

Thin wrappers around `git` for the store directory.

```bash
himitsu remote push     # git push the store
himitsu remote pull     # git pull the store
himitsu remote status   # git status of the store
```

### `himitsu sync [env]`

Sync secrets from a remote store into the local store.

```bash
himitsu sync          # sync all environments
himitsu sync prod     # sync one environment
```

### `himitsu codegen`

Generate typed declarations from the secret store.

```bash
himitsu codegen --lang typescript --env prod --stdout
himitsu codegen --lang golang     --env dev  --output ./secrets/types.go
himitsu codegen --lang python     --merge-common
```

Supported languages: `typescript`, `golang`, `python`, `rust`.

### `himitsu schema dump|dump-all|refresh|list`

Export JSON schemas describing the store's secret structure.

```bash
himitsu schema list           # list available schema names
himitsu schema refresh        # write all schemas to .himitsu/schemas/
himitsu schema dump secrets   # print one schema to stdout
himitsu schema dump-all       # print all schemas as a JSON object
```

### `himitsu git [args...]`

Run any `git` command inside the himitsu store directory.

```bash
himitsu git status
himitsu git log --oneline
himitsu git push
```

## Global Options

| Flag | Description |
|------|-------------|
| `-s, --store <path>` | Override the store directory (default: `$GIT_ROOT/.himitsu/` or `~/.himitsu/`). |
| `-v, --verbose` | Increase log verbosity. `-v` = debug, `-vv` = trace. |

## Nix Integration

The flake exposes a library for downstream consumers:

```nix
{
  inputs.himitsu.url = "github:darkmatter/himitsu";

  outputs = { self, nixpkgs, himitsu, ... }: let
    system = "x86_64-linux";
    lib = himitsu.lib.${system};
  in {
    # Wrap any devShell with automatic secret injection
    devShells.default = lib.mkDevShell {
      devShell = pkgs.mkShell { packages = [ nodejs ]; };
      store    = ./.himitsu;
      env      = "dev";
    };

    # Package encrypted secrets into a derivation
    packages.my-secrets = lib.packSecrets ./.himitsu/secrets/prod;
  };
}
```

**Flake outputs:**

| Output | Description |
|--------|-------------|
| `packages.default` / `packages.himitsu` | The `himitsu` CLI binary. |
| `packages.age-key-cmd` | Shell script that prints the local age private key. Useful as `SOPS_AGE_KEY_CMD`. |
| `lib.mkDevShell` | Wrap a devShell with automatic secret decryption on entry. |
| `lib.packSecrets` | Collect `.age` files into a Nix derivation. |
| `lib.wrapAge` | `age` binary pre-configured with the local identity. |
| `lib.wrapSops` | `sops` binary pre-configured to discover the himitsu key. |
| `lib.mkEntrypoint` | Container entrypoint that decrypts secrets then execs. |

## Development

```bash
nix develop                                              # enter dev shell

cargo build                                              # debug build
cargo build --release                                    # release build
cargo fmt --all -- --check                               # format check (CI gate)
cargo clippy --workspace --all-targets -- -D warnings    # lint (CI gate)
cargo test --workspace                                   # all tests

# Re-record the demo (requires release build)
vhs demo/demo.tape
```

## License

MIT
