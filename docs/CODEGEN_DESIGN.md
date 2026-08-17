# Codegen Redesign: Tag-Based Secret References

> Status: **Draft — awaiting approval** (HARD-GATE)
> Bead: hm-qlh / hm-fcb
> Created: 2026-08-17

## 1. Problem Statement

The current codegen system has two modes — SOPS env materialization and typed language stub generation — both built around an "environment" abstraction (`codegen.envs` in `.himitsu.yaml`) that duplicates the tag system already present for `himitsu exec`. Environments are a separate entity from tags, creating two parallel mental models for the same underlying concept: grouping secrets by deployment target.

The goal is to **eliminate environments/codegen labels as a separate entity** and make everything tags. Codegen becomes a preset of `himitsu exec` invocations — a tag like `web-prod` resolves to a set of secrets, and the codegen feature simply generates typed stubs (or SOPS materializations) from that tag's resolved secret set.

## 2. Design Overview

### 2.1 Tag-First Model

**Tags are the only grouping entity.** The current `codegen.envs` config section and `SecretInventory.environments` field are removed. A codegen "environment" is just a tag:

| Before | After |
|--------|-------|
| `codegen: { envs: { prod: { keys: [API_KEY, DB_URL] } } }` | `himitsu tag add web-prod --keys "api-key,db-url"` (or tag secrets directly) |
| `himitsu codegen prod` | `himitsu codegen web-prod` (resolves tag → secret set → SOPS materialize) |
| `himitsu codegen --lang ts --env prod` | `himitsu codegen --lang ts web-prod` |

### 2.2 Secret Reference Grammar

All secret references (used by `exec`, `codegen`, and the TUI) follow one grammar:

```
reference := explicit_ref | bare_ref
explicit_ref := prefix ':' identifier
prefix := 'tag' | 'path' | 'store'
bare_ref := path_ref | tag_ref | store_prefix_ref
path_ref := path_segment ('/' path_segment)*           # e.g. personal/foo-secret
store_prefix_ref := path_segment '/'                     # e.g. personal/ (= personal/*)
tag_ref := identifier                                     # e.g. some-tag (ambiguous: could be tag or path)
```

**Resolution precedence** (when a bare reference is ambiguous):
1. If the reference matches a path in the current store → **path** resolution.
2. If the reference matches a tag name → **tag** resolution.
3. If neither → error with a "did you mean?" hint.

**Disambiguation** via explicit prefix:
- `tag:web-prod` → tag resolution only (error if no such tag).
- `path:personal/foo-secret` → path resolution only (error if no such path).
- `store:github:darkmatter/secrets` → store prefix resolution (all secrets in that store).

### 2.3 Tag Storage

Tags live in the store, defaulting to the global (user) store like secrets do. A tag is stored as a small metadata file:

```
.himitsu/tags/<tag-name>.yaml
```

Each tag file contains:
```yaml
# .himitsu/tags/web-prod.yaml
description: "Production web service secrets"
keys:
  - api-key
  - db-url
  - path:personal/stripe-key    # explicit path ref within the same store
```

Or alternatively, tags can be applied directly to secrets via the existing tag metadata in the secret envelope (`himitsu: tags: [web-prod]`). Both mechanisms work — the tag file provides a named preset, while inline tags on secrets provide automatic grouping.

**Tag resolution** merges both sources:
1. Read all secrets with `web-prod` in their `himitsu.tags` metadata.
2. Read `.himitsu/tags/web-prod.yaml` for any additional `keys` entries.
3. Union the two sets.

### 2.4 Store-Scoped Resolution

In codegen, tags resolve to secrets **in the same store** only. To reference a secret in a different store, use the full path reference:

```
himitsu exec github:darkmatter/secrets/api-key    # cross-store reference
himitsu exec web-prod                              # resolves only in current store
```

This prevents accidental cross-store leakage in codegen presets.

