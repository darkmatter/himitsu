# Key Providers Design: Native macOS Keychain and age-plugin (YubiKey) Identities

> Status: **Draft — awaiting approval** (HARD-GATE: no implementation until approved; the new
> dependencies in §9 need explicit sign-off per `AGENTS.md`)
> Created: 2026-09-05
> Bead: hm-a0y (epic) — approval gate hm-a0y.1, bug hm-a0y.2, phases A–E hm-a0y.3 … hm-a0y.7
> Supersedes nothing. Touches: `rust/src/keyring/`, `rust/src/crypto/`, `rust/src/config/`,
> `rust/src/cli/{init,doctor,keys,recipient}.rs`, `flake.nix`, `action/`.

## 1. Problem Statement

himitsu has one hardened place for the user's age private key — the macOS Keychain — and it is
implemented by shelling out to the `security` CLI (`rust/src/keyring/macos.rs`). That design has
four limits:

1. **No user-presence control.** Nothing can require Touch ID, the login password, or a
   re-authentication window before the key is released.
2. **Weak item ACL.** Items created by `security add-generic-password` list `/usr/bin/security` as
   a trusted application, so `security find-generic-password -w` returns the age key to any
   process running as the user without a prompt.
3. **Secret in argv.** `security add-generic-password -w <secret>` exposes the key in the process
   argument list for the lifetime of the call.
4. **macOS only, software only.** There is no hardware-bound option on any platform, and the
   crypto layer (`rust/src/crypto/age.rs`) is monomorphic on `age::x25519::{Identity, Recipient}`,
   so plugin identities (YubiKey, Secure Enclave, TPM, FIDO2) cannot participate in encryption or
   decryption at all.

This document proposes two provider tracks that can ship independently:

- **Track 1 — Native macOS Keychain provider:** Security.framework access (no subprocess), a
  himitsu-scoped item ACL, and a configurable in-process user-presence gate (Touch ID / password /
  Apple Watch) with a grace period.
- **Track 2 — age-plugin identity provider:** the private key lives on a device (YubiKey PIV slot
  first; Apple Secure Enclave via `age-plugin-se` as a near-free second target). himitsu persists
  only the non-secret identity stub and the public recipient.

## 2. Goals and Non-Goals

Goals:

- G1. Remove the `security` subprocess; store/read the age key through Security.framework with the
  himitsu binary as the only trusted application.
- G2. Configurable user-presence gate for keychain-backed keys: `none | user-presence | biometry`,
  a custom prompt, and a grace period ("timeout") that spans CLI invocations.
- G3. YubiKey-resident age identities with PIN and touch policies chosen at provisioning time,
  usable on macOS and Linux.
- G4. Provider-neutral crypto layer: decrypt with any mix of native and plugin identities, encrypt
  to any mix of native and plugin recipients, **without changing the store format**.
- G5. Safe migrations: `security`-CLI items → native items; disk/keychain → plugin identity via
  add-and-rekey (never a step that loses access).
- G6. `himitsu doctor` explains the active provider, its enforcement tier, and recovery risks.

Non-goals:

- Changing where encrypted secret payloads are stored (`docs/ARCHITECTURE.md` §14 stays true:
  keychain usage is for the local decryption key only).
- GNOME Keyring / Windows Credential Manager backends (the trait shape must not preclude them).
- A decryption agent or daemon.
- Shipping a code-signed, provisioned macOS app bundle (see §6.9 for why that would be required for
  OS-enforced Keychain biometrics, and why we defer it).

## 3. Current State

- `config::KeyProvider` is `Disk | MacosKeychain` (`rust/src/config/mod.rs:14`), resolved once at
  dispatcher boot into `Context.key_provider` (`rust/src/cli/mod.rs:661`).
- `crypto::keystore` is the single chokepoint: `store_new_key`, `load_identity`,
  `load_identities` (`rust/src/crypto/keystore.rs:57-163`). The keychain branch probes the current
  `key.pub` fingerprint, the legacy `DefaultHasher` fingerprint (one-shot migration), every
  recipient `.pub` in the store (rotated keys), then unions disk/SOPS fallbacks.
