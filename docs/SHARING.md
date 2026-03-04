# Himitsu Sharing Protocol (HSP) - v1 Draft

This document defines how Himitsu shares encrypted secrets across orgs,
repositories, and identities without trusting the delivery channel.

HSP is transport-agnostic and supports:

- GitHub PR inbox (send + receive)
- Nostr relays (send + receive)
- Email identity resolution
- ENS identity resolution

## 1) Goals

1. End-to-end encryption for secret values.
2. Authenticated sender identity (signed envelopes).
3. Replay-safe inbox processing.
4. Git-native workflows and auditability.
5. Transport-independent data model.
6. Minimal runtime dependencies (`age` required, others optional adapters).

## 2) Protocol Objects

### 2.1 `himitsu.profile`

A profile describes a recipient's identity, public keys, and inbox endpoints.
Profiles are resolved from remote sources or published locally.

```json
{
  "v": 1,
  "type": "himitsu.profile",
  "ref": "remote:github:acme/secrets",
  "display_name": "Acme Secrets",
  "age_recipients": [
    "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p"
  ],
  "signing_pubkey": "base64(ed25519-public-key)",
  "inbox": {
    "github_pr": { "repo": "acme/secrets-inbox" },
    "nostr": { "npub": "npub1..." }
  }
}
```

Required fields: `v`, `type`, `ref`, `age_recipients`.
Optional fields: `display_name`, `signing_pubkey`, `inbox`.

### 2.2 `himitsu.payload`

A payload is the plaintext content of a share before encryption. The sender
constructs a payload, serializes it to JSON, then encrypts with `age`.

```json
{
  "v": 1,
  "type": "himitsu.payload",
  "secrets": [
    {
      "path": "vars/prod/API_TOKEN",
      "value": "sk_live_xxx",
      "encoding": "utf8"
    }
  ],
  "sender_ref": "remote:github:acme/payments-secrets",
  "note": "Stripe production token for ledger integration"
}
```

Required fields: `v`, `type`, `secrets` (array with `path` and `value`).
Optional fields: `sender_ref`, `note`, `secrets[].encoding` (default `utf8`,
also supports `base64` for binary values).

### 2.3 `himitsu.envelope`

A signed transport object carrying an encrypted payload. The `to` and `from`
fields use the same recipient ref format as the rest of Himitsu.

```json
{
  "v": 1,
  "type": "himitsu.envelope",
  "id": "018fa4f4-d9c3-7c02-b0e5-9c1f48d0b2e1",
  "to": ["remote:github:acme/ledger-secrets"],
  "from": "remote:github:acme/payments-secrets",
  "ciphertext": {
    "kind": "embedded_age",
    "data": "-----BEGIN AGE ENCRYPTED FILE-----\n...\n-----END AGE ENCRYPTED FILE-----",
    "sha256": "hex..."
  },
  "meta": {
    "label": "vars/prod/API_TOKEN",
    "created_at": "2026-03-04T10:12:00Z",
    "expires_at": "2026-03-11T10:12:00Z",
    "transport_hint": "github_pr"
  },
  "sig": {
    "alg": "ed25519",
    "key_id": "remote:github:acme/payments-secrets#signing-1",
    "value": "base64(signature)"
  }
}
```

Envelope invariants:

- Must be signed.
- Must never contain plaintext secret values.
- Must include a stable unique ID (UUIDv7).
- Must include a SHA-256 checksum of the encrypted payload.

## 3) Cryptography

- **Encryption**: `age` with one or more recipients (via Rust `age` crate).
- **Signing**: Ed25519 over canonical JSON envelope body.
- **Verification** always happens before decrypt/accept.

### 3.1 Canonical JSON for Signing

Envelopes are signed using JSON Canonicalization Scheme (JCS, RFC 8785):

1. Serialize the envelope object **without** the `sig` field.
2. Apply JCS canonicalization (deterministic key ordering, no whitespace,
   Unicode normalization).
3. Sign the resulting byte string with the sender's Ed25519 private key.
4. Attach the signature in the `sig` field.

Verification reverses this: strip `sig`, canonicalize, verify signature against
the sender's known public key.

## 4) Transport Adapters

### 4.1 GitHub PR inbox

Send:

1. Resolve recipient inbox repo from profile.
2. Write envelope to `.himitsu/inbox/<envelope-id>.json`.
3. Open PR to recipient inbox repo.

