# himitsu

SOPS-based secrets management with group recipient control. Wraps [sops](https://github.com/getsops/sops) and [age](https://github.com/FiloSottile/age) to provide team-friendly encrypted variable management with automatic key rotation, GitHub collaborator sync, and typed codegen.

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
```

## Quick Start

```bash
# Initialize in your repo
himitsu init

# Add yourself as a recipient
himitsu recipient add --self --group team

# Create a group and map it to environments
# (edit .meta/himitsu/data.json to configure group -> env mappings)

# Add a secret
himitsu set dev DB_HOST localhost

# Encrypt / decrypt all var files
himitsu encrypt
himitsu decrypt

# After adding/removing recipients, sync everything
himitsu sync
```

## Directory Layout

```
.meta/himitsu/
  .keys/
    age.txt              # Your local age secret key (gitignored)
  vars/
    common.sops.json     # Shared across all environments
    dev.sops.json        # Dev environment secrets
    prod.sops.json       # Prod environment secrets
  recipients/
    team/
      alice.age          # Age public key
      bob.ssh            # SSH public key
    admins/
      carol.age
    master.age           # Standalone recipient (like a 1-member group)
  .sops.yaml             # Auto-generated from recipients/
  .himitsu.yaml          # Config overrides
  data.json              # Group -> environment + app mappings
```

## Commands

### `himitsu init`

Scaffold a new himitsu directory with keypair, config, and directory structure.

### `himitsu group add|rm|ls <name>`

Manage recipient groups. The `common` group is reserved and cannot be removed.

### `himitsu recipient add|rm|ls [name]`

Manage recipients within groups.

```bash
# Add yourself (generates/uses local age key)
himitsu recipient add --self --group team

# Add someone by age public key
himitsu recipient add deploy-bot --age-key "age1..." --group admins

# Add by SSH key
himitsu recipient add alice --ssh-path ~/.ssh/id_ed25519.pub --group team

# Add by GPG key ID
himitsu recipient add bob --gpg "ABCD1234" --group team

# Remove
himitsu recipient rm alice --group team
```

### `himitsu sync`

Regenerate `.sops.yaml`, run `sops updatekeys` on all var files, fetch GitHub collaborator SSH keys and add them as recipients.

```bash
himitsu sync                # Standard sync
himitsu sync --push-secrets # Also push decrypted values as GitHub Actions secrets
```

### `himitsu ci`

Designed for CI environments. Validates recipient state, adds new GitHub collaborators, and auto-commits changes.

```bash
himitsu ci             # Auto-commit if changes found
himitsu ci --check     # Fail if state is out of date (no modifications)
himitsu ci --no-commit # Apply fixes but don't commit
```

### `himitsu codegen [language] [path]`

Generate typed config from decrypted vars. Without arguments, reads codegen targets from `data.json`.

```bash
himitsu codegen ts packages/gen/vars
himitsu codegen       # Uses data.json app config
```

### `himitsu encrypt` / `himitsu decrypt`

Bulk encrypt plaintext `vars/*.json` to `vars/*.sops.json`, or decrypt the reverse.

### `himitsu set <group> <key> <value>`

Set a key-value pair in a group's encrypted sops file.

## Configuration

### `data.json`

Maps groups to environments and apps to codegen targets:

```json
{
  "apps": {
    "web": {
      "codegen": {
        "path": "packages/gen/vars",
        "language": "ts"
      }
    }
  },
  "groups": {
    "team": {
      "groups": ["dev"],
      "apps": ["web"]
    },
    "admins": {
      "groups": ["dev", "prod"]
    }
  }
}
```

### `.himitsu.yaml`

Override default directory names:

```yaml
keys_dir: ".keys"
vars_dir: "vars"
recipients_dir: "recipients"
```

## GitHub Action

Use himitsu as a GitHub Action for self-serve recipient management:

```yaml
# .github/workflows/himitsu.yml
on:
  pull_request:
    paths: [".meta/himitsu/recipients/**"]
  workflow_dispatch:
    inputs:
      operation:
        type: choice
        options: [ci, add-recipient, rm-recipient, sync]
      recipient-name:
        type: string
        required: false

jobs:
  himitsu:
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
      - uses: darkmatter/himitsu@main
        with:
          operation: ${{ github.event.inputs.operation || 'ci' }}
          recipient-name: ${{ github.event.inputs.recipient-name }}
          sops-age-key: ${{ secrets.SOPS_AGE_KEY }}
```

### Action Inputs

| Input | Description | Default |
|-------|-------------|---------|
| `operation` | `ci`, `sync`, `add-recipient`, `rm-recipient`, `codegen` | `ci` |
| `recipient-name` | Label for add/rm operations | |
| `recipient-key` | Public key value | |
| `recipient-type` | `age`, `ssh`, or `gpg` | `age` |
| `group` | Target group | `team` |
| `sops-age-key` | Age secret key for decryption | |
| `himitsu-dir` | Path to himitsu directory | `.meta/himitsu` |
| `auto-commit` | Commit changes automatically | `true` |
| `github-token` | Token for API access | `${{ github.token }}` |

### Self-Serve Flows

**PR-based**: A developer adds their `.age` key file to `recipients/<group>/`, opens a PR, and CI validates + runs `sops updatekeys`.

**workflow_dispatch**: A developer triggers the action with their key, and the action handles file creation, re-encryption, and commit.

## Development

```bash
# Enter dev shell with all dependencies
nix develop

# Run tests
bats tests/bats/

# Check scripts
shellcheck src/bin/himitsu src/lib/*.sh
```

## License

MIT