- `keyring::KeyProvider` trait (`store_key`, `load_key`) exists with a `MockKeyProvider`, but
  `keystore` calls `MacOSKeychain` directly, so the mock is only used in one unit test.
- `key.pub` is always written and is the provider-agnostic "is initialized" probe
  (`keystore::is_initialized`). Every design below preserves that invariant.
- Identity consumers that name `age::x25519::Identity` concretely: `cli/mod.rs`, `crypto/identity.rs`,
  `cli/resolver.rs`, `cli/rekey.rs`, `cli/export.rs`, `cli/exec.rs`, `cli/output_resolver.rs`,
  `cli/search.rs`, `cli/ls.rs`, `cli/tag.rs`, `cli/get.rs`, `cli/migrate.rs`, `cli/keys.rs`,
  `cli/doctor.rs`, `tui/harness.rs`, `tui/views/secret_viewer.rs`, `tui/views/search.rs`.
- Recipients are `.pub` files holding a recipient string; the secret envelope records recipients
  as strings (`remote/store.rs::AgeRecipientMeta.recipient`). Neither assumes the `age1…` native
  prefix beyond `crypto::age::parse_recipient`, which currently rejects anything else.
- Latent bug: `recipient add --self` reads the on-disk secret file via `ctx.key_path()`
  (`rust/src/cli/recipient.rs:145-150`), which does not exist under the keychain provider.

## 4. Platform Constraints That Shape the Design

### 4.1 macOS has two keychains, and only one of them supports biometrics

| | File-based ("login") keychain | Data-protection keychain |
|---|---|---|
| Reached by | `SecItem*` without `kSecUseDataProtectionKeychain` (macOS default) | `SecItem*` with `kSecUseDataProtectionKeychain = true` |
| Access control | Per-item ACL: list of trusted applications; foreign apps trigger a login-password prompt | `SecAccessControl` (`userPresence`, `biometryAny`, `biometryCurrentSet`) enforced by the Secure Enclave |
| Entitlements | None required | `com.apple.application-identifier` + `keychain-access-groups`, authorised by a provisioning profile |
| Unsigned / ad-hoc / cargo / Nix / Homebrew CLI | Works | `errSecMissingEntitlement (-34018)` |

Apple DTS is explicit that a bare command-line tool has nowhere to carry a provisioning profile;
the sanctioned workaround is to ship the tool inside an app-style bundle signed with a Developer ID.
That is a distribution project, not a code change, so **OS-enforced biometric gating of a Keychain
item is out of reach for every build channel himitsu has today** (see §6.9).

What *does* work from any process in a GUI login session, with no entitlements:
`LAContext.evaluatePolicy` (LocalAuthentication.framework) — a real Touch ID / password / Apple
Watch prompt whose result himitsu enforces in-process.

Consequences:

- Track 1 is an **in-process gate** on top of the file-based keychain. Its policy is himitsu
  configuration, so it can be changed at any time without re-storing the key (this corrects the
  earlier assumption that the policy is fixed at item creation — that is only true for
  `SecAccessControl` items).
- Truly OS-/hardware-enforced gating comes from Track 2 (YubiKey PIN/touch policy enforced by the
  token; Secure Enclave access control enforced by the SEP via `age-plugin-se`).

### 4.2 age plugins are the standard hardware path

- The `age` crate (already a dependency, 0.11) exposes the plugin protocol behind the `plugin`
  feature: `age::plugin::{Recipient, Identity, RecipientPluginV1, IdentityPluginV1}` and the
  `age::Callbacks` trait for PIN prompts and "touch your key" messages. Plugin discovery is by name:
  `age1<name>1…` / `AGE-PLUGIN-<NAME>-1…` → `age-plugin-<name>` on `PATH`.
- `age-plugin-yubikey` (str4d) stores an ECDSA P-256 key in a PIV retired slot (82–95) with a PIN
  policy (`never | once | always`) and touch policy (`never | always | cached`) fixed at
  generation. Identity files are non-secret pointers (serial + slot). Officially supports YubiKey 4
  and 5 series; Linux needs `pcscd`.