### 2.5 Codegen TUI

The codegen TUI is **removed** in its current form. Instead, the existing TUI's tag management interface (hm-spm) handles codegen tags — they're just regular tags. The workflow becomes:

1. `himitsu tag add web-prod` (or via TUI tag management)
2. `himitsu codegen web-prod` → resolves tag → generates typed stubs or SOPS materialization
3. `himitsu codegen --lang ts web-prod` → typed stubs for TypeScript

No separate codegen TUI view is needed.

## 3. Migration Path

### Phase 1: Parser (hm-zn6)
Implement the secret reference grammar parser. No behavior changes — just a parser that can distinguish `tag:foo`, `path:bar/baz`, `store:github:...`, and bare references.

### Phase 2: Tag Data Model (hm-cw1)
Implement the tag file storage (`.himitsu/tags/<name>.yaml`). Read/write/validate tag files. No integration yet.

### Phase 3: Tag Resolution (hm-ci5)
Implement tag resolution: merge inline tags (from secret metadata) + tag-file keys. Store-scoped by default, full-path for cross-store.

### Phase 4: TUI Tag Management (hm-spm)
Add tag management to the TUI (create/edit/delete tags). This replaces the codegen TUI.

### Phase 5: Migration (hm-r80)
Migrate existing `codegen.envs` config to tag files. Provide a `himitsu codegen migrate` command that reads `.himitsu.yaml` codegen envs and creates equivalent tag files. Remove the old codegen env config support after migration.

### Backward Compatibility
- `himitsu codegen <env>` continues to work — it now resolves `<env>` as a tag name instead of an env label.
- `himitsu codegen --lang ts --env prod` continues to work — `--env` is syntactic sugar for `tag:<env>`.
- The `codegen` section in `.himitsu.yaml` is read for backward compat but emits a deprecation warning.

## 4. Exec Grammar Parsing Rules

```rust
enum SecretRef {
    Tag(String),          // tag:web-prod
    Path(String),         // path:personal/foo-secret
    StorePrefix(String),  // store:github:darkmatter/secrets
    BarePath(String),     // personal/foo-secret (resolved by precedence)
    BareTag(String),      // web-prod (resolved by precedence)
    StoreWildcard(String), // personal/ (= personal/*)
}
```

Parser rules:
1. If the reference contains a `:` before any `/` → parse as `prefix:rest`.
2. If the reference ends with `/` → `StoreWildcard`.
3. If the reference contains `/` but no prefix → `BarePath`.
4. If the reference contains no `/` and no prefix → `BareTag`.
5. Resolve `BarePath`/`BareTag` by checking store paths first, then tags.

## 5. Open Questions (for approval discussion)

1. **Tag file format**: YAML vs. a simpler key-per-line format? (Proposing YAML for consistency with other himitsu config.)
2. **Cross-store tag references**: Should tag files support `store:` references in their `keys` list, or only same-store paths? (Proposing same-store only; cross-store via full path in `exec`/`codegen` invocation.)
3. **Tag namespaces**: Should tags be per-store or global? (Proposing per-store, consistent with secrets.)
4. **Migration command name**: `himitsu codegen migrate` vs. auto-migration on first `himitsu codegen <tag>`? (Proposing explicit `migrate` subcommand.)

## 6. Acceptance Criteria

- [ ] Secret reference grammar parser handles all 5 reference types.
- [ ] Tag files stored at `.himitsu/tags/<name>.yaml`, read/written correctly.
- [ ] Tag resolution merges inline tags + tag-file keys, store-scoped.
- [ ] TUI tag management creates/edits/deletes tags.
- [ ] `himitsu codegen <tag>` resolves tag → generates typed stubs or SOPS file.
- [ ] `himitsu codegen migrate` converts old `codegen.envs` to tag files.
- [ ] Old `codegen.envs` config still works with deprecation warning.
- [ ] All existing integration tests pass (backward compat).
