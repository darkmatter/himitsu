# ADR-0001: Keep last-wins duplicate-key collapse in file generators

Date: 2026-06-10
Status: accepted

## Context

When an Output resolves two entries to the same env-var key, the three
consumers historically disagreed:

- `exec` hard-errors (a half-injected environment is forbidden);
- `generate` warns ("duplicate key … — using last value") and takes the
  last value;
- `codegen` (sops mode) silently took the last value.

The 2026-06-09 architecture review (OutputResolver, hm-i9k) made collapse
policy explicitly caller-owned — `OutputResolver::decode` preserves
duplicate keys in entry order — and deferred the question of unifying on a
hard error to hm-x7z.

## Decision

Keep last-wins collapse in the file generators (`generate`, `codegen` sops
mode). Bring `codegen` to warning parity with `generate` (the silent
variant now warns). `exec` keeps its hard error. Do not harden the file
generators to hard errors.

## Rationale

- Within a resolved Output, alias entries resolve **after** selector
  entries (`resolve_output_def`: selectors first, then aliases). Last-wins
  therefore means "the alias pins this binding" — a deliberate, useful
  override pattern: selectors sweep broadly, aliases pin specific keys.
  Hard-erroring would break configs using that pattern.
- Generated files are reviewable artifacts (sops YAML in PRs); the
  dangerous surface is `exec`'s injected environment, which already
  hard-errors on conflicts.
- The remaining gap was `codegen`'s *silent* clobber — fixed by warning
  parity, which is informational and non-breaking.

## Consequences

- Duplicate keys in `generate` and `codegen` warn on stderr and take the
  last value; `exec` remains strict.
- Future architecture reviews should not re-propose hardening the file
  generators without revisiting this ADR.
