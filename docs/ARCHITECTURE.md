# Himitsu Architecture (vNext Draft)

This document replaces the old SOPS-centric model with a unified architecture
for:

- centralized local state in `~/.himitsu/`
- `age`-only secret encryption
- transport-agnostic secret sharing (GitHub PR inbox + Nostr + email + ENS)
- full Rust rewrite of the CLI/runtime while preserving core UX

## 1) Design Goals

1. Keep secrets encrypted at rest and in transit.
2. Keep operations deterministic and auditable in git.
3. Support local-first workflows with optional remote sharing.
4. Treat transport as a delivery adapter, not a trust boundary.
5. Keep default runtime dependency minimal (`age` for crypto).

## 2) Core Model

### 2.1 Backends

A backend is a git repo containing encrypted secrets and metadata. Backends are
cloned into:

`~/.himitsu/data/<org>/<repo>/`

Multiple projects can point at the same backend.

### 2.2 Modes

Himitsu resolves context by walking from CWD to `$HOME`:

1. If `.git` is found **and** a `.himitsu.yaml` exists in the repo root, run in
   **project mode**.
2. If `.git` is found but no `.himitsu.yaml` exists, fall through to user mode.
3. Otherwise run in **user mode**.

Project mode reads `<repo>/.himitsu.yaml` for backend binding.
User mode uses `~/.himitsu/config.yaml` default backend or `-b <org/repo>`.

### 2.3 Secret Storage

Secrets are stored as one file per key:

`vars/<env>/<KEY>.age`

Subdirectories within an environment are allowed for organizational grouping:

`vars/<env>/<subdir>/<KEY>.age`

For example, `vars/prod/integrations/STRIPE_KEY.age` is valid.

Values are encrypted, key names are visible in filenames. This keeps change
diffs simple and allows fast listing/search without decrypting everything.

## 3) Filesystem Layout

### 3.1 Global state

```text
~/.himitsu/
├── config.yaml                    # User config (default backend, relays, etc.)
├── keys/
│   ├── age.txt                    # Age private key
│   ├── signing_ed25519            # Envelope signing private key
│   └── signing_ed25519.pub       # Envelope signing public key
├── data/
│   └── <org>/<repo>/              # Backend clones
├── cache/
│   ├── remote-identities/         # Pulled identities (github, well-known, ENS)
│   └── transport/nostr/           # Relay caches/checkpoints
├── locks/
│   └── sources.lock.json          # Pinned remote source fingerprints/commits (see §5.1)
└── state/
    └── inbox.db                   # Replay-protection + envelope processing state
```

### 3.2 Backend layout

```text
~/.himitsu/data/<org>/<repo>/
├── himitsu.yaml                   # Backend config (policies, remotes, targets)
├── data.json                      # Group/env/app metadata
├── vars/
│   ├── common/
│   │   └── API_BASE_URL.age
│   ├── dev/
│   │   └── DB_PASSWORD.age
│   └── prod/
│       └── DB_PASSWORD.age
├── recipients/
│   ├── team/
│   │   ├── alice.pub              # age recipient pubkey
│   │   └── bot.ssh                # ssh pubkey recipient
│   └── admins/
│       └── root.pub
└── .himitsu/
    └── inbox/                     # Incoming envelopes for GitHub PR mode
```

### 3.3 Project binding

```yaml
# <project>/.himitsu.yaml
backend: myorg/secrets

codegen:
  lang: typescript
  path: src/generated/config.ts
```

## 4) Configuration Model

### 4.1 Global config

```yaml
# ~/.himitsu/config.yaml
default_backend: myorg/secrets

nostr:
  relays:
    - wss://relay.damus.io
    - wss://relay.primal.net
  event_kind: 30420

sharing:
  default_transport: github_pr
```

### 4.2 Backend config