- `age-plugin-se` (remko) binds a P-256 key to the Mac's Secure Enclave with
  `--access-control none|passcode|any-biometry|current-biometry|…`, macOS 14+. It ships via
  Homebrew and needs no himitsu-side entitlements because the SE-wrapped key lives in the identity
  file, not in the keychain.
- SOPS ≥ 3.10.0 encrypts to and decrypts with plugin recipients/identities when the plugin binary
  is on `PATH` (relevant to `himitsu export` / `generate`, which shell out to `sops`).

## 5. Architecture Overview

```
Context::load_identities()  ──▶  IdentityResolver  ──▶  keystore::load_identities(provider)
                                                          ├─ Disk          → key file + SOPS fallbacks
                                                          ├─ MacosKeychain → keyring::native (SecItem) + user-presence gate
                                                          └─ AgePlugin     → identity stub → age::IdentityFile
                                                                ▼
                                            Vec<crypto::age::Identity>   (native | plugin, labelled)
                                                                ▼
                        crypto::age::decrypt_with_identities  /  crypto::age::encrypt(&[Recipient])
                                                                ▼
                              age::Decryptor / age::Encryptor over &dyn age::{Identity, Recipient}
```

Everything above the dashed line is unchanged. The provider fan-out gains one variant; the crypto
layer becomes trait-object based (§8.1). Store format, recipient files, and envelope metadata are
untouched.

## 6. Track 1 — Native macOS Keychain Provider

### 6.1 Backend

Replace `keyring/macos.rs`'s `Command::new("security")` with the `security-framework` crate
(`passwords::{set_generic_password, generic_password, delete_generic_password}` without
`use_protected_keychain()`, i.e. the file-based keychain). Compiled only for
`cfg(target_os = "macos")`; the `KeyProvider::MacosKeychain` config value keeps erroring on other
platforms exactly as `ensure_keychain_available` does today.

Effects: no secret in argv; the item's trusted-application ACL is the himitsu binary, so
`security find-generic-password` and other processes hit the macOS "wants to use your confidential
information" login-password prompt instead of reading silently.

### 6.2 Item layout

| | Legacy (today) | Native (new) |
|---|---|---|
| Service | `io.darkmatter.himitsu.agekey.byfp.v1` | `io.darkmatter.himitsu.agekey.byfp.v2` |
| Account | SHA-256 fingerprint of `key.pub` (`keyring::fingerprint`) | same |
| Value | `AGE-SECRET-KEY-1…` | same |
| ACL | trusts `/usr/bin/security` | trusts himitsu only |

A new service name is required because an item's ACL is fixed at creation; `SecItemUpdate` would
keep the permissive legacy ACL. Distinct names also let `doctor` detect leftovers.

### 6.3 User-presence gate

`keychain.require_auth` selects an `LAContext` policy evaluated **before** the key is read:

| Value | `LAPolicy` | Prompt |
|---|---|---|
| `none` (default) | — | none; behaviour identical to today |
| `user-presence` | `DeviceOwnerAuthentication` | Touch ID, falling back to the login password |
| `biometry` | `DeviceOwnerAuthenticationWithBiometrics` | Touch ID only (fails if not enrolled / locked out) |

`keychain.prompt` is the `localizedReason`; macOS renders it as "himitsu is trying to <prompt>".
The gate runs inside `keystore::load_identities` for the keychain branch, so every code path (CLI
and TUI) goes through it once per identity load. `evaluatePolicy` is asynchronous; himitsu blocks
on a channel with a 60 s timeout and maps cancellation/timeouts to a new
`HimitsuError::AuthRequired` variant.

### 6.4 Grace period ("timeout")

`keychain.grace_period` (default `0s`) is a himitsu-managed re-authentication window that spans
processes: after a successful evaluation, himitsu writes `<state_dir>/keychain-auth-grant`
(mode 0600) containing `expires_at` and the kernel boot time; subsequent loads within the window
and the same boot skip the prompt. Two implementation notes:

