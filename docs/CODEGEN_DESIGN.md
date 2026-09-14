# Codegen Redesign: Tag-Based Secret References

> Status: **Draft — Phases 3–5 awaiting approval** (HARD-GATE)
> Scope update (2026-09-04): **Phase 1 (hm-zn6) — the parser only — is authorized and implemented** as the pure `ReferenceExpr` grammar parser (§2.2.1). Existing `himitsu exec`, `himitsu codegen`, and all resolution/clone behavior are **unchanged**, nothing consumes the new parser yet; at that point every other behavioral change in this document (Phases 2–5) was **unapproved draft**.
> Scope update (2026-09-05): **Phase 2 (hm-cw1) — tag file storage only — is authorized and implemented** as `remote::tag_store` (§2.3.1): typed CRUD/list over `.himitsu/tags/<name>.yaml`. The API is store-local — every function takes an explicit, already-resolved store root — and no resolver, CLI, TUI, or migration behavior changed; nothing consumes it yet. Phases 3–5 remain **unapproved draft** until the gate clears.
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

#### 2.2.1 Implemented parser (hm-zn6): `ReferenceExpr`

Phase 1 is implemented as a **pure parser** — the only part of this design that
exists in code. Nothing consumes it yet and no runtime behavior changed.

```rust
pub enum ReferenceExpr {
    Tag(String),          // tag:web-prod
    Path(String),         // path:personal/foo-secret
    BareName(String),     // web-prod — single name, ambiguity retained
    PathPrefix(String),   // personal/ (equivalent to personal/*)
    Qualified(SecretRef), // github:acme/secrets#prod/API_KEY
    Store(SecretRef),     // store:github:acme/secrets
}

impl ReferenceExpr {
    pub fn parse(input: &str) -> Result<Self>; // pure: no IO, no resolution
}
```

| Input | Variant | Notes |
|-------|---------|-------|
| `tag:web-prod` | `Tag` | name validated by `crypto::tags::validate_tag` |
| `path:personal/foo-secret` | `Path` | explicit path; a single name (`path:foo`) also stays `Path` |
| `personal/foo-secret` | `Path` | local slash path, normalized |
| `web-prod` | `BareName` | tag-vs-path ambiguity **retained**; the resolver (Phase 3) decides |
| `personal/` | `PathPrefix` | trailing slash = `personal/*` (the literal `personal/*` and explicit `path:personal/` parse the same); **not** a general glob |
| `github:acme/secrets#prod/API_KEY` | `Qualified` | existing `#` and legacy slash forms both parse |
| `store:github:acme/secrets` | `Store` | requires a provider-qualified `org/repo`, **no** secret path |

Parsing rules:
1. `tag:`, `path:`, and `store:` are **keywords** and take precedence over
   provider parsing.
2. Otherwise, a colon **before the first `/`** denotes a provider prefix; a
   colon in a later, local path segment stays path text (`prod/KEY:env` is a
   path, not a provider ref). The provider must be non-empty ASCII
   alphanumeric/`_`/`-`/`+`.
3. Validation reuses the existing machinery: `crypto::tags::validate_tag` for
   `tag:` names, the private `normalize_path` for `path:` bodies and local slash
   paths, and for the qualified and `store:` forms `SecretRef::parse` followed by
   `config::validate_remote_slug` on the slug — so org/repo traversal
   (`github:../repo/key`) and extra slug segments in the canonical form
   (`github:org/repo/extra#key`) are rejected with `InvalidReference`. Provider
   charset is enforced (non-empty ASCII alphanumeric/`_`/`-`/`+`).
4. Empty references, empty tag names, empty path segments, `.`/`..` traversal
   (bare `.`/`..` are rejected outright, never `BareName`), consecutive
   separators, and malformed qualified forms are rejected.
5. No general glob expression syntax is implemented — `PathPrefix` covers only
   the documented trailing-slash wildcard.

**Explicitly out of scope for this phase** (all later work):
- Bare names are **not** resolved or checked against the filesystem or tag
  store — `BareName` deliberately carries its ambiguity to the future resolver
  (Phase 3, §2.2 precedence rules).
- No store lookup, cloning, or any IO.
- `himitsu exec`, `himitsu codegen`, the TUI, and the existing concrete
  `SecretRef` parser are unchanged; nothing in the runtime consumes
  `ReferenceExpr` yet.

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