```yaml
# backend himitsu.yaml
policies:
  - path_prefix: "vars/common/"
    include: ["group:all"]

  - path_prefix: "vars/prod/"
    include: ["group:admins", "remote:github:coopmoney/keys#team=security"]
    exclude: ["group:contractors"]

remote_sources:
  - id: coopmoney_keys
    kind: github_keys_repo
    repo: coopmoney/keys
    ref: main

  - id: coopmoney_domain
    kind: well_known
    domain: coopmoney.com
    path: /.well-known/himitsu.json

  - id: ens_default
    kind: ens_text_record
    key_public: himitsu_public_key
    key_inbox: himitsu_inbox

targets:
  - name: stripeEncrypted
    type: encrypted_symlink
    source: vars/prod/STRIPE_KEY.age
    destination: ~/code/app/config/stripe_key.age

  - name: appEnv
    type: decrypted_file
    source: vars/dev/
    format: dotenv
    destination: ~/code/app/.env.local
```

## 5) Recipient Resolution

Recipient refs are normalized into one model:

- `group:<name>`
- `remote:github:<org>/keys#team=<team>`
- `email:<user@domain>`
- `ens:<name.eth>`
- `nostr:<npub...>`

Resolution algorithm:

1. Determine path policy by longest matching `path_prefix`.
2. Expand local groups from `recipients/`.
3. Resolve remote refs through source adapters/cache.
4. Apply `exclude`.
5. Produce final deduplicated recipient list.

The resolved list becomes `age -r/-R` args.

### 5.1 Source Lockfile

`~/.himitsu/locks/sources.lock.json` pins remote identity data to specific
snapshots to prevent silent substitution.

```json
{
  "sources": {
    "coopmoney_keys": {
      "kind": "github_keys_repo",
      "repo": "coopmoney/keys",
      "ref": "main",
      "pinned_commit": "abc123...",
      "pinned_at": "2026-03-04T10:00:00Z",
      "key_fingerprints": {
        "team/security/alice.pub": "sha256:xxxx..."
      }
    }
  }
}
```

Update semantics:

- `himitsu recipient remote sync` fetches fresh data and updates the lockfile.
- On subsequent operations, pinned fingerprints are verified before use.
- Manual `--force` flag bypasses pin verification (with warning).
- Lockfile should be committed to the backend repo for team-wide pinning.

## 6) Sharing Architecture

Sharing is modeled as signed envelopes with encrypted payloads. Transport only
moves envelopes; it is never trusted with plaintext.

The full spec lives in `docs/SHARING.md`.

### v1 transports

- GitHub PR inbox (send + receive)
- Nostr (send + receive)
- Email + well-known identity resolution
- ENS identity resolution

## 7) Targets

Two target types are supported:

1. `encrypted_symlink`: safe default, links `.age` files into app paths.
2. `decrypted_file`: explicit render command writes plaintext output.

Safety rules:

- no implicit plaintext rendering in `sync`/`ci`
- rendered plaintext files use strict permissions
- `target clean` removes rendered artifacts

## 8) Rust Rewrite

The shell implementation is replaced by a Rust workspace.

### 8.1 Top-level modules

- `cli`: command parsing and UX
- `config`: mode detection, config loading, schema validation
- `backend`: backend discovery and secret file I/O
- `git`: git CLI wrapper (clone, commit, push, pull, status)
- `crypto`: age encryption/decryption (via `age` crate) + Ed25519 envelope signing
- `policy`: recipient policy engine
- `identity`: github/email/ens/nostr resolvers + cache + lockfile pinning
- `protocol`: envelope/profile/payload models + canonical JSON
- `transport`: transport trait + GitHub PR inbox + Nostr relay adapters
- `inbox`: list/accept/reject/replay-tracking pipeline
- `targets`: apply/render/clean
- `schema`: static+dynamic schema generation
- `codegen`: typed config generation (TypeScript, Go, Python)
- `import`: external source importers (SOPS, 1Password)

### 8.2 Signing Key Lifecycle

The Ed25519 signing keypair is used for envelope authentication.

