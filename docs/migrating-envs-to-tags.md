# Migrating from `envs:` to `codegen:` and Tags

## Overview

himitsu previously used an "environment namespace" concept (`envs:` block in `.himitsu.yaml` and an `environment` field in each secret's encrypted envelope). This has been replaced with a tag-based `codegen:` block that uses the existing free-form tag system.

The automated migration tool handles the full conversion:

```bash
himitsu migrate envs --dry-run  # preview changes
himitsu migrate envs             # apply changes
```

## Breaking Changes

| Old | New |
|-----|-----|
| `envs:` key in `.himitsu.yaml` | `codegen:` key (a legacy `envs:` block is tolerated with a warning; run `himitsu migrate envs` to convert) |
| `himitsu exec pci-prod -- cmd` | `himitsu exec tag:pci+tag:prod -- cmd` |
| `himitsu generate --env prod` | `himitsu generate --output prod` |
| `exec` silently launches with 0 secrets | `exec` exits 1 with error if selector matches nothing |

## How to Migrate

### Step 1: Run the migration tool

```bash
# Preview what will change (no files modified):
himitsu migrate envs --dry-run

# Apply the migration:
himitsu migrate envs
```

The tool will:
1. Walk every `.age` file in your store.
2. If the encrypted secret has an `environment` field set, decrypt it, add the value to `tags`, and re-encrypt without the `environment` field.
3. Rewrite `.himitsu.yaml`: rename `envs:` → `codegen:`, translate entries to the new `selectors:` / `aliases:` format.
4. Create a `.himitsu.yaml.bak` backup before rewriting.
5. Remove the legacy env-cache SQLite file.

### Step 2: Update your scripts

Replace any `himitsu exec <env-name>` calls with `himitsu exec tag:<name>` (or `tag:A+tag:B` for multi-tag filtering).

Replace `himitsu generate --env <name>` with `himitsu generate --output <name>`.

### New `codegen:` YAML format

The `codegen:` block in `.himitsu.yaml` defines named environment presets.

```yaml
codegen:
  # Named output 'pci-prod'
  pci-prod:
    selectors:
      # Match secrets carrying BOTH tags
      - tag:pci+tag:prod
    aliases:
      # Explicitly include a secret by tag and rename its env var
      STRIPE: tag:stripe

  # Brace expansion for multiple environments
  web-service-{dev,staging,prod}:
    selectors:
      # Glob matching
      - common/*
      # Positional capture from brace expansion
      - $1/database-url
    aliases:
      # Explicit path alias
      SOME_VALUE: path/to/some-secret
```

## Rollback Policy

**Rollback to a pre-migration binary is NOT supported** once any secrets have been re-encrypted by the new binary. The `environment` proto field will be empty on re-encrypted secrets, which the old binary cannot reconstruct.

If you need to preserve access with the old binary, run `himitsu migrate envs --dry-run` first to review the changes and only apply when you are ready to fully cut over.

## Manual Migration (Advanced)

If you prefer not to use the automated tool, you can:

1. Manually add tags to secrets: `himitsu tag <path> add <env-name>` for each secret.
2. Rename `envs:` → `codegen:` in `.himitsu.yaml` and adjust the format per the new schema above.
