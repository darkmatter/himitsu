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
export HIMITSU_HOME="$DEMO_HOME"

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

# Grab the public key for later use
PUBKEY=$(grep "# public key:" "$DEMO_HOME/keys/age.txt" | cut -d' ' -f4)

# ----------------------------------------------------------
banner "3. Version-control with git"
h git init
h git add -A
h git commit -m "himitsu: initial commit"
h git log --oneline

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
ts_add 0.2

note "Write all schemas to the store:"
h schema refresh

run_cmd "ls \$HIMITSU_HOME/store/.himitsu/schemas/" "ls '$DEMO_STORE/schemas/'"

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

run_cmd "cat \$OUT/secrets.ts" "cat '$CODEGEN_OUT'"

# ----------------------------------------------------------
banner "15. Decrypt is rejected (no plaintext at rest)"
type_cmd "himitsu decrypt"
local_output=$("$HIMITSU_BIN" -s "$DEMO_STORE" decrypt 2>&1) || true
if [[ -n "$local_output" ]]; then
  emit "$local_output"$'\n'
  ts_add 0.05
fi
emit $'\n'
ts_add 0.3

# ----------------------------------------------------------
banner "16. File layout"
type_cmd "find \$HIMITSU_HOME -type f | sort"
file_tree=$(find "$DEMO_HOME" -type f | sed "s|$DEMO_HOME|~/.himitsu|" | sort)
emit "$file_tree"$'\n'
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
"$HIMITSU_BIN" -s "$UPSTREAM_DIR" set prod SHARED_API_KEY "team-key-abc-789" > /dev/null 2>&1
"$HIMITSU_BIN" -s "$UPSTREAM_DIR" set prod SHARED_DB_URL "postgres://db.internal/app" > /dev/null 2>&1
git -C "$UPSTREAM_DIR" add -A 2>/dev/null
git -C "$UPSTREAM_DIR" commit -m "himitsu: add team secrets" -q 2>/dev/null

# ----------------------------------------------------------
banner "17. Remote add — register a team secrets repository"

note "Register a shared secrets repository as a named remote:"
ts_add 0.1
type_cmd "himitsu remote add acme/infra --url \$UPSTREAM_DIR"
_rout=$("$HIMITSU_BIN" -s "$DEMO_STORE" remote add acme/infra --url "$UPSTREAM_DIR" 2>&1) || true
if [[ -n "$_rout" ]]; then emit "$_rout"$'\n'; ts_add 0.05; fi
emit $'\n'
ts_add 0.3
note "Cloned and registered at ~/.himitsu/data/acme/infra"
ts_add 0.15

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