- `LAContext.touchIDAuthenticationAllowableReuseDuration` is set to `min(grace_period, 5m)` so a
  Touch ID *device unlock* moments earlier also satisfies the gate (Apple's semantics; it never
  reuses an in-app match).
- The grant file is forgeable by any process running as the user. That is consistent with the
  Track 1 trust model (§6.8); it is a convenience knob, not a security boundary.

The TUI, being a long-lived process, authenticates on each identity load unless a grace period is
set — this is deliberate; `grace_period: 5m` is the recommended setting for TUI-heavy use.

### 6.5 Configuration

```yaml
key_provider: macos-keychain

keychain:                                   # read only when key_provider = macos-keychain
  require_auth: user-presence               # none | user-presence | biometry
  prompt: "unlock your himitsu age key"     # shown as: himitsu is trying to <prompt>
  grace_period: 5m                          # 0s = prompt on every key load
```

Env overrides follow the existing rule (`config/mod.rs:47`): `HIMITSU_KEYCHAIN_REQUIRE_AUTH`,
`HIMITSU_KEYCHAIN_PROMPT`, `HIMITSU_KEYCHAIN_GRACE_PERIOD`. `example.yaml` and the README
"Configuration" section document the block.

### 6.6 Migration: legacy items → native

On first keychain access with the native backend, if no `v2` item exists for the fingerprint:
read the `v1` item natively (macOS prompts once because himitsu is not in the legacy ACL), write it
as a `v2` item, delete the `v1` item, and print `✓ Migrated keychain entry to native storage`.
This mirrors the existing `fingerprint_v1_legacy` migration. `doctor` warns while any `v1` items
remain. The `security`-CLI code is deleted, not kept as a fallback.

### 6.7 Non-interactive sessions

Over SSH, in tmux started from SSH, or in CI, `LAContext` fails with a not-interactive error.
himitsu surfaces `AuthRequired` with a hint: use `HIMITSU_KEYCHAIN_REQUIRE_AUTH=none` for this
session, or use a disk-provider key for automation (the GitHub Action already does). `doctor`
reports whether the current session can evaluate the configured policy
(`LAContext.canEvaluatePolicy`).

### 6.8 Trust model (what Track 1 does and does not defend)

| Threat | Legacy `security` CLI | Native, `require_auth: none` | Native, `user-presence` |
|---|---|---|---|
| Key visible in `ps` during store | yes | no | no |
| Another process reads the item silently | yes (via `security`) | no — login-password prompt | no — login-password prompt |
| Someone at your unlocked Mac runs `himitsu get` | succeeds | succeeds | Touch ID / password required |
| Unattended script or coding agent runs `himitsu exec` | succeeds | succeeds | blocked until a human authenticates |
| Malware running as you edits `~/.config/himitsu/config.yaml` or the grant file | n/a | n/a | bypasses the gate (in-process enforcement) |
| Malware calls `SecItemCopyMatching` directly | silent read | login-password prompt | login-password prompt |

Complementary OS-enforced control that exists today: Keychain Access → login keychain → "Change
Settings" can lock the keychain after inactivity or on sleep, after which any read (including
himitsu's) requires the login password. This is worth documenting alongside the new options.

### 6.9 Why not `SecAccessControl` now

Data-protection-keychain items with `userPresence` / `biometryCurrentSet` would give OS-enforced
gating, but only from a binary carrying restricted entitlements, which means an Apple Developer
Program membership, a Developer ID certificate, a provisioning profile, and shipping himitsu as
`Himitsu.app/Contents/MacOS/himitsu` with a symlink on `PATH`. Nix and cargo installs could never
qualify. Since Track 2 delivers hardware-enforced gating (YubiKey) and SEP-enforced biometrics
(`age-plugin-se`) without any of that, the recommendation is to **not** pursue a data-protection
backend. The native backend keeps a `KeychainBackend` enum with a single `FileBased` variant so a
future signed distribution could add `DataProtection` without touching callers.

