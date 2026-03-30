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
# Sections:
#   1. Help            6. List secrets    11. Decrypt rejected  14. --remote flag
#   2. Initialize      7. Rekey           12. File layout       15. Sync
#   3. Git integration 8. Recipients      13. Remote add
#   4. Set secrets     9. Groups
#   5. Get secrets    10. Search
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

# Grab the public key for later use (needed for recipient section)
PUBKEY=$(grep "# public key:" "$DEMO_HOME/share/key" 2>/dev/null | cut -d' ' -f4 || \
         grep "# public key:" "$DEMO_HOME/key" 2>/dev/null | cut -d' ' -f4 || \
         grep "# public key:" "$DEMO_HOME/keys/age.txt" 2>/dev/null | cut -d' ' -f4 || \
         find "$DEMO_HOME" -name "key" -o -name "age.txt" 2>/dev/null | \
           xargs grep -l "public key" 2>/dev/null | \
           xargs grep "# public key:" 2>/dev/null | head -1 | cut -d' ' -f4)

# ----------------------------------------------------------
banner "3. Git integration"
h git init
h git status

# ----------------------------------------------------------
banner "4. Set secrets (path-based)"
h set prod/API_KEY "sk_live_abc123"
h set prod/DB_PASSWORD "hunter2"
h set dev/DB_PASSWORD "devpass"

# ----------------------------------------------------------
banner "5. Get secrets back"
h get prod/API_KEY
h get prod/DB_PASSWORD

# ----------------------------------------------------------
banner "6. List secrets"
h ls
h ls prod

# ----------------------------------------------------------
banner "7. Rekey (re-encrypt for current recipients)"
h rekey

note "Verify secrets survive re-encryption:"
h get prod/API_KEY

# ----------------------------------------------------------
banner "8. Recipient management"
h recipient ls
h recipient add alice --age-key "$PUBKEY" --group common
h recipient ls

# ----------------------------------------------------------
banner "9. Group management"
h group add admins
h group ls

# ----------------------------------------------------------
banner "10. Search"
h search DB --refresh

# ----------------------------------------------------------
banner "11. Decrypt is rejected (no plaintext at rest)"
type_cmd "himitsu decrypt"
"$HIMITSU_BIN" -s "$DEMO_STORE" decrypt 2>&1 || true
echo
sleep 0.15

# ----------------------------------------------------------
banner "12. File layout"
type_cmd 'find $HIMITSU_HOME -type f | sort'
find "$DEMO_HOME" -type f | sed "s|$DEMO_HOME|~/.himitsu|" | sort
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
"$HIMITSU_BIN" -s "$UPSTREAM_DIR" set prod/SHARED_API_KEY "team-key-abc-789" > /dev/null 2>&1
"$HIMITSU_BIN" -s "$UPSTREAM_DIR" set prod/SHARED_DB_URL "postgres://db.internal/app" > /dev/null 2>&1
git -C "$UPSTREAM_DIR" add -A 2>/dev/null
git -C "$UPSTREAM_DIR" commit -m "himitsu: add team secrets" -q 2>/dev/null

# ----------------------------------------------------------
banner "13. Remote add — register a team secrets repository"

note "Register a shared secrets repository as a named remote:"
type_cmd "himitsu remote add acme/infra --url \$UPSTREAM_DIR"
"$HIMITSU_BIN" -s "$DEMO_STORE" remote add acme/infra --url "$UPSTREAM_DIR" || true
echo
sleep 0.15

# ----------------------------------------------------------
banner "14. --remote (-r) — select a remote store inline"

note "Inspect any registered remote store without switching projects:"
h_bare -r acme/infra ls
h_bare -r acme/infra get prod/SHARED_API_KEY

# ----------------------------------------------------------
banner "15. Sync — mirror encrypted files into the local store"

note "Sync the registered remote into the local store:"
h sync acme/infra

note "Synced secrets are now accessible:"
h ls
h get prod/SHARED_API_KEY

printf "\n${BOLD}${GREEN}Done. All operations completed successfully.${RESET}\n\n"
sleep 0.3
