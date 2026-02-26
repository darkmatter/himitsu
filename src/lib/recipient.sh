#!/usr/bin/env bash
# himitsu recipient — manage recipients within groups.

cmd_recipient() {
  local action="${1:-}"
  shift || true

  case "$action" in
    add) _recipient_add "$@" ;;
    rm)  _recipient_rm "$@" ;;
    ls)  _recipient_ls "$@" ;;
    *)   die "usage: himitsu recipient <add|rm|ls> [options]" ;;
  esac
}

_recipient_add() {
  local label=""
  local key_type="age"
  local group="team"
  local description=""
  local ssh_path=""
  local gpg_id=""
  local age_key=""
  local self_mode=false

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --self)       self_mode=true; shift ;;
      --label)      label="$2"; shift 2 ;;
      --type)       key_type="$2"; shift 2 ;;
      --group)      group="$2"; shift 2 ;;
      --description) description="$2"; shift 2 ;;
      --ssh-path)   ssh_path="$2"; key_type="ssh"; shift 2 ;;
      --gpg)        gpg_id="$2"; key_type="gpg"; shift 2 ;;
      --age-key)    age_key="$2"; key_type="age"; shift 2 ;;
      -*)           die "unknown option: $1" ;;
      *)
        if [[ -z "$label" ]]; then
          label="$1"
        fi
        shift
        ;;
    esac
  done

  # Default to --self if no explicit key source given
  if [[ -z "$age_key" && -z "$ssh_path" && -z "$gpg_id" ]]; then
    self_mode=true
  fi

  if [[ -z "$label" && "$self_mode" == true ]]; then
    label="$(whoami)"
  fi
  [[ -n "$label" ]] || die "recipient label required (positional arg or --label)"

  local rdir
  rdir="$(recipients_dir)"
  local group_path="$rdir/$group"
  mkdir -p "$group_path"

  local key_file="$group_path/${label}.${key_type}"

  if [[ -f "$key_file" ]]; then
    warn "recipient '$label' already exists in group '$group'"
    return 0
  fi

  case "$key_type" in
    age)
      if [[ "$self_mode" == true ]]; then
        local kdir
        kdir="$(keys_dir)"
        local local_key="$kdir/age.txt"
        if [[ ! -f "$local_key" ]]; then
          require_cmd age-keygen
          mkdir -p "$kdir"
          age-keygen -o "$local_key" 2>/dev/null
          log "generated new age keypair"
        fi
        age_key="$(grep '^# public key:' "$local_key" | sed 's/^# public key: //')"
      fi
      [[ -n "$age_key" ]] || die "no age public key provided (use --age-key or --self)"
      {
        [[ -n "$description" ]] && echo "# $description"
        echo "$age_key"
      } > "$key_file"
      ;;
    ssh)
      [[ -n "$ssh_path" ]] || die "no SSH key path provided (use --ssh-path)"
      [[ -f "$ssh_path" ]] || die "SSH key not found: $ssh_path"
      cp "$ssh_path" "$key_file"
      ;;
    gpg)
      [[ -n "$gpg_id" ]] || die "no GPG key ID provided (use --gpg)"
      echo "$gpg_id" > "$key_file"
      ;;
    *)
      die "unsupported key type: $key_type (expected age, ssh, or gpg)"
      ;;
  esac

  ok "added recipient '$label' to group '$group' (type: $key_type)"

  source "$HIMITSU_LIB/sops.sh"
  regenerate_sops_yaml
  log "run 'himitsu sync' to re-encrypt files for updated recipients"
}

_recipient_rm() {
  local label=""
  local group="team"

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --group) group="$2"; shift 2 ;;
      -*)      die "unknown option: $1" ;;
      *)       label="$1"; shift ;;
    esac
  done

  [[ -n "$label" ]] || die "usage: himitsu recipient rm <name> [--group <group>]"

  local rdir
  rdir="$(recipients_dir)"
  local group_path="$rdir/$group"

  local found=false
  for key_file in "$group_path/${label}".*; do
    if [[ -f "$key_file" ]]; then
      rm "$key_file"
      found=true
    fi
  done

  if [[ "$found" == false ]]; then
    die "recipient '$label' not found in group '$group'"
  fi

  ok "removed recipient '$label' from group '$group'"

  source "$HIMITSU_LIB/sops.sh"
  regenerate_sops_yaml
  log "run 'himitsu sync' to re-encrypt files for updated recipients"
}

_recipient_ls() {
  local group="${1:-}"
  local rdir
  rdir="$(recipients_dir)"

  if [[ -n "$group" ]]; then
    local group_path="$rdir/$group"
    [[ -d "$group_path" ]] || die "group '$group' not found"
    _list_group_recipients "$group" "$group_path"
  else
    for entry in "$rdir"/*; do
      [[ -e "$entry" ]] || continue
      local name
      name="$(basename "$entry")"
      if [[ -d "$entry" ]]; then
        _list_group_recipients "$name" "$entry"
      elif [[ -f "$entry" ]]; then
        echo "  $name (standalone)"
      fi
    done
  fi
}

_list_group_recipients() {
  local group_name="$1"
  local group_path="$2"
  echo "$group_name:"
  for key_file in "$group_path"/*; do
    [[ -f "$key_file" ]] || continue
    local basename
    basename="$(basename "$key_file")"
    local name="${basename%.*}"
    local ext="${basename##*.}"
    echo "  $name ($ext)"
  done
}
