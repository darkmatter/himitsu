#!/usr/bin/env bash
set -euo pipefail
#
# record.sh — Generate demo/demo.cast (asciicast v2) from live CLI output.
#
# Usage:
#   ./demo/record.sh                  # writes demo/demo.cast
#   asciinema play demo/demo.cast     # spacebar to pause, q to quit
#
# The script runs every command against a real (temp) himitsu store,
# captures output, and emits a deterministic .cast file with realistic
# typing animation.  No TTY or asciinema-rec session required.
#
# Sections (16 total):
#   1. Help             6. List secrets     10. Decrypt rejected
#   2. Initialize       7. Rekey            11. File layout
#   3. Git integration  8. Recipients       12. Remote add
#   4. Set secrets      9. Search           13. --remote flag
#   5. Get secrets                          14. Sync
#  15. Tags                                 16. Exec (inject env vars)
#

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HIMITSU_BIN="$PROJECT_ROOT/target/release/himitsu"
CAST_FILE="$SCRIPT_DIR/demo.cast"

# ── Pre-flight ────────────────────────────────────────────
if [[ ! -x "$HIMITSU_BIN" ]]; then
  echo "error: $HIMITSU_BIN not found — run 'cargo build --release' first" >&2
  exit 1
fi

DEMO_HOME="$(mktemp -d)"
DEMO_STORE="$DEMO_HOME/store/.himitsu"
export HIMITSU_CONFIG="$DEMO_HOME/config.yaml"

cleanup() { rm -rf "$DEMO_HOME"; }
trap cleanup EXIT

# ── Cast writer state ────────────────────────────────────
CAST_WIDTH=120
CAST_HEIGHT=40
TIME=0.0          # current virtual clock (seconds)

# Portable floating-point add (awk, no bc dependency)
ts_add() { TIME=$(awk "BEGIN{printf \"%.4f\", $TIME + $1}"); }

# Escape a string for JSON: handle backslash, quotes, control chars.
# Converts bare \n → \r\n so the terminal renders lines without staircase.
json_escape() {
  printf '%s' "$1" | python3 -c '
import json, sys
text = sys.stdin.read()
# Normalize to \r\n for proper terminal rendering in asciicast playback.
# First collapse any existing \r\n to \n, then convert all \n to \r\n.
text = text.replace("\r\n", "\n").replace("\n", "\r\n")
sys.stdout.write(json.dumps(text))
'
}

# Write the v2 header
write_header() {
  local ts
  ts=$(date +%s)
  cat > "$CAST_FILE" <<EOF
{"version": 2, "width": ${CAST_WIDTH}, "height": ${CAST_HEIGHT}, "timestamp": ${ts}, "env": {"TERM": "xterm-256color", "SHELL": "/bin/bash"}, "title": "himitsu demo"}
EOF
}

# Emit a single output event: [time, "o", "escaped data"]
emit() {
  local data
  data=$(json_escape "$1")
  printf '[%.4f, "o", %s]\n' "$TIME" "$data" >> "$CAST_FILE"
}

# ── Rendering helpers ────────────────────────────────────
# ANSI codes (will be baked into the cast)
C_GREEN='\033[0;32m'
C_CYAN='\033[0;36m'
C_DIM='\033[2m'
C_BOLD='\033[1m'
C_RESET='\033[0m'

