# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `himitsu migrate envs [--dry-run]`: one-shot migration command that:
- Folds `environment` proto field into `tags` for every secret in the store
- Rewrites `.himitsu.yaml` `envs:` → `codegen:` with selector translation
- Removes the legacy env-cache SQLite file
- Creates `.himitsu.yaml.bak` backup before rewriting
- Auto-fold-on-read: secrets with the legacy `environment` proto field set will have that value automatically folded into `tags` on decode (non-mutating — the on-disk file is not modified by reads alone).
- TUI path expansion now accepts the `ctrl+x ctrl+=` leader chord

### Deprecated
- Proto fields `SecretEntry.environment`, `SecretEnvelope.environment`, `StoreManifest.environments`: marked `[deprecated = true]`. Writing these fields is disabled. A follow-up release will replace them with `reserved`. Rollback to a pre-migration binary is NOT supported once secrets have been re-encrypted by the new binary.

### Fixed
- `himitsu exec` now correctly exits 1 when a selector matches no secrets.