Ad-hoc signing caveat: cargo and Nix builds on Apple Silicon are ad-hoc signed, so every rebuild or
upgrade is a "new application" to the keychain ACL and triggers one "Always Allow" prompt. Release
builds should be signed with a stable identity (self-signed is enough for ACL stability); the dev
shell can offer a `just sign` recipe. This is documented, not solved.

## 7. Track 2 — age-plugin Identity Provider (YubiKey first)

### 7.1 Model

`key_provider: age-plugin`. himitsu persists two non-secret files in `data_dir`:

- `key.pub` — the recipient string (`age1yubikey1…`, `age1se1…`, or `age1tag1…`). Existing
  invariant preserved: this is still the initialization probe and the value `join` / `recipient
  add --self` publish.
- `plugin-identity.txt` — the plugin identity file exactly as the plugin emitted it (comment
  header plus `AGE-PLUGIN-<NAME>-1…`), mode 0600. For YubiKey it encodes serial + slot; for SE it
  holds the SE-wrapped key that is useless off-device. It is a pointer, not key material, but is
  treated as private to avoid leaking device inventory.

Decryption requires the device; encryption to the recipient requires only the plugin binary (no
hardware), which matters for teammates and CI.

### 7.2 Provisioning

All flows shell out to the plugin binary (same pattern as `git`, `sops`, `op` →
`HimitsuError::External`); himitsu never speaks PC/SC itself.

| Flow | Command | What himitsu runs |
|---|---|---|
| Generate on a YubiKey | `himitsu init --key-provider age-plugin --plugin yubikey [--slot 82] [--pin-policy once] [--touch-policy always] [--serial N]` | `age-plugin-yubikey --generate --name himitsu --slot … --pin-policy … --touch-policy … [--serial …]`; parses `# Recipient:` and the identity line |
| Reuse an existing slot (new laptop, same key) | `himitsu init --key-provider age-plugin --plugin yubikey --slot 82 [--serial N]` when the slot already holds an age key | `age-plugin-yubikey --identity --slot … [--serial …]` |
| Import any plugin identity (SE, TPM, FIDO2, or a file made elsewhere) | `himitsu init --key-provider age-plugin --identity-file key.txt [--recipient age1…]` | none; recipient taken from `# public key:` / `# Recipient:` comment, `--recipient` required if absent |

Defaults: slot = first free retired slot reported by `--list-all`, `pin_policy = once`,
`touch_policy = always`. The plugin handles first-run hygiene itself (forces a PIN change off the
default, rotates the default management key into PIN-protected metadata).

The TUI init wizard gains the provider option only when a plugin binary is on `PATH`; slot and
policy selection in the wizard is a follow-up phase — CLI flags ship first.

### 7.3 Runtime loading and callbacks

`keystore::load_identities` for `AgePlugin` reads `plugin-identity.txt` through
`age::IdentityFile::from_buffer(..).with_callbacks(cb).into_identities()`, which yields the
`IdentityPluginV1` for the right plugin. Disk/SOPS fallbacks and store-recipient probing still
apply, so a previous software identity remains usable during rotation (§7.6).

`age::Callbacks` implementations:

- `TerminalCallbacks` (CLI): PIN via a no-echo stderr prompt (`cliclack` is already a dependency),
  messages such as "touch your YubiKey" to stderr.
- `TuiCallbacks` (TUI): a PIN modal and a status line. Until that view exists, the TUI's callbacks
  return `None` for passphrase requests and the plugin error is shown with a hint to use
  `pin_policy: never|once` or the CLI. Secure Enclave needs no callbacks (Touch ID is a system
  dialog), so SE works in the TUI immediately.

### 7.4 Encryption to plugin recipients

`crypto::age::parse_recipient` accepts native `age1…` and plugin `age1<name>1…` strings.
`encrypt` groups plugin recipients by plugin name into one `RecipientPluginV1` each and passes
natives directly. If a plugin binary is missing, the error names the binary and the recipient
(`age-plugin-yubikey not found in PATH; required to encrypt for recipient ops/alice`). Recipient
files and envelope metadata already store strings, so `recipient add --age-key age1yubikey1…`,
`rekey`, and `set` need no format change.