# Emit a command with per-character typing animation
type_cmd() {
  local display="$1"
  local prompt
  prompt=$(printf '%b' "${C_BOLD}${C_GREEN}\$ ${C_RESET}${C_BOLD}")
  emit "$prompt"
  ts_add 0.05

  for (( i=0; i<${#display}; i++ )); do
    local ch="${display:$i:1}"
    emit "$ch"
    ts_add 0.025
  done

  local reset
  reset=$(printf '%b' "${C_RESET}")
  emit "${reset}"$'\n'
  ts_add 0.08
}

# Run himitsu with -s store, capture output, emit events
h() {
  local display_args="himitsu $*"
  type_cmd "$display_args"

  local output
  output=$("$HIMITSU_BIN" -s "$DEMO_STORE" "$@" 2>&1) || true

  if [[ -n "$output" ]]; then
    emit "$output"$'\n'
    ts_add 0.05
  fi
  emit $'\n'
  ts_add 0.3
}

# Run himitsu without -s (bare)
h_bare() {
  local display_args="himitsu $*"
  type_cmd "$display_args"

  local output
  output=$("$HIMITSU_BIN" "$@" 2>&1) || true

  if [[ -n "$output" ]]; then
    emit "$output"$'\n'
    ts_add 0.05
  fi
  emit $'\n'
  ts_add 0.3
}

# Run an arbitrary shell command, capture output
run_cmd() {
  local display="$1"
  shift
  type_cmd "$display"

  local output
  output=$(eval "$@" 2>&1) || true

  if [[ -n "$output" ]]; then
    emit "$output"$'\n'
    ts_add 0.05
  fi
  emit $'\n'
  ts_add 0.3
}

# Section banner
banner() {
  ts_add 0.4
  emit $'\n'
  local line
  line=$(printf '%b' "${C_CYAN}${C_BOLD}# ── $1 ──${C_RESET}")
  emit "$line"$'\n'
  emit $'\n'
  ts_add 0.3
}

# Dim note
note() {
  local line
  line=$(printf '%b' "${C_DIM}# $1${C_RESET}")
  emit "$line"$'\n'
  ts_add 0.2
}

# ── Record ───────────────────────────────────────────────
echo "Recording demo → $CAST_FILE"

write_header

# Clear screen
emit $'\033[2J\033[H'
ts_add 0.1

# ASCII banner
LOGO=$(printf '%b' "${C_BOLD}${C_CYAN}")
emit "${LOGO}"
emit "  _     _           _ _"$'\n'
emit " | |__ (_)_ __ ___ (_) |_ ___ _   _"$'\n'
emit " | '_ \\| | '_ \` _ \\| | __/ __| | | |"$'\n'
emit " | | | | | | | | | | | |_\\__ \\ |_| |"$'\n'
emit " |_| |_|_|_| |_| |_|_|\\__|___/\\__,_|"$'\n'
emit ""$'\n'
emit " age-based secrets management"$'\n'
LOGO_RESET=$(printf '%b' "${C_RESET}")
emit "${LOGO_RESET}"$'\n'
ts_add 1.0

# ----------------------------------------------------------
banner "1. Help"
h_bare --help

# ----------------------------------------------------------
banner "2. Initialize"
h init

note "An age keypair has been generated and the store scaffolded."
ts_add 0.2

# Grab the public key for later use (needed for recipient section)
PUBKEY=$(grep "# public key:" "$DEMO_HOME/share/key" 2>/dev/null | cut -d' ' -f4 || \
         grep "# public key:" "$DEMO_HOME/key" 2>/dev/null | cut -d' ' -f4 || \
         grep "# public key:" "$DEMO_HOME/keys/age.txt" 2>/dev/null | cut -d' ' -f4 || \
         find "$DEMO_HOME" \( -name "key" -o -name "age.txt" \) 2>/dev/null | \
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
h recipient add common/alice --age-key "$PUBKEY"
h recipient ls

# ----------------------------------------------------------
banner "9. Search"
h search DB --refresh

# ----------------------------------------------------------
banner "10. Decrypt is rejected (no plaintext at rest)"
type_cmd "himitsu decrypt"
_dec_out=$("$HIMITSU_BIN" -s "$DEMO_STORE" decrypt 2>&1) || true
if [[ -n "$_dec_out" ]]; then
  emit "$_dec_out"$'\n'
  ts_add 0.05
fi
emit $'\n'
ts_add 0.3

# ----------------------------------------------------------
banner "11. File layout"
type_cmd 'find $DEMO_HOME -type f | sort'
_file_tree=$(find "$DEMO_HOME" -type f | sed "s|$DEMO_HOME|~/.himitsu|" | sort)
emit "$_file_tree"$'\n'
ts_add 0.3

# ── Silent setup: local "upstream" store for remote/sync demo sections ────────
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
git -C "$UPSTREAM_DIR" commit -m "himitsu: add team secrets" -q 2>/dev/null || true

# ----------------------------------------------------------
banner "12. Remote add — register a team secrets repository"

note "Register a shared secrets repository as a named remote:"
ts_add 0.1
type_cmd "himitsu remote add acme/infra --url \$UPSTREAM_DIR"
_rout=$("$HIMITSU_BIN" -s "$DEMO_STORE" remote add acme/infra --url "$UPSTREAM_DIR" 2>&1) || true
if [[ -n "$_rout" ]]; then emit "$_rout"$'\n'; ts_add 0.05; fi
emit $'\n'
ts_add 0.3
note "Registered remote acme/infra"
ts_add 0.15

# ----------------------------------------------------------
banner "13. --remote (-r) — select a remote store inline"

note "Inspect any registered remote store without switching projects:"
h_bare -r acme/infra ls
h_bare -r acme/infra get prod/SHARED_API_KEY

# ----------------------------------------------------------
banner "14. Sync — mirror encrypted files into the local store"

note "Sync the registered remote into the local store:"
h sync acme/infra

note "Synced secrets are now accessible:"
h ls
h get prod/SHARED_API_KEY

# ----------------------------------------------------------
banner "15. Tags — group secrets across path hierarchies"

note "Tag the prod secrets so we can address them as a group:"
h tag prod/API_KEY add pci stripe
h tag prod/DB_PASSWORD add pci
h tag dev/DB_PASSWORD add dev-only

note "List + filter by tag (AND-semantics across multiple --tag flags):"
h ls --tag pci
h search "" --tag stripe

note "Inspect tags on a single secret:"
h tag prod/API_KEY list

# ----------------------------------------------------------
banner "16. Exec — run a command with secrets injected as env vars"

note "Inject prod/* into the child env. Var names come from set --env-key when"
note "set, otherwise derived from the path tail (api-key → API_KEY):"
ts_add 0.1
type_cmd "himitsu exec 'prod/*' -- sh -c 'echo STRIPE_API_KEY=\$STRIPE_API_KEY'"
_eout=$("$HIMITSU_BIN" -s "$DEMO_STORE" exec 'prod/*' -- sh -c 'echo "STRIPE_API_KEY=$STRIPE_API_KEY"' 2>&1) || true
if [[ -n "$_eout" ]]; then emit "$_eout"$'\n'; ts_add 0.05; fi
emit $'\n'
ts_add 0.2

note "Combine a ref with --tag for AND-filtering before injection:"
ts_add 0.1
type_cmd "himitsu exec 'prod/*' --tag stripe -- sh -c 'env | grep ^STRIPE'"
_eout=$("$HIMITSU_BIN" -s "$DEMO_STORE" exec 'prod/*' --tag stripe -- sh -c 'env | grep ^STRIPE' 2>&1) || true
if [[ -n "$_eout" ]]; then emit "$_eout"$'\n'; ts_add 0.05; fi
emit $'\n'
ts_add 0.3

# Done
emit $'\n'
DONE=$(printf '%b' "${C_BOLD}${C_GREEN}Done. All operations completed successfully.${C_RESET}")
emit "$DONE"$'\n'
emit $'\n'
ts_add 1.0

# ── Summary ──────────────────────────────────────────────
LINES=$(wc -l < "$CAST_FILE")
echo "Wrote $CAST_FILE ($LINES events, ${TIME}s runtime)"
echo "Play it:  asciinema play $CAST_FILE"
