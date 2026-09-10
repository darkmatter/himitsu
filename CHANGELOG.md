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
- `himitsu set <path> [value]` (alias `add`) can now take the secret's value from a local file with `--file <path>`; exactly one of the literal `<value>` argument or `--file` is accepted. The file is read as raw bytes with no MIME or filename metadata recorded -- binary content and trailing whitespace roundtrip byte-for-byte via `himitsu read` / `himitsu get`.
- New public pure reference-grammar parser `reference::ReferenceExpr` (`parse(&str)`): recognizes `tag:<name>`, `path:<path>`, bare slash paths, unresolved bare names, trailing-slash path prefixes (`personal/`, equivalent to `personal/*`), provider-qualified refs (`github:acme/secrets#path`), and explicit `store:provider:org/repo` refs. Rejects malformed input via the existing validators, including org/repo traversal and extra slug segments in qualified/store refs (`config::validate_remote_slug`). Parser-only (hm-zn6): no IO, no store lookup, bare-name tag-vs-path ambiguity is retained for the future resolver, and `himitsu exec` / `himitsu codegen` behavior is unchanged — nothing consumes the new parser yet.
- New store-local tag-definition storage API `himitsu::remote::tag_store` (hm-cw1, storage-only): `TagDefinition { description, keys }` persisted at `<store>/.himitsu/tags/<name>.yaml` with `create_tag` (no-clobber), `read_tag` (re-validates on read), `update_tag`/`delete_tag` (missing tag is a `TagNotFound` error), and `list_tags` (sorted; empty when the tags directory is missing). Every function takes an explicit caller-supplied store root — no global-store resolution convention. `keys` are reference strings (never secret values), validated with `crypto::tags::validate_tag` for names (plus `.`/`..`/path-escape rejection) and `reference::ReferenceExpr::parse` for entries; writes are atomic (temp-file + rename). Storage only — no resolver, CLI, TUI, or codegen change; nothing consumes the API yet and `himitsu` runtime behavior is unchanged.

### Deprecated
- Proto fields `SecretEntry.environment`, `SecretEnvelope.environment`, `StoreManifest.environments`: marked `[deprecated = true]`. Writing these fields is disabled. A follow-up release will replace them with `reserved`. Rollback to a pre-migration binary is NOT supported once secrets have been re-encrypted by the new binary.

### Fixed
- `himitsu exec` now correctly exits 1 when a selector matches no secrets.