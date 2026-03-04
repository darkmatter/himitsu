# Himitsu Use Cases (Step-by-Step)

This document gives detailed workflows for two common scenarios:

1. Personal password manager with multi-device sync.
2. Sharing a secret across repositories, then sharing with another team.

It assumes the vNext architecture described in `docs/ARCHITECTURE.md` and the
sharing model in `docs/SHARING.md`.

For backend pattern tradeoffs (app-as-backend, dedicated backend, server
backend), see `docs/BACKENDS.md`.

## Assumptions

- You use `age` keys for encryption.
- Your local state lives in `~/.himitsu/`.
- Backends are git repositories under `~/.himitsu/data/<org>/<repo>/`.
- Shared secret files are stored as `vars/<env>/<KEY>.age`.
- Sharing commands use signed envelopes and inbox processing.

---

## Use Case 1: Personal Password Manager with Device Sync

Goal: Use Himitsu as your personal password vault and keep it synced between
Laptop A and Laptop B safely.

### Architecture for this use case

- One backend repo (example: `yourname/passwords`).
- One recipient group for your own devices (example: `devices`).
- Each device has its own age keypair (recommended).

Why per-device keys are better than copying one private key everywhere:

- You can revoke one lost device without rotating your entire identity.
- You can see which devices are allowed by looking at recipient files.
- Incident response is simpler (`recipient rm <device>` + re-key).

### Step 1: Initialize Himitsu on Laptop A

```bash
himitsu init
```

What this does:

1. Creates `~/.himitsu/` if missing.
2. Generates your local age key under `~/.himitsu/keys/age.txt`.
3. Creates global config at `~/.himitsu/config.yaml`.

### Step 2: Create your password backend

```bash
himitsu backend create github --org yourname --name passwords
```

What this does:

1. Creates private GitHub repo `yourname/passwords`.
2. Clones it into `~/.himitsu/data/yourname/passwords/`.
3. Sets it as active/default backend (if configured to do so).

### Step 3: Register Laptop A as recipient

```bash
himitsu -b yourname/passwords recipient add laptop-a --self --group devices
```

What this does:

1. Reads Laptop A public key from `~/.himitsu/keys/age.txt`.
2. Writes recipient file in backend recipients tree.
3. Makes Laptop A eligible to decrypt future secrets encrypted for `devices`.

### Step 4: Add passwords

Use one environment namespace for personal data, for example `personal`.

```bash
himitsu -b yourname/passwords set personal GITHUB_TOKEN "ghp_xxx"
himitsu -b yourname/passwords set personal BANK_PIN "1234"
himitsu -b yourname/passwords set personal EMAIL_APP_PASSWORD "xxxx-xxxx-xxxx"
```

Each command writes/updates:

- `vars/personal/GITHUB_TOKEN.age`
- `vars/personal/BANK_PIN.age`
- `vars/personal/EMAIL_APP_PASSWORD.age`

### Step 5: Push to remote so other devices can pull

```bash
himitsu -b yourname/passwords backend push
```

This creates a normal git commit and pushes encrypted files only.

### Step 6: Set up Laptop B

On Laptop B:

```bash
himitsu init
himitsu backend add yourname/passwords
```

At this point Laptop B has the backend clone, but it may not yet be a recipient.

### Step 7: Add Laptop B recipient and re-key

On Laptop B, export its public key:

```bash
himitsu -b yourname/passwords recipient add laptop-b --self --group devices
himitsu -b yourname/passwords backend push
```

This commits Laptop B's public key to the recipient tree. However, existing
secrets are still encrypted only for Laptop A. Re-keying requires decryption,
which only an existing recipient can do.

On **Laptop A** (which can already decrypt), pull and re-key:

```bash
himitsu -b yourname/passwords backend pull
himitsu -b yourname/passwords sync
himitsu -b yourname/passwords backend push
```

`sync` re-encrypts all secrets for the updated recipient set (now including
Laptop B).

On Laptop B, pull the re-keyed secrets:

```bash
himitsu -b yourname/passwords backend pull
```

### Step 8: Verify access from both devices

On each device:

```bash
himitsu -b yourname/passwords ls personal
himitsu -b yourname/passwords get personal GITHUB_TOKEN
```

Expected result:

- `ls` shows key names.
- `get` decrypts successfully on both devices.

### Day-to-day workflow

When you update passwords on any device:

1. `set` secret(s)
2. `backend push`
3. On other devices: `backend pull`

### Device lost/stolen workflow

If Laptop B is compromised:

1. Remove its recipient:

   ```bash
   himitsu -b yourname/passwords recipient rm laptop-b --group devices
   ```

2. Re-key all affected secrets:

   ```bash
   himitsu -b yourname/passwords sync
   ```

3. Push the rotation:

   ```bash
   himitsu -b yourname/passwords backend push
   ```

After this, old Laptop B key can no longer decrypt newly re-encrypted files.

---

## Use Case 2: Share Secret Across Repos, Then With Another Team

Goal: You already use Himitsu in your repo. You want to:

1. Make one secret available in another repo you control.
2. Share that same secret with another team's repo/workflow.

We will use:

- Internal repo-to-repo sharing via HSP envelopes.
- External team delivery via GitHub PR inbox.

### Topology choices (important)

You do **not** need a separate `*-secrets` repository to start.

There are three valid patterns:

1. **No dedicated backend yet (ad-hoc share):**
   - You can run `himitsu share send --value ...` directly.
   - Useful for one-off sharing.
   - Tradeoff: weaker local source-of-truth/audit compared to storing in backend first.
2. **App repo as backend:**
   - `acme/payments-app` itself is the backend (`backend: acme/payments-app`).
   - No extra repo required.
