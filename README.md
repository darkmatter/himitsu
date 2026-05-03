<center>
  <h1>himitsu<sup>秘密</sup></h1>
</center>

Age-based secrets manager that supports cross-repo sharing. Secrets stored one-file-per-key in a git-backed `.himitsu/` store, with path-based recipient control and typed codegen.

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
- [Configuration](#configuration)
- [Sync & Store Health](#sync--store-health)
- [Demo & Recordings](#demo--recordings)
- [Nix Integration](#nix-integration)
- [Development](#development)
- [License](#license)

---

## Intro

himitsu solves duplication and eases maintenance, and is made for the following workflow:

- Create your personal store as a git repo e.g. `user/secrets` - personal secrets go here.
- Any orgs/teams you are part of have a store at `org/secrets` - team secrets go here.
- Access-control is path-based - configure who can access `prod/*` in `.himitsu.yaml`
- Store secrets, organizing using plain directories
- Map secrets to environments:

```yaml
# .himitsu.yaml
...
envs:
  web-service-{dev,staging,prod}:
    # includes all secrets in the "common" directory, converting 'foo-bar' to 'FOO_BAR'
    - common/*
    # includes dev/database-url for dev, etc
    - $1/database-url
    # override environment variable key
    - SOME_VALUE: path/to/some-secret
    # use the full ref to access external stores
    - SHARED_SECRET: github:org/secrets#prod/api-key
```

With this config you can run `himitsu generate --target gen` which will build SOPS-compatible yaml files to the `gen/` directory.


## Features

- **Age-only encryption** -- no KMS, no GPG, no SOPS. One `.age` file per secret.
- **One file per secret** -- `.himitsu/secrets/<env>/<KEY>.age` keeps diffs readable.
- **Path-based recipients** -- organize keys into directories (e.g. `ops/alice`, `team/*`); re-encrypt for all with `rekey`.
- **Typed codegen** -- generate TypeScript, Go, Python, or Rust type stubs from your secret store.
- **Cross-store search** -- `himitsu search` reads every known store directly; results are live, no index to rebuild.
- **TUI** -- in-terminal dashboard with fuzzy search, secret viewer, inline editing, and store health.
- **Import / Export** -- bulk import from 1Password (`op`) or SOPS files; export as SOPS-encrypted YAML/JSON.
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

The fastest path is the TUI. Run `himitsu` with no subcommand: if no age key
exists yet, the init wizard launches automatically and walks you through
scaffolding a store. Once that completes you land on the search dashboard.

```bash
himitsu          # opens the TUI (init wizard on first run, dashboard otherwise)
```

From the dashboard you can fuzzy-search secrets, `ctrl-n` to create one,
`ctrl-p` for the command palette, `enter` to view, `e` to edit -- see
[TUI](#tui) for the full keymap.

### CLI / scripting

The same operations are available as scriptable subcommands. Use these in CI,
shell scripts, or when you'd rather stay in your editor:

```bash
# Initialize headlessly (skip the wizard)
himitsu init --name you/secrets

# Store and read secrets
himitsu set prod/API_KEY     "sk_live_abc123"
himitsu set prod/DB_PASSWORD "hunter2"
himitsu set dev/DB_PASSWORD  "devpass" --no-push   # batch-friendly
himitsu get prod/API_KEY                            # -> sk_live_abc123

# List, rekey, search
himitsu ls                # -> dev/, prod/
himitsu ls prod           # -> API_KEY, DB_PASSWORD
himitsu rekey             # re-encrypt for current recipient set
himitsu search DB         # cross-store fuzzy search
```

Override the active store with `-s` (path) or `-r` (slug):

```bash
himitsu -s /path/to/.himitsu set prod/API_KEY "sk_live_xxx"
himitsu -r org/repo get prod/API_KEY
```

## TUI

Launch the interactive terminal UI:

```bash
himitsu        # no subcommand opens the TUI
```

Search is the **root view** -- the app opens straight into a fuzzy filter over every secret in the active store. Start typing to narrow the list, arrow keys to move, `enter` to open. Press `?` in any view for a help overlay, or `ctrl-p` to open the **command palette** -- the canonical, fuzzy-filterable list of every action the current view exposes.

![browse and drill](demo/tui-us-011.gif)

### Search (root)

| Key | Action |
|-----|--------|
| _type_ | filter results |
| `up` / `down` | move selection |
| `enter` | open selected secret |
| `backspace` | delete filter char |
| `ctrl-p` | command palette |
| `ctrl-n` | new secret |
| `ctrl-s` | switch store |
| `ctrl-y` | copy selected value |
| `shift-e` | browse env presets |
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
himitsu init                              # interactive TUI wizard
himitsu init --name you/secrets           # headless, creates/restores a primary store
himitsu init --name org/repo --url <url>  # restore from a custom git remote
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

Fuzzy-search secret paths across every known store. Results are read live
from store files on every invocation -- there is no SQLite index to rebuild,
and decrypted descriptions are pulled best-effort with the ambient age key.

```bash
himitsu search DB             # search all stores
himitsu search DB --refresh   # accepted as a no-op; kept for backward compat
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

See [Sync & Store Health](#sync--store-health) for the auto-commit / auto-push
behavior, `--no-push`, and the `auto_pull` config switch.

### `himitsu import`

Import secrets from 1Password (`--op`) or a SOPS-encrypted file (`--sops`).
Both backends shell out to external tools, so they must be on `PATH`:

- **1Password** -- requires the [`op`](https://developer.1password.com/docs/cli/)
  CLI, signed in to the relevant account.
- **SOPS** -- requires [`sops`](https://github.com/getsops/sops); decryption
  runs as `sops -d <file>`.

```bash
himitsu import prod/STRIPE_KEY --op "op://Engineering/Stripe/api_key"
himitsu import prod          --op "op://Engineering/Stripe"   # whole item
himitsu import prod          --sops secrets.enc.yaml --dry-run
```

### `himitsu export`

Export secrets matching a glob as a SOPS-encrypted file. Requires `sops` on
`PATH`; encryption is piped through `sops --encrypt` so plaintext never hits
disk.

```bash
himitsu export "prod/*" -o prod.sops.yaml
```

### `himitsu generate`

Generate SOPS-encrypted output files from env definitions in project config.
Also requires `sops` on `PATH` -- same pipe-through-stdin contract as
`export`.

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

## Configuration

User-level settings live at `~/.config/himitsu/config.yaml`. Every field has a
`HIMITSU_<FIELD>` environment override (uppercased, dots become underscores)
that takes precedence over the file.

```yaml
# Default store when neither -s nor -r is given.
default_store: myorg/secrets        # env: HIMITSU_DEFAULT_STORE

# Where age private keys live: "disk" or "macos-keychain".
key_provider: disk                  # env: HIMITSU_KEY_PROVIDER

# When true, every store-touching command first runs `git fetch` and
# fast-forwards before dispatching, so reads see latest state and writes
# can't fast-fail on a remote-side commit. Failures are non-fatal.
auto_pull: false                    # env: HIMITSU_AUTO_PULL

tui:
  # Built-in palette. `random` (the default) picks one per launch.
  # Accepted: random, himitsu, apathy, apathy-minted, apathy-theory,
  #           apathy-storm, ayu, catppuccin, material, rose-pine.
  theme: random                     # env: HIMITSU_TUI_THEME

  # Opt in to Nerd Font glyphs in the dashboard. Off by default because
  # there is no reliable runtime check for font support.
  nerd_fonts: false

  # Per-action keybindings. Each action takes a list, so multiple keys can
  # trigger the same action. Unspecified actions fall back to the
  # hardcoded defaults documented in [TUI](#tui).
  keys:
    new_secret: ["F2", "ctrl+n"]
    quit:       ["esc", "ctrl+q"]
```

Binding strings are `<mod>+<mod>+<code>`, lowercased, modifiers first --
e.g. `"ctrl+n"`, `"shift+tab"`, `"esc"`, `"?"`, `"F2"`. Uppercase
characters imply `shift` (`"Y"` == `"shift+y"`). Bare letters match
case-insensitively, so `"y"` matches both `y` and `Y`. Malformed bindings
surface as a clear config error at startup.

The full action list (with defaults) is in
[`rust/src/tui/keymap.rs`](rust/src/tui/keymap.rs):
`quit`, `help`, `command_palette`, `new_secret`, `switch_store`,
`copy_selected`, `envs`, `reveal`, `copy_value`, `rekey`, `edit`, `delete`,
`back`, `save_secret`, `next_field`, `prev_field`, `cancel`.

## Sync & Store Health

Himitsu treats the store as an append-only git repo and keeps `git status`
clean for you.

- **Auto-commit on every mutation.** `set`, `write`, `rekey`, `import`,
  `recipient add/rm`, etc. each produce a commit (`himitsu: <action>`) on
  success or `himitsu: FAILED: <action>: <error>` on failure -- the working
  tree is never left dirty.
- **Auto-push on success.** When the commit lands and a remote is
  configured, himitsu also runs `git push`. Pass `--no-push` on `set`,
  `write`, or `import` to skip the push (handy for batch loads); the next
  mutation without `--no-push` will flush everything.
- **Auto-pull (opt-in).** Set `auto_pull: true` (or `HIMITSU_AUTO_PULL=1`)
  to fetch and fast-forward before every store-touching command. Failures
  surface as a stderr warning rather than aborting the command.

The TUI dashboard renders a store-health indicator in the header bar with
the following states:

| State          | Meaning                                                              |
|----------------|----------------------------------------------------------------------|
| `synced`       | Local checkout matches the remote tracking branch.                   |
| `behind N`     | Tracking branch is ahead of local by N commits -- run `himitsu sync`.|
| `dirty`        | Working tree has uncommitted changes (rare; usually a manual edit).  |
| `behind+dirty` | Both behind remote AND has local changes.                            |
| `no remote`    | Repo exists but no remote -- run `himitsu remote add <slug>`.        |
| `not pushed`   | Remote configured, tracking branch missing -- run `himitsu git push -u origin main`. |
| `not git`      | Store directory is not a git repo (init bug, almost never happens).  |
| `unknown`      | Status couldn't be determined; treat as a hint to investigate.       |

## Demo & Recordings

The headline demo at the top of this README is `demo/demo-vhs.gif`,
rendered from `demo/demo.tape`. The smaller `demo/tui-us-*.{tape,gif,cast}`
files are per-user-story regression / demo artifacts -- one per shipped TUI
story (US-008 new-secret form, US-011 browse, US-012 viewer, etc.) -- and
double as visual tests that the documented flow still works.

To regenerate locally:

```bash
cargo build --release
vhs demo/demo.tape                 # canonical headline GIF
vhs demo/tui-us-011.tape           # one specific story
```

CI re-renders every tape on changes to `demo/**`, `rust/**`, or the
workflow file (`.github/workflows/vhs.yml`). The CI run redirects each
tape's `Output` line to a scratch path under `target/vhs-out/` so the
checkout never picks up binary diffs; the rendered GIFs are uploaded as a
build artifact (`vhs-demo-renders`) for inspection. Commit a refreshed
`demo-vhs.gif` only when you intend the README hero to change.

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
