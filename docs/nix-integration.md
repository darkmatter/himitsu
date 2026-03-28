# Himitsu Nix Integration

> Seamless secret injection for `nix develop`, OCI containers, and CI pipelines.

Himitsu provides a Nix library (`lib`) that wraps your development shell with
automatic secret decryption, environment variable injection, and an optional
credential server that speaks AWS ECS, SOPS, and raw HTTP.

---

## Table of Contents

- [Quick Start](#quick-start)
- [Architecture](#architecture)
- [API Reference](#api-reference)
  - [`packSecrets`](#packsecrets)
  - [`mkDevShell`](#mkdevshell)
  - [`wrapAge`](#wrapage)
  - [`wrapSops`](#wrapsops)
  - [`mkCredentialServer`](#mkcredentialserver)
  - [`mkCredentialServerImage`](#mkcredentialserverimage)
  - [Legacy: `mkEncryptedSecrets`](#legacy-mkencryptedsecrets)
  - [Legacy: `mkDecryptWrapper`](#legacy-mkdecryptwrapper)
- [Delivery Modes](#delivery-modes)
- [Credential Server](#credential-server)
- [AWS Integration](#aws-integration)
- [SOPS Integration](#sops-integration)
- [Container Sidecar Pattern](#container-sidecar-pattern)
- [Environment Variables](#environment-variables)
- [Security Model](#security-model)
- [Examples](#examples)
- [Troubleshooting](#troubleshooting)

---

## Quick Start

### 1. Add himitsu to your flake inputs

```nix
# flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    himitsu.url = "github:darkmatter/himitsu";
  };

  outputs = { self, nixpkgs, himitsu, ... }:
    let
      system = "aarch64-darwin";  # or x86_64-linux, etc.
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      devShells.${system}.default = himitsu.lib.${system}.mkDevShell {
        devShell = pkgs.mkShell {
          packages = [ pkgs.nodejs pkgs.python3 ];
        };
        store = ./.himitsu;
        env   = "dev";
      };
    };
}
```

### 2. Enter the shell

```bash
nix develop
# 🔓 himitsu: decrypted 12 secret(s) into dev shell

echo $DATABASE_URL       # ← individual env var
echo $HIMITSU_SECRETS_DIR  # ← path to plaintext files
```

That's it. Every `.age` file in `.himitsu/vars/common/` and `.himitsu/vars/dev/`
is decrypted using your local age identity (`~/.himitsu/keys/age.txt`), and the
plaintext values are:

- Exported as environment variables (`DATABASE_URL`, `STRIPE_KEY`, etc.)
- Available as a base64-encoded JSON blob (`HIMITSU_SECRETS_JSON`)
- Written to a secure tmpdir (`HIMITSU_SECRETS_DIR`)
- Cleaned up automatically when the shell exits

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  nix develop                                                    │
│                                                                 │
│  ┌──────────┐    ┌──────────────┐    ┌────────────────────────┐ │
│  │ packSe-  │───►│  shellHook   │───►│  Your devShell         │ │
│  │ crets    │    │  (decrypt)   │    │  (nodejs, python, …)   │ │
│  │ .age     │    └──────┬───────┘    └────────────────────────┘ │
│  └──────────┘           │                                       │
│                         ├─► env vars      (DATABASE_URL=…)      │
│                         ├─► JSON blob     (HIMITSU_SECRETS_JSON) │
│                         ├─► plaintext dir (HIMITSU_SECRETS_DIR) │
│                         ├─► SOPS_AGE_KEY_FILE (auto-set)        │
│                         └─► credential server (optional)        │
│                              ├─► :9292/v1/secrets               │
│                              ├─► :9292/credentials (AWS ECS)    │
│                              └─► :9292/v1/sops-age-key          │
└─────────────────────────────────────────────────────────────────┘
```

---

## API Reference

### `packSecrets`

Collect encrypted `.age` files into a clean Nix derivation.

#### Simple form — path to an env directory

```nix
secrets = himitsu.lib.${system}.packSecrets ./.himitsu/vars/dev;
```

#### Attrset form — full control

```nix
secrets = himitsu.lib.${system}.packSecrets {
  src       = ./.himitsu;          # root of the himitsu store
  env       = "dev";               # target environment
  mergeEnvs = [ "common" ];        # lower-priority envs merged in
  name      = "my-project-secrets-dev";
};
```

**Merge semantics:** Files from `mergeEnvs` are copied first. Then the target
`env` is copied on top, overwriting any conflicts. This means `dev` values take
precedence over `common` values when the same key name exists in both.

#### Output structure

```
$out/
├── secrets/
│   ├── DATABASE_URL.age
│   ├── STRIPE_KEY.age
│   └── …
└── manifest.txt          # newline-delimited key names
```

---

### `mkDevShell`

Wrap any existing devShell (or `null` for a bare shell) with automatic himitsu
secret injection.

```nix
himitsu.lib.${system}.mkDevShell {
  # ── Required (one of) ────────────────────────────────────────
  store   = ./.himitsu;           # auto-packs secrets from this store
  env     = "dev";                # which environment to decrypt
  # — OR —
  secrets = myPackedSecrets;      # pre-built packSecrets derivation

  # ── Shell to wrap ────────────────────────────────────────────
  devShell = pkgs.mkShell {       # your existing shell (optional)
    packages = [ pkgs.nodejs ];
  };

  # ── Delivery configuration ──────────────────────────────────
  exportEnv = true;               # export each secret as an env var
  jsonEnv   = true;               # export HIMITSU_SECRETS_JSON (base64)

  plaintext = {
    enable = false;               # write plaintext files to disk
    dir    = null;                 # override destination (default: secure tmpdir)
  };

  credentialServer = {
    enable     = false;           # start HTTP credential server
    port       = 9292;            # listen port
    awsProfile = null;            # set to enable AWS ECS credential endpoint
    sops       = false;           # serve age key for SOPS consumption
  };

  # ── Advanced ─────────────────────────────────────────────────
  mergeEnvs    = [ "common" ];    # lower-priority envs to merge
  identityFile = null;            # override age identity path
  extraPackages = [];             # additional packages in the shell
  verbose       = false;          # print each decrypted key name
}
```

#### Parameters

| Parameter | Type | Default | Description |
|---|---|---|---|
| `devShell` | derivation / null | `null` | Existing shell to wrap. All its `buildInputs`, `nativeBuildInputs`, env vars, and `shellHook` are preserved. |
| `store` | path | — | Path to a `.himitsu` store directory. Mutually exclusive with `secrets`. |
| `secrets` | derivation / list | — | Pre-built `packSecrets` derivation(s). Pass a list to merge multiple packs. |
| `env` | string | `"dev"` | Target environment to decrypt. |
| `mergeEnvs` | list of string | `["common"]` | Lower-priority environments whose keys are included unless overridden by `env`. |
| `exportEnv` | bool | `true` | Export each decrypted secret as a shell environment variable. |
| `jsonEnv` | bool | `true` | Export `HIMITSU_SECRETS_JSON` (base64) and `HIMITSU_SECRETS_JSON_RAW`. |
| `plaintext.enable` | bool | `false` | Write plaintext files to a directory. |
| `plaintext.dir` | path / null | `null` | Override plaintext destination. Default: `$_himitsu_tmpdir/plaintext`. |
| `credentialServer.enable` | bool | `false` | Start the HTTP credential server in the background. |
| `credentialServer.port` | int | `9292` | Port for the credential server. |
| `credentialServer.awsProfile` | string / null | `null` | If non-null, sets `AWS_CONTAINER_CREDENTIALS_FULL_URI` to point at the server. |
| `credentialServer.sops` | bool | `false` | Serve the age identity key via `/v1/sops-age-key`. |
| `identityFile` | path / null | `null` | Override the age identity file. Default: `~/.himitsu/keys/age.txt` or `$HIMITSU_IDENTITY`. |
| `extraPackages` | list | `[]` | Additional packages to add to the shell. |
| `verbose` | bool | `false` | Print each key name as it's decrypted. |

---

### `wrapAge`

A wrapped `age` binary that automatically injects the himitsu identity file
when decrypting.

```nix
wrapAge = himitsu.lib.${system}.wrapAge {
  identityFile = null;  # default: $HIMITSU_IDENTITY or ~/.himitsu/keys/age.txt
};
```

**Behavior:**

- `age --decrypt secret.age` → identity flag is injected automatically
- `age --encrypt -r age1… -o out in` → passed through untouched
- If `-i` is already present, no injection occurs

This is included automatically in every `mkDevShell`. You can also use it
standalone:

```nix
devShells.default = pkgs.mkShell {
  packages = [
    (himitsu.lib.${system}.wrapAge {})
  ];
};
```

---

### `wrapSops`

A wrapped `sops` binary that automatically sets `SOPS_AGE_KEY_FILE` to the
himitsu identity.

```nix
wrapSops = himitsu.lib.${system}.wrapSops {
  identityFile = null;  # default: $HIMITSU_IDENTITY or ~/.himitsu/keys/age.txt
};
```

> **Note:** Requires `pkgs.sops` to be available in your nixpkgs. If it isn't,
> this will throw an error at evaluation time.

---

### `mkCredentialServer`

Build a standalone credential server script. Useful for systemd services,
manual invocation, or CI pipelines.

```nix
credServer = himitsu.lib.${system}.mkCredentialServer {
  store     = ./.himitsu;
  env       = "prod";
  mergeEnvs = [ "common" ];
  port      = 9292;
  host      = "127.0.0.1";
};
```

Run it:

```bash
# Uses ~/.himitsu/keys/age.txt by default
himitsu-credential-server

# Override identity
himitsu-credential-server --identity /path/to/age.txt

# Override port
himitsu-credential-server --port 8080
```

---

### `mkCredentialServerImage`

Build a minimal OCI (Docker) container image containing the credential server
and encrypted secrets. The image decrypts secrets at container start time.

```nix
packages.secrets-server = himitsu.lib.${system}.mkCredentialServerImage {
  store     = ./.himitsu;
  env       = "prod";
  mergeEnvs = [ "common" ];
  port      = 9292;
  name      = "himitsu-credential-server";
  tag       = "latest";
};
```

```bash
# Build the image
nix build .#secrets-server

# Load into Docker
docker load < result

# Run with mounted identity
docker run -p 9292:9292 \
  -v ~/.himitsu/keys/age.txt:/identity/age.txt:ro \
  himitsu-credential-server

# Or pass identity via env var
docker run -p 9292:9292 \
  -e AGE_IDENTITY_B64="$(base64 < ~/.himitsu/keys/age.txt)" \
  himitsu-credential-server
```

---

### Legacy: `mkEncryptedSecrets`

> **Deprecated** — prefer [`packSecrets`](#packsecrets).

```nix
encrypted = himitsu.lib.${system}.mkEncryptedSecrets {
  src  = ./.himitsu;
  env  = "prod";
  name = "himitsu-secrets";
};
```

### Legacy: `mkDecryptWrapper`

> **Deprecated** — prefer [`mkDevShell`](#mkdevshell) or [`mkCredentialServer`](#mkcredentialserver).

```nix
decrypt = himitsu.lib.${system}.mkDecryptWrapper {
  secretsPkg = myEncryptedSecrets;
  destDir    = "/run/secrets";
};
```

---

## Delivery Modes

`mkDevShell` supports multiple delivery modes simultaneously:

### 1. Environment Variables (`exportEnv = true`)

Each secret file (e.g., `DATABASE_URL.age`) becomes a shell environment
variable with the decrypted value:

```bash
echo $DATABASE_URL       # postgresql://…
echo $STRIPE_KEY         # sk_live_…
```

### 2. JSON Blob (`jsonEnv = true`)

All secrets are available as a single JSON object, both raw and base64-encoded:

```bash
echo $HIMITSU_SECRETS_JSON_RAW | jq .
# {
#   "DATABASE_URL": "postgresql://…",
#   "STRIPE_KEY": "sk_live_…"
# }

echo $HIMITSU_SECRETS_JSON | base64 -d | jq .
# Same thing, useful for passing through systems that mangle env vars
```

### 3. Plaintext Files (`plaintext.enable = true`)

Decrypted files are written to a directory:

```bash
ls $HIMITSU_SECRETS_DIR/
# DATABASE_URL  STRIPE_KEY  …

cat $HIMITSU_SECRETS_DIR/DATABASE_URL
# postgresql://…
```

By default, files are written to a secure tmpdir under `/dev/shm` (Linux) or
`$TMPDIR` (macOS). Override with `plaintext.dir`.

### 4. Credential Server (`credentialServer.enable = true`)

An HTTP server runs in the background, serving secrets over localhost.

```bash
curl -H "Authorization: Bearer $HIMITSU_SERVER_TOKEN" \
  http://127.0.0.1:9292/v1/secrets | jq .
```

---

## Credential Server

The credential server is a lightweight Python HTTP service (stdlib only, no
external dependencies) that serves decrypted secrets over `127.0.0.1`.

### Routes

| Method | Path | Response | Description |
|---|---|---|---|
| GET | `/v1/health` | `{"status":"ok","secrets":N}` | Health check |
| GET | `/v1/secrets` | `{"KEY":"value",…}` | All secrets as JSON |
| GET | `/v1/secrets/<KEY>` | raw value | Single secret value |
| GET | `/v1/secrets-b64` | base64 string | All secrets as base64-encoded JSON |
| GET | `/v1/age-identity` | raw key | Age secret key (for tooling) |
| GET | `/v1/sops-age-key` | raw key | Age secret key (SOPS format) |
| GET | `/credentials` | AWS ECS JSON | AWS ECS credential provider format |

### Authentication

Every request requires a Bearer token:

```
Authorization: Bearer <token>
```

The token is auto-generated at startup and exported as `$HIMITSU_SERVER_TOKEN`.
You can also provide your own via `--token` or `$HIMITSU_SERVER_TOKEN`.

### Security

- Binds to `127.0.0.1` only (devshell) or `0.0.0.0` (container mode)
- Requires Bearer token authentication on every request
- Plaintext secrets exist only in a `chmod 700` tmpdir
- tmpdir is on `/dev/shm` (RAM-backed) when available
- Everything is cleaned up on shell exit / container stop

---

## AWS Integration

The credential server speaks the [AWS ECS Task IAM Role](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/task-iam-roles.html) protocol natively. Set `credentialServer.awsProfile` to any non-null value, and the shell automatically configures:

```bash
AWS_CONTAINER_CREDENTIALS_FULL_URI=http://127.0.0.1:9292/credentials
AWS_CONTAINER_AUTHORIZATION_TOKEN=Bearer <token>
```

The `/credentials` endpoint returns:

```json
{
  "AccessKeyId": "<from AWS_ACCESS_KEY_ID secret>",
  "SecretAccessKey": "<from AWS_SECRET_ACCESS_KEY secret>",
  "Token": "<from AWS_SESSION_TOKEN secret>",
  "Expiration": "2099-12-31T23:59:59Z"
}
```

**Requirements:** Your himitsu store must contain secrets named
`AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` (and optionally
`AWS_SESSION_TOKEN`).

```nix
himitsu.lib.${system}.mkDevShell {
  store = ./.himitsu;
  env   = "dev";
  credentialServer = {
    enable     = true;
    awsProfile = "default";
  };
};
```

```bash
nix develop
aws s3 ls  # ← just works, no aws configure needed
```

---

## SOPS Integration

Himitsu automatically sets `SOPS_AGE_KEY_FILE` to your age identity in every
`mkDevShell`. This means `sops` can decrypt files without any manual
configuration:

```bash
nix develop
sops --decrypt secrets.enc.yaml  # ← uses your himitsu age key
```

If you add `wrapSops` to your packages, you get a `sops` binary that always
has the correct key file set, even outside the devshell:

```nix
packages = [ (himitsu.lib.${system}.wrapSops {}) ];
```

The credential server also exposes `/v1/sops-age-key` for programmatic access
to the raw age key, useful for CI systems or custom integrations.

---

## Container Sidecar Pattern

Use `mkCredentialServerImage` to build a secrets sidecar for Docker Compose,
Kubernetes, or any container orchestrator:

```yaml
# docker-compose.yml
services:
  secrets:
    image: himitsu-credential-server:latest
    volumes:
      - ~/.himitsu/keys/age.txt:/identity/age.txt:ro
    ports:
      - "9292:9292"

  api:
    image: my-api:latest
    depends_on:
      - secrets
    environment:
      AWS_CONTAINER_CREDENTIALS_FULL_URI: "http://secrets:9292/credentials"
      AWS_CONTAINER_AUTHORIZATION_TOKEN: "Bearer ${HIMITSU_TOKEN}"
      HIMITSU_SERVER_URL: "http://secrets:9292"

  worker:
    image: my-worker:latest
    depends_on:
      - secrets
    environment:
      # Workers can fetch individual secrets via HTTP
      SECRET_PROVIDER_URL: "http://secrets:9292/v1/secrets"
```

### Kubernetes

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: my-app
spec:
  initContainers:
    - name: secrets
      image: himitsu-credential-server:latest
      env:
        - name: AGE_IDENTITY_B64
          valueFrom:
            secretKeyRef:
              name: himitsu-identity
              key: age-key-b64
  containers:
    - name: app
      image: my-app:latest
      env:
        - name: AWS_CONTAINER_CREDENTIALS_FULL_URI
          value: "http://localhost:9292/credentials"
```

---

## Environment Variables

### Set by `mkDevShell`

| Variable | Description |
|---|---|
| `HIMITSU_ENV` | Current environment name (e.g., `"dev"`) |
| `HIMITSU_STORE` | Path to the himitsu store |
| `HIMITSU_IDENTITY` | Path to the age identity file |
| `HIMITSU_SECRETS_DIR` | Path to decrypted plaintext files |
| `HIMITSU_SECRETS_COUNT` | Number of successfully decrypted secrets |
| `HIMITSU_SECRETS_JSON` | Base64-encoded JSON of all secrets |
| `HIMITSU_SECRETS_JSON_RAW` | Raw JSON of all secrets |
| `SOPS_AGE_KEY_FILE` | Points to the age identity (for SOPS compat) |
| `<SECRET_NAME>` | Each secret is exported individually |

### Set when credential server is enabled

| Variable | Description |
|---|---|
| `HIMITSU_SERVER_URL` | `http://127.0.0.1:9292` |
| `HIMITSU_SERVER_TOKEN` | Bearer token for authentication |
| `HIMITSU_SERVER_PID` | PID of the background server process |

### Set when AWS profile is enabled

| Variable | Description |
|---|---|
| `AWS_CONTAINER_CREDENTIALS_FULL_URI` | `http://127.0.0.1:9292/credentials` |
| `AWS_CONTAINER_AUTHORIZATION_TOKEN` | `Bearer <token>` |

### Inputs you can override

| Variable | Description |
|---|---|
| `HIMITSU_IDENTITY` | Override the age identity file path (default: `~/.himitsu/keys/age.txt`) |

---

## Security Model

### What is safe

- **Encrypted secrets in the Nix store:** `.age` files are safe to include in
  derivations, push to Cachix, or commit to Git. They are only decryptable by
  holders of the age private key.

- **The OCI image:** Contains only encrypted `.age` files. The private key is
  never baked into the image; it must be mounted or injected at runtime.

- **The credential server:** Binds to localhost only in devshell mode, requires
  a per-session Bearer token, and stores plaintext in a `chmod 700` tmpdir
  backed by `/dev/shm` when available.

### What to be careful about

- **`exportEnv = true`:** Secrets become shell environment variables, which can
  be leaked via `/proc/*/environ`, `ps eww`, or child process inheritance.
  Acceptable for local development; avoid in production.

- **`plaintext.dir`:** If you set a custom directory, it is your responsibility
  to ensure it has appropriate permissions and is cleaned up.

- **Container networking:** When running the credential server in a container,
  it binds to `0.0.0.0` so other containers in the network can reach it. Ensure
  the Docker network is trusted and the Bearer token is not exposed to
  untrusted workloads.

- **The age identity file:** `~/.himitsu/keys/age.txt` is the crown jewel. Never
  commit it, never copy it to insecure locations. Mount it read-only when
  possible.

### Cleanup guarantees

The `mkDevShell` registers an `EXIT` trap that:

1. Kills the credential server (if running)
2. Removes the secure tmpdir (all plaintext files)
3. Removes the custom `plaintext.dir` (if one was specified)

---

## Examples

### Minimal: just inject env vars

```nix
devShells.default = himitsu.lib.${system}.mkDevShell {
  store = ./.himitsu;
  env   = "dev";
};
```

### Wrap an existing devShell

```nix
devShells.default = himitsu.lib.${system}.mkDevShell {
  devShell = pkgs.mkShell {
    packages = [ pkgs.nodejs pkgs.yarn pkgs.postgresql ];
    shellHook = ''
      echo "Welcome to my project!"
    '';
  };
  store = ./.himitsu;
  env   = "dev";
};
```

### Multiple environments in one flake

```nix
let
  mkEnvShell = env: himitsu.lib.${system}.mkDevShell {
    devShell = myBaseShell;
    store = ./.himitsu;
    inherit env;
  };
in {
  devShells.${system} = {
    default = mkEnvShell "dev";
    staging = mkEnvShell "staging";
    prod    = mkEnvShell "prod";
  };
}
```

```bash
nix develop          # dev secrets
nix develop .#staging  # staging secrets
```

### Full-featured: credential server + AWS + SOPS

```nix
devShells.default = himitsu.lib.${system}.mkDevShell {
  devShell = pkgs.mkShell {
    packages = [ pkgs.awscli2 pkgs.terraform ];
  };
  store   = ./.himitsu;
  env     = "dev";
  verbose = true;
  credentialServer = {
    enable     = true;
    port       = 9292;
    awsProfile = "default";
    sops       = true;
  };
};
```

### Pre-pack secrets for caching

```nix
let
  devSecrets = himitsu.lib.${system}.packSecrets {
    src       = ./.himitsu;
    env       = "dev";
    mergeEnvs = [ "common" ];
  };
in {
  devShells.default = himitsu.lib.${system}.mkDevShell {
    secrets = devSecrets;
    devShell = myShell;
  };

  packages.dev-secrets = devSecrets;  # push to cache
}
```

### OCI credential server for staging

```nix
packages.staging-secrets-server =
  himitsu.lib.${system}.mkCredentialServerImage {
    store = ./.himitsu;
    env   = "staging";
    port  = 9292;
    name  = "ghcr.io/myorg/himitsu-secrets";
    tag   = "staging";
  };
```

### Use wrapAge independently

```nix
devShells.default = pkgs.mkShell {
  packages = [
    (himitsu.lib.${system}.wrapAge {})
  ];
  shellHook = ''
    # age --decrypt now auto-injects your himitsu identity
    age --decrypt .himitsu/vars/dev/TOKEN.age
  '';
};
```

### Merge multiple secret packs

```nix
let
  commonSecrets = himitsu.lib.${system}.packSecrets ./.himitsu/vars/common;
  devSecrets    = himitsu.lib.${system}.packSecrets ./.himitsu/vars/dev;
in
himitsu.lib.${system}.mkDevShell {
  secrets = [ commonSecrets devSecrets ];
  devShell = myShell;
};
```

---

## Troubleshooting

### "age identity not found"

```
⚠  himitsu: age identity not found at /Users/you/.himitsu/keys/age.txt
```

**Fix:** Run `himitsu init` to generate a keypair, or set `HIMITSU_IDENTITY` to
point at your existing key file.

### "N secret(s) failed to decrypt"

```
⚠  himitsu: 3 secret(s) failed to decrypt
```

This means your age identity can't decrypt some secrets. Common causes:

- Your public key isn't in the store's recipients list
- The secrets were encrypted for a different team/group
- **Fix:** Run `himitsu recipient add self --self` and then `himitsu encrypt`

### Credential server fails to start

Check the log file:

```bash
cat $_himitsu_tmpdir/.server-log
```

Common causes:

- Port already in use (try a different `credentialServer.port`)
- Python not available (shouldn't happen with `mkDevShell`, but check)

### Nix flake doesn't see my proto/nix files

```
error: Path 'nix/lib' in the repository is not tracked by Git.
```

**Fix:** `git add nix/lib/ proto/ build.rs` — Nix flakes only see Git-tracked
files.

### Secrets are stale after `himitsu set`

The `packSecrets` derivation is built from the Nix store snapshot of your
`.himitsu` directory. After changing secrets:

```bash
# Exit the current shell
exit

# Re-enter to pick up changes
nix develop
```

For a more dynamic workflow during development, use `himitsu get` directly:

```bash
export DATABASE_URL=$(himitsu get dev DATABASE_URL)
```