#### 2.3.1 Implemented storage API (hm-cw1): `remote::tag_store`

Phase 2 is implemented as **storage only** — the second slice of this design
to exist in code. It validates and persists tag definition files but does no
resolution, and nothing in the runtime consumes it yet.

```rust
// himitsu::remote::tag_store — every function takes the caller's store root
pub struct TagDefinition {
    pub description: String,  // human-readable; defaults to ""
    pub keys: Vec<String>,    // ordered reference strings, never secret values
}

pub fn tags_dir(store: &Path) -> PathBuf;                                        // <store>/.himitsu/tags
pub fn create_tag(store: &Path, name: &str, def: &TagDefinition) -> Result<()>;  // no-clobber
pub fn read_tag(store: &Path, name: &str) -> Result<TagDefinition>;              // TagNotFound if missing
pub fn update_tag(store: &Path, name: &str, def: &TagDefinition) -> Result<()>;  // requires existing
pub fn delete_tag(store: &Path, name: &str) -> Result<()>;                       // requires existing
pub fn list_tags(store: &Path) -> Result<Vec<String>>;                           // sorted; empty when dir missing
```

| Operation | Contract |
|-----------|----------|
| `create_tag` | validates first; creates `.himitsu/tags/` on first use; **refuses to overwrite** an existing tag |
| `read_tag` | `HimitsuError::TagNotFound(name)` when missing; the file is re-validated on read, so a hand-edited malformed tag fails loudly |
| `update_tag` | missing tag is an error (`TagNotFound`); replaces the definition via atomic write |
| `delete_tag` | missing tag is an error (`TagNotFound`); does not prune the now-empty tags directory |
| `list_tags` | deterministic: names sorted alphabetically; a missing `.himitsu/tags/` directory yields an empty list |

Validation happens before any filesystem work:

1. Tag names run through `crypto::tags::validate_tag` (grammar
   `[A-Za-z0-9_.-]+`, 1–64 chars) **plus** filesystem safety: `.` and `..`
   are rejected outright, and the grammar's charset (no `/`, no `:`) keeps
   the generated filename inside `.himitsu/tags/`.
2. Each `keys` entry is parsed with `reference::ReferenceExpr::parse`
   (§2.2.1). Plain store-relative paths, `path:`-qualified paths, and bare
   names all parse — bare-name tag-or-path ambiguity is deliberately
   retained for the resolver (Phase 3), not decided by storage.
   Provider-qualified refs parse syntactically; whether resolution honors
   cross-store refs is open question 2 (§5, still draft). Malformed refs,
   traversal, and empty segments are rejected.
3. `keys` entries are **reference strings, never secret values** — a tag
   file holds zero plaintext.
4. Writes are atomic — a temp file beside the target, then rename
   (`persist`) — the same pattern as `migrate.rs`'s `atomic_write`.

**Explicitly out of scope for this phase** (all later work):

- No resolution: the inline-tags + tag-file merge in §2.3 above is Phase 3
  (hm-ci5); the storage layer never touches secret contents.
- No CLI/TUI: `himitsu tag` still manages only inline tags on secrets; the
  TUI (Phase 4) and codegen paths are untouched.
- No migration from `codegen.envs` (Phase 5).
- **The caller supplies the store.** Every function takes an explicit
  `store: &Path`; no global-store resolution convention was added. "Tags
  default to the global store" (§2.3) stays a caller decision for the
  integration phases — the library imposes no store-selection policy.

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

### Phase 1: Parser (hm-zn6) — implemented (parser-only scope)
Implemented as the pure `ReferenceExpr` parser (§2.2.1): recognizes `tag:foo`,
`path:bar/baz`, bare names, trailing-slash path prefixes, and provider-qualified
and `store:` references, rejecting malformed input via the existing validators.
**No behavior changes** — the parser is not wired into `exec`/`codegen`, does no
store lookup or IO, and the existing `SecretRef` parser used today is untouched.
Giving bare names a meaning is Phase 3 (resolver) work.

### Phase 2: Tag Data Model (hm-cw1) — implemented (storage-only scope)
Implemented as `remote::tag_store` (§2.3.1): typed read/write/validate/list over
`.himitsu/tags/<name>.yaml` in an explicitly supplied store root — no-clobber
create, existing-required update/delete, deterministic sorted listing,
reference-string keys, atomic writes. **No integration**: the resolver, CLI,
TUI, and codegen are unchanged and nothing consumes the API yet; resolution is
Phase 3.

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