Receive:

1. Read envelope files from inbox path or PR context.
2. Verify signature and replay status.
3. Accept or reject by policy.

### 4.2 Nostr

Nostr events use a custom event kind for HSP envelopes. The Nostr event
signature is transport-level authentication; the HSP `sig` field inside the
envelope content is the application-level signature that is always verified.

Event structure:

```json
{
  "kind": 30420,
  "content": "<JSON-serialized himitsu.envelope>",
  "tags": [
    ["p", "<recipient-npub-hex>"],
    ["d", "<envelope-id>"],
    ["t", "himitsu-envelope"],
    ["expiration", "<unix-timestamp>"]
  ]
}
```

- **Kind 30420**: parameterized replaceable event (NIP-33). The `d` tag is the
  envelope ID, allowing senders to update/revoke an envelope.
- **`p` tag**: one per recipient, enables relay-side filtering.
- **`t` tag**: type marker for discovery.
- **`expiration` tag**: optional, mirrors `meta.expires_at` (NIP-40).

Send:

1. Serialize envelope as JSON.
2. Create Nostr event with kind 30420 and envelope as content.
3. Add `p`/`d`/`t` tags.
4. Sign with Nostr identity key and broadcast to configured relays.

Receive:

1. Subscribe to relays with filter: `kinds: [30420]`, `#p: [own-npub-hex]`.
2. Parse envelope from event content.
3. Verify HSP signature (not just Nostr event signature) and replay status.
4. Accept or reject by policy.

### 4.3 Email and HTTP (Identity Resolution Only)

Email and HTTP are **identity resolution** mechanisms, not envelope transports
in v1. They allow resolving a recipient's profile (age public keys, inbox
endpoints) so that envelopes can be delivered via GitHub PR or Nostr.

- `email:user@domain.com` resolves via `https://domain.com/.well-known/himitsu.json`.
- `ens:name.eth` resolves via ENS text records.

Future versions may add direct email-based envelope delivery, but v1 treats
email/HTTP as untrusted lookup channels only. All trust is rooted in age
encryption and Ed25519 envelope signatures.

## 5) Inbox Acceptance Pipeline

1. Fetch envelope from transport.
2. Validate schema.
3. Check replay DB for `envelope_id`.
4. Verify signature.
5. Evaluate sender/path policy.
6. Check expiration if present.
7. Decrypt payload.
8. Re-encrypt into remote's format (`vars/<env>/<KEY>.age`).
9. Commit or PR according to local policy.
10. Record envelope as processed.

## 6) Identity Resolution

Resolvers normalize identities into a common profile model:

- `remote:github:<org>/keys#team=<name>`
- `email:user@domain.com` via `/.well-known/himitsu.json`
- `ens:name.eth` via ENS text records:
  - `himitsu_public_key`
  - `himitsu_inbox`
- `nostr:npub...` via npub and optional metadata

## 7) Security Controls

- Replay protection stored in `~/.himitsu/state/inbox.db`
- Sender allowlist policy support
- Destination path allowlist support
- Source lockfile pinning for remote identity snapshots

## 8) GitHub Actions Receiver Contract

Receiver automation should:

1. Trigger on `.himitsu/inbox/*.json`.
2. Run `himitsu inbox validate`.
3. Run `himitsu inbox accept --from-pr`.
4. Commit or open internal PR for applied encrypted updates.
5. Publish acceptance/rejection result.

## 9) Planned CLI Surface

```bash
# Send
himitsu share send --to github:owner/repo --path vars/prod/API_TOKEN --value "..."
himitsu share send --to nostr:npub1... --path vars/prod/API_TOKEN --value "..."
himitsu share send --to email:alice@example.com --path vars/dev/API_TOKEN --value "..."
himitsu share send --to ens:team.eth --path vars/dev/API_TOKEN --value "..."

# Inbox
himitsu inbox list [--transport github_pr|nostr|email|http]
himitsu inbox accept <envelope-id>
himitsu inbox reject <envelope-id>
```

## 10) v1 Priorities

1. Protocol structs + signing + verification.
2. GitHub PR inbox transport end-to-end.
3. Nostr send/receive end-to-end.
4. Resolver adapters (email, ENS, nostr identity).
5. Policy hardening + schema generation.
