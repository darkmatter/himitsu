# Himitsu Serve: Zero-Trust Relay Architecture

## Objective
Enable untrusted environments (like CI/CD runners or staging servers) to use Himitsu secrets without needing access to the master `age` private key. The communication happens via a Cloudflare Worker relay, ensuring the relay server only facilitates transit and never stores secrets.

## Architecture Overview

The system consists of three components:
1. **The Relay (Cloudflare Worker):** A lightweight proxy that coordinates requests using Durable Objects (one DO per session to avoid single-thread bottlenecks).
2. **`himitsu serve` (Trusted Daemon):** Runs in a trusted environment, holding the `age` private key and maintaining an outbound WebSocket connection to the Relay.
3. **The Untrusted Client:** Runs in CI/CD. Makes HTTP requests to the Relay to fetch secrets or perform cryptographic operations.

## Security Model: True E2E Encrypted & Authenticated RPC
To ensure the Cloudflare Worker never sees the plaintext secret, the request path, or can tamper/spoof messages:

1. **Client Request:** 
   - Generates an ephemeral `age` keypair for the response.
   - Creates a JSON payload: `{"action": "decrypt", "path": "prod/db", "reply_to": "<ephemeral_pubkey>", "nonce": "<random>"}`.
   - **Encrypts** this payload using the daemon's known static `age` public key.
   - **Signs** the encrypted blob using the client's static Ed25519 identity key.
   - Sends the signed envelope to the Relay.

2. **Relay Routing:**
   - The Relay only sees the destination Session ID and opaque ciphertext.
   - It forwards the envelope to the active WebSocket for that session.

3. **Daemon Processing:**
   - Verifies the client's Ed25519 signature against an allowed list of client identities.
   - Decrypts the request using the daemon's `age` private key.
   - Rejects duplicate nonces (Replay protection).
   - Enforces ACLs (e.g., is this client allowed to read `prod/db`?).
   - Decrypts the requested secret locally.

4. **Daemon Response:**
   - Creates a response payload: `{"ciphertext": "<secret_data>", "nonce": "<request_nonce>"}`.
   - **Encrypts** the secret using the client's ephemeral `reply_to` public key.
   - **Signs** the response using the daemon's Ed25519 identity key.
   - Sends the signed envelope back to the Relay.

5. **Client Completion:**
   - The Relay forwards the response to the client.
   - Client verifies the daemon's signature.
   - Client decrypts the payload using its ephemeral private key.
   - Ephemeral key is discarded.

**Result:** The Cloudflare Worker handles pure, authenticated ciphertext. No metadata leaks (paths are hidden). Replay attacks and ciphertext substitution are impossible.

## Future Vision: Remote Signing (Targeted for V1)
This architecture perfectly supports "sign, don't transmit":
- Instead of requesting the secret, a client sends: 
  `{"action": "sign", "path": "prod/jwt_secret", "payload": "<data>"}`
- `himitsu serve` decrypts `prod/jwt_secret`, computes the HMAC over `<data>`, and returns the signature.
- **Benefit:** The secret material NEVER leaves the trusted environment, neutralizing entire classes of CI/CD exfiltration attacks.

## Implementation Steps
1. **Worker:** Build the CF Worker with a Durable Object (routed by `session_id`) to hold WebSocket connections. Add timeout handling (e.g., 10s) and disconnect detection.
2. **Daemon:** Add `himitsu serve --relay wss://... --allow-client <did> --allow-path prod/*`. Implement WebSocket reconnects and E2E envelope processing.
3. **Client:** Add `himitsu fetch <secret_path> --relay https://... --daemon-key <pubkey>` command to handle the E2E RPC protocol and ephemeral key generation.
