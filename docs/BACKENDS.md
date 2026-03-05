# Backend Strategy and Server Feedback

This document explains backend options for Himitsu and evaluates two proposals:

1. `himitsu server` as a compatible server that can run on Cloudflare Workers.
2. A hosted public server for users.

It complements:

- `docs/ARCHITECTURE.md` for core system design
- `docs/SHARING.md` for sharing protocol and transport model
- `docs/USE_CASES.md` for workflow walkthroughs
- `docs/SERVER_API.md` for concrete server endpoint contracts

## 1) Backend Types in Himitsu

Himitsu can use multiple backend patterns without changing the encryption model.

### 1.1 Git backend (current default)

- Storage: git repositories under `~/.himitsu/data/<org>/<repo>/`
- Sync: `backend push/pull`
- Strengths: auditability, familiar workflows, offline-friendly
- Weaknesses: less real-time delivery, requires git hosting for collaboration

### 1.2 Transport backends (delivery channels)

These deliver signed encrypted envelopes, not plaintext:

- GitHub PR inbox
- Nostr relays
- HTTP inbox server (proposed)
- Email carrier (identity and optional transport)

Transport choice should not change secret format in destination backends.

### 1.3 Server backend (proposal)

`himitsu server` is best treated as an inbox and envelope-distribution backend
for sharing, not as a trusted decryption backend.

Server stores encrypted envelopes and metadata. Clients still verify signatures
and decrypt locally.

## 2) Proposal A: `himitsu server` on Cloudflare Workers

Short answer: good idea, high leverage, and compatible with the current model.

### 2.1 Why this fits the architecture

- Preserves end-to-end encryption and zero-trust transport assumptions.
- Gives a standard HTTP transport when GitHub/Nostr are not ideal.
- Works well for enterprise firewalls and deterministic delivery APIs.
- Can be self-hosted by teams while keeping protocol compatibility.

### 2.2 Suggested responsibilities

Server should:

- accept encrypted signed envelopes
- index and return envelopes for recipients
- enforce auth, quotas, and abuse controls
- keep replay and retention metadata

Server should not:

- decrypt secret payloads
- hold recipient private keys
- rewrite payload content

### 2.3 Minimal API surface (v1)

```text
POST /v1/envelopes
GET  /v1/envelopes?to=<recipient>&since=<cursor>
GET  /v1/envelopes/<id>
POST /v1/envelopes/<id>/ack
GET  /v1/health
```

Envelope body should be the same JSON schema as `himitsu.envelope`.

Detailed endpoint request/response examples are in `docs/SERVER_API.md`.

### 2.4 Cloudflare Worker deployment shape

Recommended service composition:

- Worker: request auth, validation, API routing
- D1: envelope metadata index, replay/ack state
- R2: envelope blobs (if large payloads are used later)
- KV: caches, rate limit counters, config snapshots
- Queues: async processing (optional)

This supports low-cost global delivery with simple operational footprint.

### 2.5 CLI surface suggestion

```bash
himitsu server init
himitsu server dev
himitsu server deploy --provider cloudflare

himitsu share send --to http:https://inbox.example.com --path ... --value ...
himitsu inbox list --transport http --server https://inbox.example.com
```

### 2.6 Security controls to require

- envelope signature validation at ingest (optional strict mode) or at accept
- sender auth token or mTLS for protected servers
- per-recipient and per-tenant rate limits
- retention policies and hard TTL for stale envelopes
- immutable append-only envelope IDs to support replay defense

## 3) Proposal B: Public hosted server for users

Short answer: viable, but operationally and legally much heavier than self-host
because it becomes an internet-facing multi-tenant message system.

### 3.1 What a public service provides

- managed inbox endpoint for users and teams
- always-on delivery surface without self-hosting
- simpler onboarding for small teams

### 3.2 Major tradeoffs

- abuse/spam handling and moderation load
- account lifecycle and anti-fraud complexity
- multi-tenant isolation guarantees
- legal/compliance obligations even for encrypted blobs
- support burden for delivery/debug issues

### 3.3 Minimum requirements before launch

1. Strong tenant authn/authz model.
2. Signed request auth for senders.
3. Rate limiting and abuse prevention.
4. Robust observability and audit logging.
5. Data retention and delete/export policy.
6. Incident response and key compromise playbooks.

### 3.4 Privacy model to communicate clearly

Even with encrypted payloads, the public service still sees metadata:

- sender and recipient identifiers
- timing and frequency
- message sizes
- transport-level headers and IPs

Users should understand this and choose transport accordingly.

## 4) Recommended Rollout Order

### Phase 1: Protocol-first compatibility

- finalize envelope/profile schemas
- keep transports interchangeable
- maintain E2E and replay guarantees

### Phase 2: Self-hostable `himitsu server`

- ship reference Worker implementation
- provide one-command deploy path
- document hardening checklist

### Phase 3: Optional managed public server

- start as invite-only beta
- enforce strict quotas
- gather operational telemetry before broad rollout

This sequencing gives the ecosystem value early without prematurely committing
to full hosted-service complexity.

## 5) How this interacts with existing backends

- Git backend remains source-of-truth for stored secrets.
- GitHub/Nostr/HTTP become interchangeable delivery paths.
- Users can mix modes:
  - internal team via GitHub PR inbox
  - external partner via HTTP server
  - decentralized delivery via Nostr

No transport should force a storage migration.

## 6) Opinionated Recommendation

If you do one thing now: build `himitsu server` as a deployable compatibility
layer (Cloudflare Worker first), but keep public hosted service as a later stage.

Why:

- fastest path to utility
- lowest trust and lock-in concerns
- aligns with your protocol and E2E design
- avoids early multi-tenant operational burden
