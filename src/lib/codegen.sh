#!/usr/bin/env bash
# himitsu codegen — generate typed config from decrypted vars.

cmd_codegen() {
  local language="${1:-}"
  local target_path="${2:-}"

  if [[ -z "$language" ]]; then
    # Try reading codegen config from data.json apps section
    _codegen_from_config
    return
  fi

  [[ -n "$target_path" ]] || die "usage: himitsu codegen <language> <target-path>"

  _codegen_for_target "$language" "$target_path"
}

_codegen_from_config() {
  local data
  data="$(read_data_json)"

  local app_count
  app_count="$(jq '.apps // {} | length' <<< "$data")"
  if [[ "$app_count" -eq 0 ]]; then
    die "no apps configured in data.json and no language/path provided"
  fi

  local apps
  apps="$(jq -r '.apps // {} | keys[]' <<< "$data")"

  while IFS= read -r app; do
    [[ -n "$app" ]] || continue
    local lang path
    lang="$(jq -r --arg a "$app" '.apps[$a].codegen.language // empty' <<< "$data")"
    path="$(jq -r --arg a "$app" '.apps[$a].codegen.path // empty' <<< "$data")"

    if [[ -z "$lang" || -z "$path" ]]; then
      warn "app '$app' missing codegen.language or codegen.path, skipping"
      continue
    fi

    log "codegen for app '$app' ($lang -> $path)"
    _codegen_for_target "$lang" "$path" "$app"
  done <<< "$apps"
}

_codegen_for_target() {
  local language="$1"
  local target_path="$2"
  local app_filter="${3:-}"

  local vdir
  vdir="$(vars_dir)"
  local sops_path
  sops_path="$(sops_yaml_path)"

  mkdir -p "$target_path"

  # Discover environments from var files
  local envs=()
  for f in "$vdir"/*.sops.json "$vdir"/*.sops.yaml; do
    [[ -f "$f" ]] || continue
    local bn
    bn="$(basename "$f")"
    local env_name="${bn%.sops.*}"
    [[ "$env_name" != "common" ]] && envs+=("$env_name")
  done

  for env_name in "${envs[@]}"; do
    local out_file="$target_path/${env_name}.json"
    local merged="{}"

    # Merge common first
    local common_file="$vdir/common.sops.json"
    if [[ -f "$common_file" ]]; then
      local common_data
      if [[ -n "$app_filter" ]]; then
        common_data="$(sops decrypt --config "$sops_path" --extract "[\"$app_filter\"]" "$common_file" 2>/dev/null)" || \
        common_data="$(sops decrypt --config "$sops_path" "$common_file" 2>/dev/null)" || common_data="{}"
      else
        common_data="$(sops decrypt --config "$sops_path" "$common_file" 2>/dev/null)" || common_data="{}"
      fi
      merged="$(jq -s '.[0] * .[1]' <<< "$merged"$'\n'"$common_data")"
    fi

    # Merge env-specific
    local env_file
    for ext in json yaml; do
      env_file="$vdir/${env_name}.sops.${ext}"
      [[ -f "$env_file" ]] && break
    done

    if [[ -f "$env_file" ]]; then
      local env_data
      if [[ -n "$app_filter" ]]; then
        env_data="$(sops decrypt --config "$sops_path" --extract "[\"$app_filter\"]" "$env_file" 2>/dev/null)" || \
        env_data="$(sops decrypt --config "$sops_path" "$env_file" 2>/dev/null)" || env_data="{}"
      else
        env_data="$(sops decrypt --config "$sops_path" "$env_file" 2>/dev/null)" || env_data="{}"
      fi
      merged="$(jq -s '.[0] * .[1]' <<< "$merged"$'\n'"$env_data")"
    fi

    echo "$merged" | jq '.' > "$out_file"
    ok "wrote $out_file"
  done

  # Run quicktype if available and language is supported
  case "$language" in
    ts|typescript)
      _quicktype_generate "typescript" "$target_path"
      ;;
    go|golang)
      _quicktype_generate "go" "$target_path"
      ;;
    python|py)
      _quicktype_generate "python" "$target_path"
      ;;
    json)
      log "json output only, skipping type generation"
      ;;
    *)
      warn "unknown language '$language', skipping type generation (json files written)"
      ;;
  esac
}

_quicktype_generate() {
  local qt_lang="$1"
  local target_path="$2"

  if ! command -v quicktype >/dev/null 2>&1; then
    warn "quicktype not found, skipping type generation"
    return 0
  fi

  for json_file in "$target_path"/*.json; do
    [[ -f "$json_file" ]] || continue
    local bn
    bn="$(basename "$json_file" .json)"
    local ext
    case "$qt_lang" in
      typescript) ext="ts" ;;
      go) ext="go" ;;
      python) ext="py" ;;
      *) ext="txt" ;;
    esac
    local out="$target_path/${bn}.${ext}"
    quicktype -l "$qt_lang" -s json "$json_file" -o "$out" 2>/dev/null && \
      ok "generated $out" || \
      warn "quicktype failed for $json_file"
  done
}
