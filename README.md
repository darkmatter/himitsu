<center>
  <h1>himitsu<sup>秘密</sup></h1>
</center>

Age-based secrets management. Secrets are encrypted with [age](https://github.com/FiloSottile/age) x25519 keys, stored one-file-per-key in a git-backed `.himitsu/` store, with path-based recipient control and typed codegen.

![himitsu demo](demo/demo-vhs.gif)

---

## Table of Contents

- [Features](#features)
- [Install](#install)
- [Quick Start](#quick-start)
- [TUI](#tui)
- [Store Layout](#store-layout)
- [Commands](#commands)
  - [init](#himitsu-init)
  - [set](#himitsu-set-path-value)
  - [get](#himitsu-get-path)
  - [ls](#himitsu-ls-prefix)
  - [rekey](#himitsu-rekey-prefix)
  - [search](#himitsu-search-query)
  - [recipient](#himitsu-recipient-addrmlsshow)
  - [remote](#himitsu-remote-addremovelistdefault)
  - [sync](#himitsu-sync-env)
  - [import](#himitsu-import)
  - [export](#himitsu-export)
  - [generate](#himitsu-generate)
  - [git](#himitsu-git-args)
  - [docs](#himitsu-docs)
- [Global Options](#global-options)
- [Nix Integration](#nix-integration)
- [Development](#development)
- [License](#license)

---

## Features

- **Age-only encryption** -- no KMS, no GPG, no SOPS. One `.age` file per secret.
- **One file per secret** -- `.himitsu/secrets/<env>/<KEY>.age` keeps diffs readable.
- **Path-based recipients** -- organize keys into directories (e.g. `ops/alice`, `team/*`); re-encrypt for all with `rekey`.
- **Typed codegen** -- generate TypeScript, Go, Python, or Rust type stubs from your secret store.
- **Cross-store search** -- `himitsu search` queries a local SQLite index across all known stores.
- **TUI** -- in-terminal dashboard with fuzzy search, secret viewer, inline editing, and store health.
- **Import / Export** -- bulk import from 1Password or SOPS; export as SOPS-encrypted YAML/JSON.
- **Nix integration** -- flake library for devShell injection, secret packaging, and container entrypoints.

## Install

```bash
# Nix (run directly)
nix run github:darkmatter/himitsu -- <command>

# Nix (add to a devShell)
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
# 1. Initialize -- creates age keypair and scaffolds the store
himitsu init

# 2. Store secrets
himitsu set prod/API_KEY     "sk_live_abc123"
himitsu set prod/DB_PASSWORD "hunter2"
himitsu set dev/DB_PASSWORD  "devpass"

# 3. Read them back
himitsu get prod/API_KEY      # -> sk_live_abc123
himitsu get dev/DB_PASSWORD   # -> devpass

# 4. List secrets
himitsu ls        # -> dev/, prod/
himitsu ls prod   # -> API_KEY, DB_PASSWORD

# 5. Re-encrypt for all current recipients
himitsu rekey

# 6. Search across all stores
himitsu search DB --refresh
```

Override the store with `-s` or `-r`:

```bash
himitsu -s /path/to/.himitsu set prod/API_KEY "sk_live_xxx"
himitsu -r org/repo get prod/API_KEY
```

## TUI

Launch the interactive terminal UI:

```bash
himitsu        # no subcommand opens the TUI
```

Search is the **root view** -- the app opens straight into a fuzzy filter over every secret in the active store. Start typing to narrow the list, arrow keys to move, `enter` to open. Press `?` in any view for a help overlay.

![browse and drill](demo/tui-us-011.gif)

### Search (root)

| Key | Action |
|-----|--------|
| _type_ | filter results |
| `up` / `down` | move selection |
| `enter` | open selected secret |
| `backspace` | delete filter char |
| `ctrl-n` | new secret |
| `ctrl-s` | switch store |
| `ctrl-y` | copy selected value |
| `?` | help |
| `esc` / `ctrl-c` | quit |

### Secret viewer

| Key | Action |
|-----|--------|
| `r` | reveal / hide value |
| `y` | copy to clipboard |
| `e` | edit in `$EDITOR` |
| `R` | rekey for current recipients |
| `d` | delete (confirms with `y`) |
| `?` | help |
| `esc` | back |

![secret viewer](demo/tui-us-012.gif)

### New-secret form

Fields: `path`, `value`, `description`, `url`, `totp`, `env_key`, `expires_at`.

| Key | Action |
|-----|--------|
| `tab` / `enter` | next field |
| `shift-tab` | previous field |
| `ctrl-s` / `ctrl-w` | save |
| `esc` / `ctrl-c` | cancel |

![create secret](demo/tui-us-008.gif)

## Store Layout

```
.himitsu/
  secrets/
    prod/
      API_KEY.age
      DB_PASSWORD.age
    dev/
      DB_PASSWORD.age
  recipients/
    self.pub                   # your key (added on init)
    ops/
      alice.pub                # path-based: ops/alice
      deploy-bot.pub           # path-based: ops/deploy-bot
  config.yaml                  # store-level config
  schemas/
    secrets.json
```

The keyring lives separately:

```
~/.local/share/himitsu/        # $XDG_DATA_HOME/himitsu
  key                          # age private key
  key.pub                      # age public key
```

## Commands

### `himitsu init`

Create an age keypair and scaffold the store. Adds `self.pub` as a recipient automatically.

```bash
himitsu init                   # interactive TUI wizard
himitsu init --name org/repo   # headless, registers a named store
```

### `himitsu set <path> <value>`

Encrypt and store a secret. Path is slash-delimited (`prod/API_KEY`).

```bash
himitsu set prod/API_KEY "sk_live_abc123"
himitsu set dev/DB_PASSWORD "devpass" --no-push
```

### `himitsu get <path>`

Decrypt and print a secret.

```bash
himitsu get prod/API_KEY
```

### `himitsu ls [prefix]`

Browse secrets like a directory.

```bash
himitsu ls         # -> dev/, prod/
himitsu ls prod    # -> API_KEY, DB_PASSWORD
```

### `himitsu rekey [prefix]`

Re-encrypt secrets for the current recipient set. Run after adding or removing recipients.

```bash
himitsu rekey         # everything
himitsu rekey prod    # one subtree
```

### `himitsu search <query>`

Search secret names across all known stores.

```bash
himitsu search DB             # cached index
himitsu search DB --refresh   # rebuild first
```

### `himitsu recipient add|rm|ls|show`

Manage recipients with path-based names. Slash-separated paths create a directory hierarchy (e.g. `ops/alice` -> `recipients/ops/alice.pub`).

```bash
himitsu recipient add laptop --self
himitsu recipient add ops/alice --age-key "age1abc..." --description "Alice"
himitsu recipient show ops/alice
himitsu recipient rm ops/alice
himitsu recipient ls
```

### `himitsu remote add|remove|list|default`

Manage remote store registrations.

```bash
himitsu remote add org/repo                           # clone from GitHub
himitsu remote add git@github.com:org/repo.git        # full URL also works
himitsu remote add org/repo --url https://custom/url   # custom git host
himitsu remote default org/repo                        # set default store
himitsu remote list
himitsu remote remove org/repo
```

### `himitsu sync [env]`

Pull from the git remote and optionally rekey drifted secrets.

```bash
himitsu sync          # all environments
himitsu sync prod     # one environment
```

### `himitsu import`

Import secrets from 1Password or SOPS files.

```bash
himitsu import --from 1password --vault "Engineering"
himitsu import --from 1password --item "API Keys"
himitsu import --from sops secrets.enc.yaml --dry-run
```

### `himitsu export`

Export secrets matching a glob as a SOPS-encrypted file.

```bash
himitsu export "prod/*" -o prod.sops.yaml
```

### `himitsu generate`

Generate SOPS-encrypted output files from env definitions in project config.

```bash
himitsu generate           # all envs defined in himitsu.yaml
himitsu generate --env prod
```

### `himitsu git [args...]`

Run any git command inside the store directory.

```bash
himitsu git status
himitsu git log --oneline
himitsu git --all status       # all stores
```

### `himitsu docs`

Render this README in the terminal.

```bash
himitsu docs
```

## Global Options

| Flag | Description |
|------|-------------|
| `-s, --store <path>` | Override the store path directly. |
| `-r, --remote <slug>` | Select a store by `org/repo` slug (or full git URL). |
| `-v, --verbose` | Increase log verbosity (`-v` debug, `-vv` trace). |

## Nix Integration

```nix
{
  inputs.himitsu.url = "github:darkmatter/himitsu";

  outputs = { self, nixpkgs, himitsu, ... }: let
    system = "x86_64-linux";
    lib = himitsu.lib.${system};
  in {
    devShells.default = lib.mkDevShell {
      devShell = pkgs.mkShell { packages = [ nodejs ]; };
      store    = ./.himitsu;
      env      = "dev";
    };

    packages.my-secrets = lib.packSecrets ./.himitsu/secrets/prod;
  };
}
```

| Output | Description |
|--------|-------------|
| `packages.default` | The `himitsu` CLI binary. |
| `packages.age-key-cmd` | Prints the local age private key (useful as `SOPS_AGE_KEY_CMD`). |
| `lib.mkDevShell` | Wrap a devShell with automatic secret decryption. |
| `lib.packSecrets` | Collect `.age` files into a Nix derivation. |
| `lib.wrapAge` | `age` pre-configured with the local identity. |
| `lib.wrapSops` | `sops` pre-configured to discover the himitsu key. |
| `lib.mkEntrypoint` | Container entrypoint that decrypts then execs. |

## Development

```bash
nix develop                    # enter dev shell
cargo build                    # debug build
cargo build --release          # release build
cargo test --workspace         # all tests
cargo fmt --all -- --check     # format check
cargo clippy --workspace --all-targets -- -D warnings
```

## License

MIT