### 7.5 Policies and what "timeout" means here

| Control | Values | Enforced by | Time behaviour |
|---|---|---|---|
| PIN policy | `never`, `once`, `always` | YubiKey PIV applet | `once` = cached until the key is unplugged or another applet (e.g. FIDO2) is used; YubiKey 4 cannot preserve the cache across plugin invocations |
| Touch policy | `never`, `always`, `cached` | YubiKey | `cached` = ~15 s hardware window |
| SE access control | `none`, `passcode`, `any-biometry`, `current-biometry`, combinations | Secure Enclave | per operation; no software timeout |

Both are fixed when the key is generated. Changing a policy means generating a new slot identity
and rotating (§7.6). himitsu adds no software cache on top — that would defeat the purpose.

### 7.6 Rotation and recovery guardrails

Switching an already-initialized machine to `age-plugin`:

1. `init --key-provider age-plugin …` writes the new `key.pub` and identity stub. The previous
   disk or keychain identity is still loaded (fallback union), so nothing stops decrypting.
2. Per store: `himitsu join` publishes the new recipient, `himitsu rekey` re-encrypts.
3. Optionally `recipient rm <old>` + `rekey`, then delete the old key material manually
   (`keys private` still prints it while the fallback is present).

Guardrails (cheap, no new state): `init` prints a recovery warning when the new recipient is
hardware-bound; `doctor` warns for any store whose entire recipient set is hardware-bound
(`age1yubikey1…`, `age1se1…`, `age1tag1…`) and recommends a second recipient (backup YubiKey or an
offline, passphrase-protected age key). Lost/reset hardware with no second recipient is
unrecoverable by design, and the tooling should say so before it happens.

### 7.7 Secure Enclave via `age-plugin-se`

Because plugin dispatch is by name, supporting `age-plugin-se` costs only the import flow in §7.2
and a `--plugin se` convenience that runs `age-plugin-se keygen --access-control <ac>`. This is the
route to **OS-enforced Touch ID for himitsu on Apple Silicon without code-signing infrastructure**,
so it is included as a documented, manually tested target rather than a separate design. The
device-bound key makes the §7.6 second-recipient guidance mandatory in the docs.

### 7.8 Configuration

```yaml
key_provider: age-plugin

age_plugin:                                        # optional; defaults shown
  identity_file: ~/Library/Application Support/himitsu/plugin-identity.txt
```

No runtime `yubikey:` block: serial, slot, and policies are provisioning-time inputs encoded on the
token and in the identity stub. Env override: `HIMITSU_AGE_PLUGIN_IDENTITY_FILE`.

### 7.9 Linux

`age-plugin` is the first hardened provider available on Linux (`macos-keychain` remains
macOS-only). Requirements: `age-plugin-yubikey` on `PATH` and a running `pcscd`. `doctor` checks
both.

## 8. Shared Changes

### 8.1 Crypto layer generalization (zero behaviour change, lands first)

```rust
// crypto/age.rs
pub struct Recipient { text: String, kind: RecipientKind }   // Display = text (dedupe relies on it)
enum RecipientKind { Native(age::x25519::Recipient), Plugin(age::plugin::Recipient) }

pub struct Identity { pub label: String, inner: Box<dyn age::Identity> }
// label: native → to_public(); plugin → recipient from the identity file header, else
// "<plugin> identity" — used by doctor and the `no matching key` message in cli/resolver.rs.

pub fn parse_recipient(&str) -> Result<Recipient>;
pub fn encrypt(&[u8], &[Recipient]) -> Result<Vec<u8>>;               // groups plugins by name
pub fn decrypt_with_identities(&[u8], &[Identity]) -> Result<Vec<u8>>;
pub fn read_identities(&Path) -> Result<Vec<Identity>>;               // via age::IdentityFile
```

The ~17 files that name `age::x25519::Identity` switch to `crypto::age::Identity`; `keygen` and
the disk provider are unchanged. This is a mechanical PR verified by the existing suite.

### 8.2 `keyring::KeyProvider` trait becomes the real seam

