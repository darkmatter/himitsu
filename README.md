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
himitsu --remote myorg/secrets git push
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

### `himitsu rekey [path]`

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

### `himitsu remote add|default|list|remove`

```bash
himitsu remote add myorg/secrets              # Clone existing
himitsu remote add myorg/secrets --url git@github.com:myorg/secrets.git

himitsu remote list                           # Show all registered stores
himitsu remote default myorg/secrets          # Set default store
himitsu remote default                        # Show current default
himitsu remote remove myorg/secrets           # Delete store checkout
```

### `himitsu sync [store]`

Pull all stores (or a specific one) from git remotes and re-encrypt any drifted secrets.

```bash
himitsu sync                    # Pull and rekey all stores
himitsu sync myorg/secrets      # Pull and rekey one store
himitsu sync --no-rekey         # Pull only, skip re-encryption
```

### Flake Outputs

The flake provides the following outputs:

- `packages.default` and `packages.himitsu` - The `himitsu` CLI binary.
- `packages.age-key-cmd` - A wrapper script that outputs the local `himitsu` age private key. Useful as a `SOPS_AGE_KEY_CMD`.
- `lib.mkEncryptedSecrets` - A Nix function to package a remote's encrypted `vars/` directory into a Nix derivation.
- `lib.mkDecryptWrapper` - A Nix function to create a wrapper script that decrypts packaged secrets using the provided `ageKeyCmd`.

Example usage of lib functions:

```nix
{
  inputs.himitsu.url = "github:darkmatter/himitsu";
  
  outputs = { self, nixpkgs, himitsu, ... }: {
    packages.x86_64-linux = {
      # Package your production secrets
      my-secrets = himitsu.lib.x86_64-linux.mkEncryptedSecrets {
        name = "my-prod-secrets";
        src = ./path/to/remote;
        env = "prod";
      };

      # Create a decryption script
      decrypt-my-secrets = himitsu.lib.x86_64-linux.mkDecryptWrapper {
        name = "decrypt-prod-secrets";
        secretsPkg = self.packages.x86_64-linux.my-secrets;
        destDir = "/run/secrets/decrypted";
        # Uses the local himitsu age-key-cmd by default
      };
    };
  };
}
```

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
