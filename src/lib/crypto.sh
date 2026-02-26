#!/usr/bin/env bash
# himitsu encrypt/decrypt — bulk encrypt/decrypt var files.

source "$HIMITSU_LIB/sops.sh"

cmd_encrypt() {
  local vdir
  vdir="$(vars_dir)"
  local sops_path
  sops_path="$(sops_yaml_path)"

  [[ -d "$vdir" ]] || die "vars directory not found: $vdir"

  for plain_file in "$vdir"/*.json; do
    [[ -f "$plain_file" ]] || continue
    local bn
    bn="$(basename "$plain_file")"

    # Skip already-encrypted files
    [[ "$bn" == *.sops.json ]] && continue

    local env_name="${bn%.json}"
    local sops_file="$vdir/${env_name}.sops.json"

    log "encrypting $bn -> ${env_name}.sops.json"
    local sops_err
    if sops_err="$(sops encrypt --config "$sops_path" --input-type json --output-type json "$plain_file" 2>&1 > "$sops_file")"; then
      ok "encrypted ${env_name}.sops.json"
    else
      rm -f "$sops_file"
      warn "failed to encrypt $plain_file: $sops_err"
    fi
  done
}

cmd_decrypt() {
  local vdir
  vdir="$(vars_dir)"
  local sops_path
  sops_path="$(sops_yaml_path)"

  [[ -d "$vdir" ]] || die "vars directory not found: $vdir"

  for sops_file in "$vdir"/*.sops.json "$vdir"/*.sops.yaml; do
    [[ -f "$sops_file" ]] || continue
    local bn
    bn="$(basename "$sops_file")"
    local env_name="${bn%.sops.*}"
    local plain_file="$vdir/.${env_name}.json"

    log "decrypting $bn -> .${env_name}.json"
    sops decrypt --config "$sops_path" --output-type json "$sops_file" > "$plain_file" 2>/dev/null && \
      ok "decrypted to .${env_name}.json" || \
      warn "failed to decrypt $sops_file"
  done
}

cmd_set() {
  local group="${1:-}"
  local key="${2:-}"
  local value="${3:-}"

  [[ -n "$group" && -n "$key" ]] || die "usage: himitsu set <group> <key> <value>"

  local vdir
  vdir="$(vars_dir)"
  local sops_path
  sops_path="$(sops_yaml_path)"

  local sops_file="$vdir/${group}.sops.json"

  if [[ ! -f "$sops_file" ]]; then
    log "creating new sops file: ${group}.sops.json"
    local tmp_plain
    tmp_plain="$(mktemp "$vdir/${group}.XXXXXX.json")"
    echo '{}' > "$tmp_plain"
    if sops encrypt --config "$sops_path" --input-type json --output-type json "$tmp_plain" > "$sops_file" 2>/dev/null; then
      rm -f "$tmp_plain"
    else
      rm -f "$tmp_plain" "$sops_file"
      die "failed to create encrypted file"
    fi
  fi

  # Use sops to set the value in-place
  sops set "$sops_file" "[\"$key\"]" "\"$value\"" && \
    ok "set $key in ${group}.sops.json" || \
    die "failed to set $key"
}