Add `delete_key`, route `keystore` through `&dyn KeyProvider` (native on macOS, `MockKeyProvider`
in tests), and put the user-presence gate behind a small `UserPresence` trait with a mock, so the
gate/grace-period logic is unit-tested without a GUI session.

### 8.3 Command surface

| Command | Change |
|---|---|
| `init` | `--key-provider age-plugin`, `--plugin`, `--slot`, `--serial`, `--pin-policy`, `--touch-policy`, `--identity-file`, `--recipient`; keychain path migrates `v1` → `v2` |
| `doctor` | provider + backend + enforcement tier; legacy `v1` items; `canEvaluatePolicy` for the configured gate; plugin binary path/version; configured token present (`age-plugin-yubikey --list`); hardware-only recipient sets; `pcscd` on Linux |
| `keys private` | for `age-plugin`, prints the identity stub with a stderr note that it is device-bound, not key material |
| `recipient add --self` | reads `key.pub` instead of the disk secret file (fixes the existing keychain bug) |
| TUI wizard | provider option shown when a plugin binary is present; slot/policy steps in a later phase |

## 9. Dependencies and Packaging (require approval)

| Dependency | Scope | Purpose |
|---|---|---|
| `age` feature `plugin` | all targets | plugin recipients/identities (feature flag on an existing dep) |
| `security-framework` (+ `core-foundation`) | `cfg(target_os = "macos")` | native keychain items |
| `objc2-local-authentication`, `objc2-foundation`, `block2` | `cfg(target_os = "macos")` | `LAContext` gate |
| `age-plugin-yubikey` (runtime tool) | dev shell, Nix package `propagatedBuildInputs` or wrapper, GitHub Action closure | provisioning and encrypt/decrypt via plugin |
| `age-plugin-se` (runtime tool, optional) | docs; Homebrew/nixpkgs | Secure Enclave identities |
| `pcscd` / `pcsclite` | Linux docs | YubiKey transport |

Removed: all `security` CLI invocations. The GitHub Action needs the plugin binary because rekeying
a store that contains a hardware recipient encrypts to it (no hardware required). If the Rust `age`
crate does not natively handle `age1tag1…` recipients, packaging also provides an
`age-plugin-tag` alias (validate in Phase A spike).

## 10. Rollout Plan

Each phase is one PR, gated by `cargo fmt --all -- --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace`, and `nix flake check` when `flake.nix`
changes. Phases are tracked as `hm-a0y.3` (A) through `hm-a0y.7` (E), all blocked on the approval
decision `hm-a0y.1`; `docs/IMPLEMENTATION_PLAN.md` Phase 2 keychain items are updated when Phase B
lands.

| Phase | Scope | Exit criteria |
|---|---|---|
| A | §8.1 crypto generalization, §8.2 trait seam, `recipient add --self` fix, spikes: `age1tag1` handling, `age-plugin-yubikey` encrypt-without-pcscd | suite green, no behaviour change, spike findings recorded in this doc |
| B | native keychain backend, `v1`→`v2` migration, doctor updates, `require_auth: none` default | existing keychain users see one migration line and no other change |
| C | `LAContext` gate, grace period, config/env, README + `example.yaml`, non-interactive error path | manual matrix in §11 passes; unit tests for policy/grace logic |
| D | `age-plugin` provider: init flows, identity stub, loading, plugin recipient parsing, `TerminalCallbacks`, doctor checks, flake/action packaging, docs | YubiKey 5 manual matrix passes on macOS and Linux; CI encrypts to a plugin recipient with the plugin binary present |
| E | TUI: wizard provisioning steps, `TuiCallbacks` PIN modal; `age-plugin-se` documented flow | TUI reveal/copy works with `pin_policy: once` and with SE |

## 11. Testing Strategy

- Unit (CI, all platforms): recipient parsing for native/plugin strings; plugin grouping in
  `encrypt`; identity labelling; grace-period/policy state machine via the `UserPresence` mock;
  keystore fan-out via `MockKeyProvider`; identity-file header parsing; init flag validation.
