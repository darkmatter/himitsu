#!/usr/bin/env bash
# himitsu sops — generate .sops.yaml from recipients/ tree and data.json mappings.

regenerate_sops_yaml() {
  local sops_path
  sops_path="$(sops_yaml_path)"
  local data
  data="$(read_data_json)"

  local var_files=()
  local vdir
  vdir="$(vars_dir)"
  if [[ -d "$vdir" ]]; then
    for f in "$vdir"/*.sops.json "$vdir"/*.sops.yaml; do
      [[ -f "$f" ]] && var_files+=("$(basename "$f")")
    done
  fi

  # Collect all unique environment names from var files
  local envs=()
  for vf in "${var_files[@]}"; do
    local env_name="${vf%.sops.*}"
    envs+=("$env_name")
  done

  # Start building .sops.yaml
  local yaml="creation_rules:"

  # Per-environment rules: match groups that map to this env
  for env_name in "${envs[@]}"; do
    local age_keys=()

    # Find groups that include this env
    local mapped_groups
    mapped_groups="$(jq -r --arg env "$env_name" '
      .groups // {} | to_entries[]
      | select(.value.groups // [] | index($env))
      | .key
    ' <<< "$data")"

    while IFS= read -r grp; do
      [[ -n "$grp" ]] || continue
      while IFS= read -r key; do
        [[ -n "$key" ]] && age_keys+=("$key")
      done <<< "$(collect_recipients_for_group "$grp")"
    done <<< "$mapped_groups"

    # Always include all recipients for common.sops.json
    if [[ "$env_name" == "common" ]]; then
      while IFS= read -r key; do
        [[ -n "$key" ]] && age_keys+=("$key")
      done <<< "$(collect_all_recipients)"
    fi

    # Deduplicate
    local unique_keys
    unique_keys="$(printf '%s\n' "${age_keys[@]}" | sort -u | paste -sd ',' -)"

    if [[ -n "$unique_keys" ]]; then
      yaml+="
  - path_regex: 'vars/${env_name}\\..*\\.(json|yaml)$'
    age: \"${unique_keys}\""
    fi
  done

  # Catch-all rule using all recipients
  local all_keys=()
  while IFS= read -r key; do
    [[ -n "$key" ]] && all_keys+=("$key")
  done <<< "$(collect_all_recipients)"

  local all_unique
  all_unique="$(printf '%s\n' "${all_keys[@]}" | sort -u | paste -sd ',' -)"

  if [[ -n "$all_unique" ]]; then
    yaml+="
  - path_regex: 'vars/.*\\.(json|yaml)$'
    age: \"${all_unique}\""
  fi

  echo "$yaml" > "$sops_path"
  ok "regenerated $(sops_yaml_path)"
}

run_updatekeys() {
  local vdir
  vdir="$(vars_dir)"
  local sops_path
  sops_path="$(sops_yaml_path)"

  [[ -d "$vdir" ]] || return 0

  for f in "$vdir"/*.sops.json "$vdir"/*.sops.yaml; do
    [[ -f "$f" ]] || continue
    log "updating keys for $(basename "$f")"
    sops updatekeys --config "$sops_path" -y "$f" || warn "failed to update keys for $f"
  done
}