- **Generation**: `himitsu init` generates `~/.himitsu/keys/signing_ed25519` and
  `signing_ed25519.pub` if they do not exist.
- **Storage**: private key is never committed; public key is published via
  profile or committed to backend `recipients/` for verification.
- **Rotation**: `himitsu key rotate --signing` generates a new keypair and
  optionally publishes the updated public key to configured profiles.
- **Backup**: users are responsible for backing up `~/.himitsu/keys/`. The
  signing key is not recoverable if lost.

### 8.3 CLI continuity

Keep existing semantics where possible:

- `init`, `set`, `get`, `ls`, `encrypt`, `decrypt`, `sync`
- `recipient add|rm|ls`, `group add|rm|ls`, `backend create|add|push|pull|status`
- add/expand `share`, `inbox`, `target`, `schema`, `codegen`
- new: `import` (SOPS, 1Password)

## 9) Security Model

Trust boundaries:

- untrusted: transport medium (GitHub PR, relays, email, HTTP)
- trusted: local machine + private keys

Controls:

- envelope signature verification before accept
- replay prevention via envelope-id persistence
- source lockfile pinning for remote identity data
- sender allowlist policy on receiver side
- expiration support (`expires_at`) in envelope metadata

## 10) JSON Schema and Autocomplete

Schema strategy:

- static schema for structure:
  - `schemas/himitsu.schema.json`
- generated dynamic schema for completion:
  - `schemas/recipients.schema.json`

`himitsu schema refresh` regenerates dynamic enums from:

- local groups/recipients
- remote team indexes
- cached resolver identities

## 11) GitHub Actions Role

Receiver automation (in recipient repo) can:

1. validate envelope signature and policy
2. decrypt payload with inbox key
3. re-encrypt into destination backend format
4. commit or open internal PR
5. annotate result for audit trail

This enables "share with team" using one published inbox key even for large orgs.

## 12) Nostr Role

Nostr provides decentralized send+receive transport:

- sender publishes signed envelope event with recipient tags
- receiver polls/subscribes, verifies, accepts, and writes encrypted secrets

Relay metadata is untrusted; confidentiality is enforced by payload encryption.

## 13) Import and Migration

### 13.1 Import sources

`himitsu import` brings secrets from external systems into a backend:

```bash
# Import from SOPS-encrypted YAML/JSON
himitsu import --sops path/to/secrets.sops.yaml --env prod

# Import from 1Password via CLI
himitsu import --op "op://vault/item/field" --env prod --key API_TOKEN

# Import from 1Password (multiple fields from one item)
himitsu import --op "op://vault/item" --env prod
```

Import is always additive: existing secrets are not overwritten unless
`--overwrite` is passed.

### 13.2 Migration from shell implementation

1. Introduce Rust binary behind feature flag, keep shell commands for fallback.
2. Use `himitsu import --sops` to convert `vars/*.sops.json` into
   `vars/<env>/<KEY>.age` format.
3. Migrate recipient files to `.pub`/`.ssh` convention.
4. Enable sharing transports incrementally (GitHub first, then Nostr, then others).
5. Remove shell path once parity tests pass.

## 14) Non-goals (v1)

- no KMS integration
- no automatic secret rotation engine
- no requirement for hosted control plane
- no plaintext storage in repositories
- no GPG recipient support (age-only; GPG keys from the shell implementation are
  not carried forward)

## 15) Detailed Implementation Plan

See `docs/IMPLEMENTATION_PLAN.md` for phase-by-phase execution details,
deliverables, risks, and acceptance criteria.

## 16) Detailed Use Cases

For hands-on end-to-end walkthroughs, see `docs/USE_CASES.md`.

## 17) Backend Strategy and Server Model

For backend-mode comparisons and server backend feedback (including Cloudflare
Workers and public hosted service considerations), see `docs/BACKENDS.md`.

For concrete HTTP API contracts for `himitsu server`, see `docs/SERVER_API.md`.