3. **Dedicated secrets backend (recommended for scale):**
   - App repo points to `acme/payments-secrets`.
   - Best when multiple repos/environments need shared lifecycle controls.

So yes: `acme/payments-app` can share with `coopmoney/app` **without** an extra
source secrets repo, as long as the destination exposes an inbox flow (or you
target their inbox repo such as `coopmoney/secrets-inbox`).

Example direct share (no dedicated source secrets repo):

```bash
himitsu share send \
  --to github:coopmoney/app \
  --path vars/prod/STRIPE_WEBHOOK_SECRET \
  --value "whsec_xxx"
```

### Scenario setup used in this walkthrough

This walkthrough uses pattern 3 (dedicated backends) because it is easier to
reason about ownership and rotation at scale.

- Source app repo: `acme/payments-app`
- Source secrets backend: `acme/payments-secrets`
- Target app repo: `acme/ledger-app`
- Target secrets backend: `acme/ledger-secrets`
- External team keys repo: `github.com/coopmoney/keys`
- External team inbox repo: `coopmoney/secrets-inbox`

Secret to share:

- Key: `STRIPE_WEBHOOK_SECRET`
- Path namespace: `vars/prod/integrations/`

### Part A: Share from one repo to another repo you own

#### Step 1: Ensure both projects are backend-bound

In each app repo, `.himitsu.yaml` should point to the intended backend.

Example (`acme/payments-app/.himitsu.yaml`):

```yaml
backend: acme/payments-secrets
```

Example (`acme/ledger-app/.himitsu.yaml`):

```yaml
backend: acme/ledger-secrets
```

If you choose pattern 2 (app repo as backend), this step becomes:

```yaml
# payments-app/.himitsu.yaml
backend: acme/payments-app

# ledger-app/.himitsu.yaml
backend: acme/ledger-app
```

#### Step 2: Write secret in source backend

From `payments-app`:

```bash
himitsu set prod STRIPE_WEBHOOK_SECRET "whsec_xxx"
himitsu backend push
```

This stores encrypted source-of-truth in `acme/payments-secrets`.

#### Step 3: Send a share envelope to target repo inbox

```bash
himitsu share send \
  --to github:acme/ledger-secrets \
  --path vars/prod/STRIPE_WEBHOOK_SECRET \
  --value "whsec_xxx"
```

What happens:

1. Himitsu resolves target profile/inbox.
2. Builds payload and encrypts with target recipients.
3. Signs envelope with sender signing key.
4. Opens PR adding `.himitsu/inbox/<id>.json` in target inbox flow.

#### Step 4: Accept in target backend

In target workflow (or locally in `ledger-secrets`):

```bash
himitsu inbox list --transport github_pr
himitsu inbox accept <envelope-id>
himitsu backend push
```

Accept action:

1. Validates signature and replay status.
2. Decrypts payload.
3. Re-encrypts secret into target backend format.
4. Commits encrypted file.

#### Step 5: Consume in target app repo

From `ledger-app`:

```bash
himitsu backend pull
himitsu get prod STRIPE_WEBHOOK_SECRET
```

Now the secret exists in both backends, with independent lifecycle afterward.

### Part B: Share the same secret with another team

#### Step 1: Configure external team source in backend config

In source backend `himitsu.yaml`:

```yaml
remote_sources:
  - id: coopmoney_keys
    kind: github_keys_repo
    repo: coopmoney/keys
    ref: main
```

Sync remote recipient metadata:

```bash
himitsu recipient remote sync
```

#### Step 2: Verify external team ref resolves

Example team ref:

- `remote:github:coopmoney/keys#team=security`

Validate it appears in resolved recipients or schema suggestions before sending.

#### Step 3: Send to external team inbox

```bash
himitsu share send \
  --to github:coopmoney/secrets-inbox \
  --path vars/prod/STRIPE_WEBHOOK_SECRET \
  --value "whsec_xxx"
```

Why PR is used here:

- You usually cannot push directly to another org's default branch.
- PR provides audit trail and policy gates.
- Their Action can verify and route before accepting.

#### Step 4: External team GitHub Action processes envelope

On their side, the receiver workflow should:

1. Trigger on `.himitsu/inbox/*.json` PR changes.
2. Run `himitsu inbox validate`.
3. Run `himitsu inbox accept --from-pr`.
4. Route secret to destination backend/path according to their policy.
5. Commit or open internal PR.

Your responsibility ends at delivering a valid signed encrypted envelope.

#### Step 5: Confirm acceptance

Expected confirmations:

- PR status/check comments in `coopmoney/secrets-inbox`.
- Optional ack comment or artifact.
- No plaintext disclosed in transit or PR contents.

### Operational notes for this flow

- Internal and external shares should use separate envelope IDs (automatic).
- Replaying old envelope IDs should be rejected by recipient replay DB.
- If sender key rotates, update sender profile/signing metadata first.
- If external team rotates inbox key, re-run `recipient remote sync` before new shares.

---

## Quick Failure Guide

### "I can list keys but cannot decrypt"

Likely causes:

- device key not in recipient set
- secrets not re-keyed after recipient change
- wrong backend selected

Fix:

1. Check backend with `-b` explicitly.
2. Confirm recipient entry exists.
3. Run `sync` then `backend push/pull`.

### "Share sent but receiver cannot accept"

Likely causes:

- signature verification failed
- envelope expired
- sender not allowlisted
- replayed envelope ID

Fix:

1. Inspect receiver workflow logs.
2. Re-send with new envelope ID and valid policy sender.
3. Ensure clocks are sane for `expires_at`.
