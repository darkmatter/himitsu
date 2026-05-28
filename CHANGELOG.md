# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed (BREAKING)

- **Removed `envs:` block** from `.himitsu.yaml`. The `envs:` key now produces a hard error directing users to run `himitsu migrate envs`. See [Migration Guide](docs/migrating-envs-to-tags.md).
- **`himitsu exec`** now accepts a tag-selector grammar for the `<REF>` argument:
  - `tag:NAME`: all secrets tagged "NAME"
  - `tag:A+tag:B`: secrets tagged A AND B
  - `prod/*+tag:pci`: secrets under prod/* AND tagged pci
  - `tag:A,tag:B`: secrets tagged A OR B (OR via comma)
  - Old named-env dispatch (e.g. `exec pci-prod`) is removed; use `exec tag:pci+tag:prod` instead.
- **`himitsu exec`** exits 1 with `error: selector 'X' matched no secrets` when no secrets match (previously silently launched subprocess).
- **`himitsu generate --env`** flag removed; use `--output` instead. Passing `--env` now errors.
- **`outputs:` block** replaces `envs:` in `.himitsu.yaml`. Migration tool: `himitsu migrate envs`.

### Added

- `himitsu migrate envs [--dry-run]`: one-shot migration command that:
  - Folds `environment` proto field into `tags` for every secret in the store
  - Rewrites `.himitsu.yaml` `envs:` → `outputs:` with selector translation
  - Removes the legacy env-cache SQLite file
  - Creates `.himitsu.yaml.bak` backup before rewriting
- Auto-fold-on-read: secrets with the legacy `environment` proto field set will have that value automatically folded into `tags` on decode (non-mutating — the on-disk file is not modified by reads alone).

### Fixed

- `himitsu exec` now correctly exits 1 when a selector matches no secrets.

### Known Issues

- Demo tapes (`demo/*.tape`) reference the old `envs:` commands and need re-recording in a follow-up PR.

### Deprecated

- Proto fields `SecretEntry.environment`, `SecretEnvelope.environment`, `StoreManifest.environments`: marked `[deprecated = true]`. Writing these fields is disabled. A follow-up release will replace them with `reserved`. Rollback to a pre-migration binary is NOT supported once secrets have been re-encrypted by the new binary.
