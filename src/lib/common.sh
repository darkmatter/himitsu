#!/usr/bin/env bash
# Common utilities shared across all himitsu subcommands.

HIMITSU_DEFAULT_DIR=".meta/himitsu"
HIMITSU_CONFIG_FILE=".himitsu.yaml"
HIMITSU_DATA_FILE="data.json"

RED='\033[0;31m'
YELLOW='\033[0;33m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

log()  { echo -e "${BLUE}[himitsu]${NC} $*"; }
warn() { echo -e "${YELLOW}[himitsu]${NC} $*" >&2; }
err()  { echo -e "${RED}[himitsu]${NC} $*" >&2; }
ok()   { echo -e "${GREEN}[himitsu]${NC} $*"; }

die() {
  err "$@"
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

find_himitsu_root() {
  local dir="${HIMITSU_DIR:-}"
  if [[ -n "$dir" ]]; then
    if [[ -d "$dir" ]]; then
      echo "$dir"
      return
    fi
    die "HIMITSU_DIR does not exist: $dir"
  fi

  local search_dir
  search_dir="$(pwd)"
  while [[ "$search_dir" != "/" ]]; do
    if [[ -d "$search_dir/$HIMITSU_DEFAULT_DIR" ]]; then
      echo "$search_dir/$HIMITSU_DEFAULT_DIR"
      return
    fi
    search_dir="$(dirname "$search_dir")"
  done

  die "could not find himitsu directory (looked for $HIMITSU_DEFAULT_DIR). Run 'himitsu init' first."
}

load_config() {
  local config_path="$HIMITSU_DIR/$HIMITSU_CONFIG_FILE"
  if [[ -f "$config_path" ]]; then
    HIMITSU_KEYS_DIR="$(yq -r '.keys_dir // ".keys"' "$config_path")"
    HIMITSU_VARS_DIR="$(yq -r '.vars_dir // "vars"' "$config_path")"
    HIMITSU_RECIPIENTS_DIR="$(yq -r '.recipients_dir // "recipients"' "$config_path")"
  else
    HIMITSU_KEYS_DIR=".keys"
    HIMITSU_VARS_DIR="vars"
    HIMITSU_RECIPIENTS_DIR="recipients"
  fi

  export HIMITSU_KEYS_DIR HIMITSU_VARS_DIR HIMITSU_RECIPIENTS_DIR
  export SOPS_AGE_KEY_FILE="${SOPS_AGE_KEY_FILE:-$HIMITSU_DIR/$HIMITSU_KEYS_DIR/age.txt}"
}

_abs() {
  local p="$1"
  if [[ "$p" = /* ]]; then
    echo "$p"
  else
    echo "$(pwd)/$p"
  fi
}

data_json_path() {
  _abs "$HIMITSU_DIR/$HIMITSU_DATA_FILE"
}

sops_yaml_path() {
  _abs "$HIMITSU_DIR/.sops.yaml"
}

vars_dir() {
  _abs "$HIMITSU_DIR/$HIMITSU_VARS_DIR"
}

recipients_dir() {
  _abs "$HIMITSU_DIR/$HIMITSU_RECIPIENTS_DIR"
}

keys_dir() {
  _abs "$HIMITSU_DIR/$HIMITSU_KEYS_DIR"
}

read_data_json() {
  local path
  path="$(data_json_path)"
  if [[ ! -f "$path" ]]; then
    echo '{}'
    return
  fi
  cat "$path"
}

write_data_json() {
  local path
  path="$(data_json_path)"
  jq '.' <<< "$1" > "$path"
}

collect_recipients_for_group() {
  local group="$1"
  local rdir
  rdir="$(recipients_dir)"

  local group_path="$rdir/$group"
  if [[ ! -d "$group_path" ]] && [[ ! -f "$group_path" ]]; then
    return
  fi

  if [[ -f "$group_path" ]]; then
    cat "$group_path"
    return
  fi

  for key_file in "$group_path"/*; do
    [[ -f "$key_file" ]] || continue
    local ext="${key_file##*.}"
    case "$ext" in
      age)
        grep -v '^#' "$key_file" | grep -v '^$' | head -1
        ;;
      ssh)
        cat "$key_file"
        ;;
      gpg)
        cat "$key_file"
        ;;
    esac
  done
}

collect_all_recipients() {
  local rdir
  rdir="$(recipients_dir)"
  [[ -d "$rdir" ]] || return

  for entry in "$rdir"/*; do
    local name
    name="$(basename "$entry")"
    if [[ -d "$entry" ]]; then
      collect_recipients_for_group "$name"
    elif [[ -f "$entry" ]]; then
      local ext="${entry##*.}"
      case "$ext" in
        age) grep -v '^#' "$entry" | grep -v '^$' | head -1 ;;
        ssh|gpg) cat "$entry" ;;
      esac
    fi
  done
}

groups_for_env() {
  local env_name="$1"
  local data
  data="$(read_data_json)"
  jq -r --arg env "$env_name" '
    .groups // {} | to_entries[]
    | select(.value.groups // [] | index($env))
    | .key
  ' <<< "$data"
}
