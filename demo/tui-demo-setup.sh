#!/usr/bin/env bash
# Shared fixture builder for the TUI Phase 2 demos (US-008..US-013).
#
# Each demo (VHS tape or asciinema driver) sources this file and calls
# `tui_demo_prepare` to get an ephemeral demo root populated with a sample
# store. We point `HIMITSU_CONFIG` at `$DEMO_HOME/config.yaml`; the binary
# then derives data/state from the config's parent directory. The
# `launch_tui()` code path resolves the store from `stores_dir()` (not
# from `-s`), so the primary demo store is placed at
# `$DEMO_HOME/state/stores/demo/main`. Demos that need a second store
# for the picker also call `tui_demo_register_second_store`, which adds
# `acme/infra` and writes `config.yaml` to disambiguate.
#
# The caller is responsible for calling `tui_demo_cleanup` on exit.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HIMITSU_BIN="$PROJECT_ROOT/target/release/himitsu"

tui_demo_prepare() {
  if [[ ! -x "$HIMITSU_BIN" ]]; then
    echo "error: $HIMITSU_BIN not found — run 'cargo build --release'" >&2
    return 1
  fi

  local name="${DEMO_NAME:-tui-demo}"
  DEMO_HOME="$(mktemp -d -t "himitsu-${name}.XXXXXX")"
  export DEMO_HOME
  export HIMITSU_CONFIG="$DEMO_HOME/config.yaml"
  # Primary store lives under stores_dir so `launch_tui()` auto-resolves it.
  export DEMO_STORE="$DEMO_HOME/state/stores/demo/main"
  mkdir -p "$(dirname "$DEMO_STORE")"

  "$HIMITSU_BIN" -s "$DEMO_STORE" init --no-tui >/dev/null
  "$HIMITSU_BIN" -s "$DEMO_STORE" set prod/API_KEY        "sk_live_abc123"  --description "Stripe production API key" --env-key STRIPE_API_KEY --no-push >/dev/null
  "$HIMITSU_BIN" -s "$DEMO_STORE" set prod/DATABASE_URL   "postgres://prod" --description "Production database URL" --env-key DATABASE_URL --no-push >/dev/null
  "$HIMITSU_BIN" -s "$DEMO_STORE" set staging/API_KEY     "sk_test_xyz789"  --description "Staging API key" --no-push >/dev/null
  "$HIMITSU_BIN" -s "$DEMO_STORE" set staging/DEBUG_TOKEN "dbg_42"          --description "Temporary debug token" --no-push >/dev/null
}

# Register a second store under stores_dir and write a global config
# pointing default_store at the primary, so the ambiguous-store resolver
# still opens the right dashboard.
tui_demo_register_second_store() {
  local second="$DEMO_HOME/state/stores/acme/infra"
  mkdir -p "$(dirname "$second")"
  "$HIMITSU_BIN" -s "$second" init --no-tui >/dev/null
  "$HIMITSU_BIN" -s "$second" set prod/SHARED_KEY "team-secret-123" --no-push >/dev/null
  "$HIMITSU_BIN" -s "$second" set prod/SHARED_URL "https://api.acme.internal" --no-push >/dev/null

  cat > "$HIMITSU_CONFIG" <<YAML
default_store: demo/main
YAML
}

tui_demo_cleanup() {
  if [[ -n "${DEMO_HOME:-}" && -d "$DEMO_HOME" ]]; then
    rm -rf "$DEMO_HOME"
  fi
}
