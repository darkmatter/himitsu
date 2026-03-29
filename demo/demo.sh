#!/usr/bin/env bash
set -euo pipefail
#
# demo.sh — Run the himitsu demo live in your terminal.
#
# For pauseable playback (spacebar = pause, q = quit), use the
# pre-recorded asciicast instead:
#
#   ./demo/record.sh                # (re)generate demo.cast
#   asciinema play demo/demo.cast   # spacebar to pause
#

# ── Helpers ───────────────────────────────────────────────
HIMITSU_BIN="$(cd "$(dirname "$0")/.." && pwd)/target/release/himitsu"
DEMO_HOME="$(mktemp -d)"
DEMO_STORE="$DEMO_HOME/store/.himitsu"
export HIMITSU_HOME="$DEMO_HOME"

GREEN='\033[0;32m'
CYAN='\033[0;36m'
DIM='\033[2m'
BOLD='\033[1m'
RESET='\033[0m'

type_cmd() {
	printf "${BOLD}${GREEN}$ ${RESET}${BOLD}"
	local display="$*"
	for ((i = 0; i < ${#display}; i++)); do
		printf '%s' "${display:$i:1}"
		sleep 0.008
	done
	printf "${RESET}\n"
	sleep 0.05
}

# Display 'himitsu ...' but run the actual binary with -s pointing at our store
h() {
	type_cmd "himitsu $*"
	"$HIMITSU_BIN" -s "$DEMO_STORE" "$@"
	echo
	sleep 0.15
}

# Run himitsu without the implicit -s (for commands that don't need a store)
h_bare() {
	type_cmd "himitsu $*"
	"$HIMITSU_BIN" "$@"
	echo
	sleep 0.15
}

run() {
	type_cmd "$@"
	eval "$@"
	echo
	sleep 0.15
}

banner() {
	echo
	printf "${CYAN}${BOLD}# ── %s ──${RESET}\n" "$1"
	echo
	sleep 0.1
}

note() {
	printf "${DIM}# %s${RESET}\n" "$1"
}

cleanup() { rm -rf "$DEMO_HOME"; }
trap cleanup EXIT

# ── Demo ──────────────────────────────────────────────────

clear
printf "${BOLD}${CYAN}"
cat <<'ART'
  _     _           _ _
 | |__ (_)_ __ ___ (_) |_ ___ _   _
 | '_ \| | '_ ` _ \| | __/ __| | | |
 | | | | | | | | | | | |_\__ \ |_| |
 |_| |_|_|_| |_| |_|_|\__|___/\__,_|

 age-based secrets management
ART
printf "${RESET}\n"
sleep 0.3

# ----------------------------------------------------------
banner "1. Help"
h_bare --help

# ----------------------------------------------------------
banner "2. Initialize"
h init

note "An age keypair has been generated and the store scaffolded."
sleep 0.1

# Grab the public key for later use
PUBKEY=$(grep "# public key:" "$DEMO_HOME/keys/age.txt" | cut -d' ' -f4)

# ----------------------------------------------------------
banner "3. Version-control with git"
h git init
h git status

# ----------------------------------------------------------
banner "4. Set secrets"
h set prod API_KEY "sk_live_abc123"
h set prod DB_PASSWORD "hunter2"
h set dev DB_PASSWORD "devpass"
h set common API_BASE_URL "https://api.example.com"

# ----------------------------------------------------------
banner "5. Get secrets back"
h get prod API_KEY
h get prod DB_PASSWORD
h get dev DB_PASSWORD

# ----------------------------------------------------------
banner "6. List environments"
h ls

# ----------------------------------------------------------
banner "7. List keys in an environment"
h ls prod
h ls common

# ----------------------------------------------------------
banner "8. Re-encrypt for current recipients"
h encrypt

note "Verify secrets survive re-encryption:"
h get prod API_KEY

# ----------------------------------------------------------
banner "9. Recipient management"
h recipient ls
h recipient add alice --age-key "$PUBKEY" --group common
h recipient ls

# ----------------------------------------------------------
banner "10. Group management"
h group add admins
h group add staging
h group ls

# ----------------------------------------------------------
banner "11. Search across stores"
h search DB --refresh
h search API --refresh

# ----------------------------------------------------------
banner "12. Schema generation"
h schema list
h schema dump config
echo

note "Write all schemas to the store:"
h schema refresh

run "ls $DEMO_STORE/schemas/"

# ----------------------------------------------------------
banner "13. Codegen"

note "Generate TypeScript types from store contents:"
h codegen --lang typescript --env prod --stdout

note "Generate Go code:"
h codegen --lang golang --env prod --stdout

note "Generate Python dataclasses:"
h codegen --lang python --env prod --stdout

note "Generate Rust types:"
h codegen --lang rust --env prod --stdout

# ----------------------------------------------------------
banner "14. Codegen to file"

CODEGEN_OUT="$DEMO_HOME/generated/secrets.ts"
h codegen --lang typescript --env prod --output "$CODEGEN_OUT"

run "cat $CODEGEN_OUT"

# ----------------------------------------------------------
banner "15. Decrypt is rejected (no plaintext at rest)"
type_cmd "himitsu decrypt"
"$HIMITSU_BIN" -s "$DEMO_STORE" decrypt 2>&1 || true
echo
sleep 0.15

# ----------------------------------------------------------
banner "16. File layout"
type_cmd "find \$HIMITSU_HOME -type f | sort"
find "$DEMO_HOME" -type f | sed "s|$DEMO_HOME|\~/.himitsu|" | sort
echo

# ── Offline setup: local "upstream" store for remote/sync sections ────────────
# Creates a self-contained git repo whose himitsu store is at its root —
# the layout that `remote add` clones and `-r` / `sync` read from.
UPSTREAM_DIR="$DEMO_HOME/demo-remote"
mkdir -p "$UPSTREAM_DIR"
git -C "$UPSTREAM_DIR" init -q 2>/dev/null
git -C "$UPSTREAM_DIR" config user.email "demo@example.com"
git -C "$UPSTREAM_DIR" config user.name "Demo"
"$HIMITSU_BIN" -s "$UPSTREAM_DIR" init > /dev/null 2>&1
"$HIMITSU_BIN" -s "$UPSTREAM_DIR" set prod SHARED_API_KEY "team-key-abc-789" > /dev/null 2>&1
"$HIMITSU_BIN" -s "$UPSTREAM_DIR" set prod SHARED_DB_URL "postgres://db.internal/app" > /dev/null 2>&1
git -C "$UPSTREAM_DIR" add -A 2>/dev/null
git -C "$UPSTREAM_DIR" commit -m "himitsu: add team secrets" -q 2>/dev/null

# ----------------------------------------------------------
banner "17. Remote add — register a team secrets repository"

note "Register a shared secrets repository as a named remote:"
type_cmd "himitsu remote add acme/infra --url \$UPSTREAM_DIR"
"$HIMITSU_BIN" -s "$DEMO_STORE" remote add acme/infra --url "$UPSTREAM_DIR" || true
echo
sleep 0.15

# ----------------------------------------------------------
banner "18. --remote (-r) — select a remote store inline"

note "Inspect any registered remote store without switching projects:"
h_bare -r acme/infra ls
h_bare -r acme/infra ls prod
h_bare -r acme/infra get prod SHARED_API_KEY

# ----------------------------------------------------------
banner "19. Sync — mirror encrypted files into the project store"

note "Bind the current project store to the remote:"
h sync --bind acme/infra

note "Mirror all environments from the bound remote (no decryption):"
h sync

note "Synced secrets are now accessible in the local store:"
h ls prod
h get prod SHARED_API_KEY

printf "\n${BOLD}${GREEN}Done. All operations completed successfully.${RESET}\n\n"
sleep 0.3
