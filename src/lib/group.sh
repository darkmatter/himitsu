#!/usr/bin/env bash
# himitsu group — manage recipient groups.

cmd_group() {
  local action="${1:-}"
  shift || true

  case "$action" in
    add) _group_add "$@" ;;
    rm)  _group_rm "$@" ;;
    ls)  _group_ls "$@" ;;
    *)   die "usage: himitsu group <add|rm|ls> [name]" ;;
  esac
}

_group_add() {
  local name="${1:-}"
  [[ -n "$name" ]] || die "usage: himitsu group add <name>"

  local rdir
  rdir="$(recipients_dir)"
  local group_path="$rdir/$name"

  if [[ -d "$group_path" ]]; then
    warn "group '$name' already exists"
    return 0
  fi

  mkdir -p "$group_path"

  local data
  data="$(read_data_json)"
  data="$(jq --arg name "$name" '
    .groups[$name] //= {"groups": []}
  ' <<< "$data")"
  write_data_json "$data"

  ok "created group '$name'"
}

_group_rm() {
  local name="${1:-}"
  [[ -n "$name" ]] || die "usage: himitsu group rm <name>"

  if [[ "$name" == "common" ]]; then
    die "'common' is a reserved group and cannot be removed"
  fi

  local rdir
  rdir="$(recipients_dir)"
  local group_path="$rdir/$name"

  if [[ ! -d "$group_path" ]] && [[ ! -f "$group_path" ]]; then
    die "group '$name' does not exist"
  fi

  rm -rf "$group_path"

  local data
  data="$(read_data_json)"
  data="$(jq --arg name "$name" 'del(.groups[$name])' <<< "$data")"
  write_data_json "$data"

  ok "removed group '$name'"
  log "run 'himitsu sync' to update encrypted files"
}

_group_ls() {
  local rdir
  rdir="$(recipients_dir)"
  [[ -d "$rdir" ]] || die "no recipients directory found"

  for entry in "$rdir"/*; do
    [[ -e "$entry" ]] || continue
    local name
    name="$(basename "$entry")"
    local count=0
    if [[ -d "$entry" ]]; then
      count=$(find "$entry" -type f | wc -l | tr -d ' ')
    elif [[ -f "$entry" ]]; then
      count=1
    fi
    echo "$name ($count recipients)"
  done
}