Phase 1 (hm-zn6) implemented the grammar as `ReferenceExpr` — a **pure parser**
with no resolution or IO (full API in §2.2.1). The pre-implementation sketch
(`SecretRef` with `Tag`/`Path`/`StorePrefix`/`BarePath`/`BareTag`/`StoreWildcard`
variants) was superseded by the implemented API:

```rust
enum ReferenceExpr {
    Tag(String),          // tag:web-prod
    Path(String),         // path:personal/foo-secret
    BareName(String),     // web-prod — tag-vs-path ambiguity retained for the resolver
    PathPrefix(String),   // personal/ (= personal/*)
    Qualified(SecretRef), // github:acme/secrets#prod/API_KEY
    Store(SecretRef),     // store:github:acme/secrets
}
```

Parser rules (as implemented):
1. `tag:`, `path:`, and `store:` are keywords and take precedence over provider
   parsing.
2. Otherwise a colon before the first `/` denotes a provider prefix (provider
   must be non-empty ASCII alphanumeric/`_`/`-`/`+`); a colon in a later, local
   path segment stays path text (`prod/KEY:env` is a path). The slug of a
   `Qualified`/`Store` ref is validated by `config::validate_remote_slug`, so
   `org/repo` traversal and extra slug segments in the canonical form are
   rejected.
3. Trailing `/` → `PathPrefix` — the documented slash wildcard only; no general
   glob expression syntax is supported. This check applies within the `path:`
   body too, so `path:prod/` → `PathPrefix("prod")`, not `Path`.
4. Contains `/` but no provider prefix → `Path` (the explicit `path:` form
   included).
5. No `/` and no prefix → `BareName`, deliberately unresolved (bare `.`/`..` are
   rejected as traversal, never `BareName`).
6. **Resolution is not part of the parser.** The store-paths-then-tags
   precedence below belongs to the future resolver (Phase 3), which is what
   `himitsu exec`/`codegen` will eventually consult:
   1. matches a path in the current store → path resolution;
   2. matches a tag name → tag resolution;
   3. neither → error with a "did you mean?" hint.

**Runtime today is unchanged**: `exec`/`codegen` still use the existing
`SecretRef` parser; nothing consumes `ReferenceExpr` yet.

## 5. Open Questions (for approval discussion)

1. **Tag file format**: YAML — **settled 2026-09-05 with the hm-cw1 storage approval**, as implemented by `remote::tag_store` (§2.3.1; originally proposed for consistency with other himitsu config).
2. **Cross-store tag references**: Should tag files support `store:` references in their `keys` list, or only same-store paths? (Proposing same-store only; cross-store via full path in `exec`/`codegen` invocation.) Storage (hm-cw1) is policy-neutral here — `keys` entries are checked for reference syntax only, so qualified forms parse; the policy is decided by Phase 3.
3. **Tag namespaces**: Should tags be per-store or global? (Proposing per-store, consistent with secrets.)
4. **Migration command name**: `himitsu codegen migrate` vs. auto-migration on first `himitsu codegen <tag>`? (Proposing explicit `migrate` subcommand.)

## 6. Acceptance Criteria

> Scope: only the first two items below are authorized and implemented
> (parser-only hm-zn6; storage-only hm-cw1). All other items remain gated on
> approval of this design.

- [x] Secret reference grammar parser handles all 5 reference types. (Implemented as the pure `ReferenceExpr` parser, hm-zn6 — see §2.2.1; nothing consumes it yet, runtime unchanged.)
- [x] Tag files stored at `.himitsu/tags/<name>.yaml`, read/written correctly. (Implemented as the storage-only `remote::tag_store` API, hm-cw1 — see §2.3.1; no CLI/TUI/resolver integration yet, runtime unchanged.)
- [ ] Tag resolution merges inline tags + tag-file keys, store-scoped.
- [ ] TUI tag management creates/edits/deletes tags.
- [ ] `himitsu codegen <tag>` resolves tag → generates typed stubs or SOPS file.
- [ ] `himitsu codegen migrate` converts old `codegen.envs` to tag files.
- [ ] Old `codegen.envs` config still works with deprecation warning.
- [ ] All existing integration tests pass (backward compat).
