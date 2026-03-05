<center>
  <h1>himitsu<sup>秘密</sup></h1>
</center>

Age-based secrets management with transport-agnostic sharing. Encrypted secrets are stored as one file per key (`vars/<env>/<KEY>.age`) in git-backed remotes, with group-based recipient control and cross-remote search.

## Features

- **Age-only encryption** -- secrets are encrypted with [age](https://github.com/FiloSottile/age) x25519 keys. No KMS, no GPG.
- **One file per secret** -- `vars/<env>/<KEY>.age` keeps diffs simple and listing fast.
- **Group-based recipients** -- organize keys into groups (team, admins, devices) with per-path policies.
- **Cross-remote search** -- `himitsu search` finds secrets across all your remotes.
- **Transport-agnostic sharing** -- share secrets via GitHub PR inbox or Nostr (planned).
- **Typed codegen** -- generate TypeScript, Go, or Python config from your secrets (planned).

## Install

```bash
# Run directly
nix run github:darkmatter/himitsu -- <command>

# Add to devShell
{
  inputs.himitsu.url = "github:darkmatter/himitsu";
  # ...
  devShells.default = pkgs.mkShell {
    packages = [ himitsu.packages.${system}.default ];
  };
}

# Or build from source
cargo build --release
```

## Quick Start

```bash
# 1. Initialize himitsu (creates ~/.himitsu/ with age keys and config)
himitsu init

# 2. Create a remote to store secrets
himitsu remote add myorg/secrets

# 3. Add yourself as a recipient
himitsu -r myorg/secrets recipient add laptop --self --group team

# 4. Add secrets
himitsu -r myorg/secrets set prod API_KEY "sk_live_xxx"
himitsu -r myorg/secrets set prod DB_PASSWORD "hunter2"
himitsu -r myorg/secrets set dev DB_PASSWORD "devpass"

# 5. Read secrets back
himitsu -r myorg/secrets get prod API_KEY

# 6. List environments and keys
himitsu -r myorg/secrets ls          # lists: dev, prod
himitsu -r myorg/secrets ls prod     # lists: API_KEY, DB_PASSWORD

# 7. Search across all remotes
himitsu search DB

# 8. Push changes
himitsu -r myorg/secrets remote push
```

## Directory Layout

### Global state (`~/.himitsu/`)

```
~/.himitsu/
  config.yaml              # User config (default remote, etc.)
  keys/
    age.txt                # Your age private key
  data/
    <org>/<repo>/          # Remote clones
  state/
    index.db               # Cross-remote search index
  cache/
  locks/
```

### Remote layout (`~/.himitsu/data/<org>/<repo>/`)

```
himitsu.yaml               # Remote config (policies, identity sources)
data.json                  # Group/env metadata
vars/
  common/
    API_BASE_URL.age
  dev/
    DB_PASSWORD.age
  prod/
    DB_PASSWORD.age
recipients/
  team/
    alice.pub              # age public key
    bob.pub
  admins/
    root.pub
```

### Project binding (`<repo>/.himitsu.yaml`)

```yaml
remote: myorg/secrets

codegen:
  lang: typescript
  path: src/generated/config.ts
```

When `.himitsu.yaml` exists in a git repo, himitsu runs in **project mode** and uses the bound remote automatically (no `-r` flag needed).

## Commands

### `himitsu init`

Create `~/.himitsu/` with age keypair, config, and directory structure.

### `himitsu set <env> <key> <value>`

Encrypt and store a secret.

### `himitsu get <env> <key>`

Decrypt and print a secret value.

### `himitsu ls [env]`

List environments, or list keys within an environment.

### `himitsu encrypt [env]`

Re-encrypt all secrets for the current recipient set. Run this after adding or removing recipients.

### `himitsu search <query>`

Search key names across all remotes. Use `--refresh` to rebuild the index first.

### `himitsu recipient add|rm|ls`

```bash
# Add yourself
himitsu -r myorg/secrets recipient add laptop --self --group team

# Add someone by age public key
himitsu -r myorg/secrets recipient add deploy-bot --age-key "age1..." --group admins

# Remove
himitsu -r myorg/secrets recipient rm deploy-bot --group admins

# List
himitsu -r myorg/secrets recipient ls
```

### `himitsu group add|rm|ls`

```bash
himitsu -r myorg/secrets group add admins
himitsu -r myorg/secrets group ls
himitsu -r myorg/secrets group rm temp    # 'common' is reserved
```

### `himitsu remote add|push|pull|status`

```bash
himitsu remote add myorg/secrets              # Clone existing
himitsu remote add --github --org myorg --name secrets  # Create + clone

himitsu -r myorg/secrets remote push
himitsu -r myorg/secrets remote pull
himitsu -r myorg/secrets remote status
```

### `himitsu sync [env]`

Re-encrypt all secrets for the updated recipient set and sync to project destinations.

## Global Options

| Flag | Description |
|------|-------------|
| `-r <org/repo>` | Target remote. Overrides project binding and default remote. |
| `-v` | Increase log verbosity (`-v` debug, `-vv` trace). |

## Development

```bash
# Enter dev shell
nix develop

# Build
cargo build

# Run tests
cargo test

# Lint
cargo clippy -- -D warnings
cargo fmt -- --check
```

## License

MIT