- Integration (CI): existing `tests/integration/cli_test.rs` patterns with `HIMITSU_HOME`; new
  cases for `--key-provider age-plugin --identity-file` using a checked-in identity file with a
  fake `age-plugin-fake` shim only where the protocol exchange is not exercised (e.g. init,
  doctor, keys). Missing-plugin error text is asserted.
- Gated integration (`HIMITSU_TEST_KEYCHAIN=1`, local macOS only): native store/load/delete,
  `v1`→`v2` migration against a real login keychain. Headless CI keychains are unreliable, so
  these do not run in CI.
- Manual matrix (recorded in the PR): Touch ID prompt / password fallback / cancel / SSH session
  for each `require_auth`; grace period across two invocations and across reboot; YubiKey 5 with
  each PIN and touch policy on macOS and Linux; unplug mid-session; `age-plugin-se` any-biometry;
  `himitsu export` through `sops ≥ 3.10` with a hardware recipient.

## 12. Compatibility and Migration Matrix

| Situation | Behaviour |
|---|---|
| Existing `disk` users | unchanged |
| Existing `macos-keychain` users | one-time ACL prompt and migration line on first run after upgrade; `require_auth` defaults to `none` |
| Stores with only native recipients | unchanged; no plugin binary needed anywhere |
| Stores gaining a hardware recipient | every encrypting party (teammates, CI) needs the plugin binary on `PATH`; `sops ≥ 3.10` for `export`/`generate` consumers |
| `SOPS_AGE_KEY_CMD` / `age-key-cmd` Nix helper | unchanged (disk key file) |
| Downgrade to a himitsu without this work | `v2` keychain items are invisible to the old `security` path → run `keys private` first and re-store, or keep the disk key |

## 13. Risks and Open Questions

- `age` crate plugin API is pre-1.0; pin the minor version and wrap it in `crypto::age` only.
- Rust `age` may not natively parse `age1tag1…` (Go age ≥ 1.3 does); the `age-plugin-tag` alias
  is the fallback. Validate in Phase A.
- `LAContext` from processes without WindowServer access fails; the error path must be obvious
  (§6.7).
- himitsu executes `age-plugin-*` from `PATH`, the same trust posture as age/rage/sops. `doctor`
  prints the resolved path; a pinned `age_plugin.binary_dir` is possible later if needed.
- Ad-hoc-signed builds re-prompt for keychain ACL after each rebuild (§6.9).
- Open: default `grace_period` when the TUI is the primary interface (`0s` is proposed; `5m` is
  the documented recommendation).
- Open: whether `init --plugin yubikey` should refuse to proceed when the store it is about to
  `join` has no second recipient, or only warn (proposal: warn; `doctor` repeats it).

## 14. Decisions to Record (ADR-0003, on approval)

1. Keychain-backed keys use the file-based keychain with an in-process user-presence gate; a
   data-protection backend is not pursued because it requires a signed, provisioned bundle.
2. Hardware- and SEP-enforced gating is delivered through the age plugin ecosystem
   (`age-plugin-yubikey`, `age-plugin-se`), not through himitsu-specific device code.
3. himitsu never stores hardware key material; it stores the plugin identity stub plus `key.pub`,
   and the store format remains recipient-string based.
4. Provider policies live in himitsu config (keychain gate) or on the device (plugin policies);
   himitsu adds no software cache in front of device-enforced policies.

## 15. References

- Apple: Accessing Keychain Items with Face ID or Touch ID; Restricting keychain item
  accessibility; TN3137 On Mac keychain APIs and implementations; DTS forum thread on
  `-34018 errSecMissingEntitlement` from command-line tools.
- `age` crate 0.11 docs: `age::plugin`, `age::IdentityFile`, `age::Callbacks`.
- `security-framework` crate: `passwords`, `access_control`.
- `objc2-local-authentication` crate: `LAContext`, `LAPolicy`.
- str4d/age-plugin-yubikey README (PIN/touch policies, PIN cache semantics, `age1tag` note).
- remko/age-plugin-se README (access-control options, device-bound key, backup guidance).
- getsops/sops PR #1641 (age plugin support, milestone 3.10.0).
