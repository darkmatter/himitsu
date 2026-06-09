# Himitsu — Domain & Architecture Context

## Domain Terms

These are the concepts the codebase uses. When naming modules, prefer these terms.

- **Store** — a git repo containing encrypted secrets and metadata at `.himitsu/`
- **Remote** — a named store slug (`org/repo`) registered in the user's global config
- **Secret** — one encrypted value stored as `.himitsu/secrets/<path>.yaml`
- **Recipient** — an age public key that can decrypt secrets in a store
- **Output** — a named group of secrets defined in project config (`outputs:` block)
- **Selector** — a query over secrets: `tag:pci`, `prod/*`, `tag:A+tag:B`
- **Reference** — a string that identifies a secret: path, qualified ref (`github:org/repo/path`), or selector
- **Identity** — an age x25519 private key, loaded from disk or macOS Keychain

## Architecture Terms (from 2026-06-08 review)

These name the deepened modules introduced by the architecture review.

- **GitAdapter** — the seam for git operations. Production: `CliGitAdapter` (shells out). Tests: `InMemoryGitAdapter`. Absorbs the commit/push/pull orchestration that was previously inline in Context.
- **SecretStore** — the deepened `remote::store` module. A struct that owns the store root and resolves `recipients_path` once at construction. Narrow interface: `read`, `write`, `list`, `recipients`.
- **SecretResolver** — the deepened secret-resolution pipeline. One module owns the full path from reference string to decrypted `DecodedSecret`. Absorbs the duplicated ref→store→decrypt→decode pipeline that was spread across 5 CLI modules.
