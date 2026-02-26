# Architecture

## Directory Structure

```
.meta/himitsu/                      # Default directory for himitsu configuration
├── .keys/                          # Git-ignored directory
│   └── age.txt                     # SOPS_AGE_KEY_CMD checks this file
├── vars/                           # Encrypted variable files per environment
│   ├── common.sops.json            # Variables shared across all environments
│   ├── dev.sops.json               # Development environment variables
│   └── prod.sops.json              # Production environment variables
├── recipients/                     # Age/SSH/GPG recipient keys
│   ├── team/
│   │   └── label.age               # Uses extension to determine type
│   ├── admins/
│   └── master.age                  # Files or directories supported
├── .sops.yaml                      # SOPS configuration
├── .himitsu.yaml                   # Global defaults configuration
└── data.json                       # Application and group configuration
```

**Note:** `.keys/` directory is git-ignored to protect sensitive key material.

## Configuration Files

### `data.json`

Defines applications and access groups:

```jsonc
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
      "apps": ["web"]                 // Optional, defaults to all apps
    },
    "admins": {
      "groups": ["dev", "prod"]
    }
  }
}
```

### Variable Storage

Variables are stored under the app key (e.g., `web`) in environment-specific files like `dev.sops.json` and `prod.sops.json`.

## Commands

### Core Commands

#### `himitsu set <group> <key> <value>`
Set a variable in a specific group.

#### `himitsu group`
Manage access groups.

- **`add <name>`** - Create a new group
- **`rm <name>`** - Remove a group

**Note:** The `common` group is reserved, always exists, and contains the union of all recipients.

#### `himitsu recipient`
Manage recipients (users with access to secrets).

- **`add <name>`** - Add a recipient
- **`rm <name>`** - Remove a recipient

**Default behavior:** Adds self using age key if it doesn't exist.

**Options:**
- `--self` - Add self if not exists (default)
- `--label <label>` - Set a label for the recipient
- `--description <desc>` - Add a description
- `--type <age|ssh|gpg>` - Specify key type
- `--ssh-path <path>` - Path to SSH key
- `--gpg <id>` - GPG key ID
- `--age-key <key>` - Age key string

### Operations

#### `himitsu sync`
Synchronize secrets with GitHub and update encryption keys.

**Actions:**
1. Ensures GitHub has correct secrets
2. Calls `publish`
3. Runs `sops updatekeys <file>` for each encrypted file
4. Adds all GitHub collaborators to recipients using their published keys

#### `himitsu ci`
CI/CD validation command.

**Actions:**
1. Runs `sops updatekeys` on all `vars/*.sops.json` files
2. Ensures SOPS can decrypt all groups
3. Adds all GitHub collaborators to recipients

#### `himitsu codegen <language> <target-path>`
Generate code from encrypted variables for use in applications.

**TypeScript example:**
```bash
sops decrypt vars.sops.yaml --extract '["service"]' --output-type json > gen/vars/data.json
quicktype -l ts > gen/vars/index.ts
```

**Result:** Creates an importable package at the specified path with type-safe access to variables.

#### `himitsu encrypt` / `himitsu decrypt`
Utility commands to encrypt/decrypt all files.

**Behavior:**
- Places a `.<ext>` file next to `.sops.<ext>` files and vice versa
- Can be used to quickly encrypt/decrypt entire variable sets

## Code Generation

When `himitsu codegen` is run, it processes the configuration:

**Example for `dev` environment:**
```bash
# Merge common and environment-specific variables
sops decrypt common.sops.json > gen/vars/dev.json
sops decrypt dev.sops.json >> gen/vars/dev.json

# Generate TypeScript types
quicktype -l ts -o packages/gen/vars/index.ts gen/vars/dev.json
```

This process is repeated for each environment (dev, prod, etc.).

## Group Publishing

**Example:** `himitsu group push dev`

Uploads the group's encrypted variables to GitHub Actions secrets.

## Key Management

### Key Types Supported
- **Age** - Modern encryption tool (recommended)
- **SSH** - Uses SSH keys for encryption
- **GPG** - Traditional PGP encryption

### Key Discovery
- Recipients are identified by file extension (`.age`, `.ssh`, `.gpg`)
- Both directories and files are supported in `recipients/`
- A file is treated like a directory with one recipient

### Security Model
- Private keys stored in `.keys/` (git-ignored)
- Public keys stored in `recipients/` (version controlled)
- SOPS manages encryption/decryption using these keys