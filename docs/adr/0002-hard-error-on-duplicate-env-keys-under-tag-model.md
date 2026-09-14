# ADR-0002: Hard-error on duplicate Env Keys under the Tag model

Date: 2026-08-31
Status: accepted (supersedes ADR-0001)

## Context

ADR-0001 kept last-wins duplicate-key collapse in the file generators
(`generate`, `codegen`) on the strength of one pattern: within a resolved
Output, alias entries resolved after selector entries, so last-wins meant
"the alias deliberately pins this binding." `exec` already hard-errored.

The tag-first redesign (hm-qlh, session 2026-08-31) removes the alias
layer: an Env Key is a property of the Secret alone (`set --env-key` or
name-derived), and a Tag is a pure group — inline-tagged secrets united
with what the tag's own references resolve to, with no ordering among
inline members. "Last" stopped being well-defined, and last-wins stopped
meaning a deliberate override.

## Decision

A resolved Tag whose members produce the same Env Key is a hard error in
every consumer — `exec`, `generate`, `codegen` (sops materialization and
typed stubs alike). No consumer collapses duplicates. The error names
both colliding secret paths and the derived key; the fix is
`himitsu set --env-key` on one of them.

## Rationale

- The deliberate-override pattern ADR-0001 protected was alias-based;
  aliases no longer exist.
- Under pure name derivation, a duplicate Env Key is two different
  secrets claiming one name. Arbitrarily picking one for a generated
  file or an injected environment is worse than erroring.
- Keeping `exec` strict and the file generators lax was a
  consumer-dependent difference with no remaining justification.

## Consequences

- Configs that previously warned and took the last value now fail until
  one colliding secret gets an explicit Env Key.
- ADR-0001 is superseded; its instruction that future reviews not
  re-propose hardening "without revisiting this ADR" is discharged by
  this revisit.
