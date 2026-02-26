#!/usr/bin/env bash
# himitsu sync — regenerate .sops.yaml, updatekeys, fetch GH collaborators, push secrets.

source "$HIMITSU_LIB/sops.sh"

cmd_sync() {
  local push_secrets=false

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --push-secrets) push_secrets=true; shift ;;
      -*)             die "unknown option: $1" ;;
      *)              shift ;;
    esac
  done

  log "syncing himitsu..."

  # 1. Fetch GitHub collaborator keys if in a git repo with a remote
  _sync_gh_collaborators

  # 2. Regenerate .sops.yaml from recipients tree
  regenerate_sops_yaml

  # 3. Run sops updatekeys on all var files
  run_updatekeys

  # 4. Optionally push decrypted vars as GitHub Actions secrets
  if [[ "$push_secrets" == true ]]; then
    _push_gh_secrets
  fi

  ok "sync complete"
}

_sync_gh_collaborators() {
  if ! command -v gh >/dev/null 2>&1; then
    warn "gh CLI not found, skipping collaborator sync"
    return 0
  fi

  local repo
  repo="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null)" || true
  if [[ -z "$repo" ]]; then
    warn "not in a GitHub repo or not authenticated, skipping collaborator sync"
    return 0
  fi

  log "fetching collaborators for $repo..."

  local collaborators
  collaborators="$(gh api "repos/$repo/collaborators" --paginate -q '.[].login' 2>/dev/null)" || {
    warn "failed to fetch collaborators (check permissions)"
    return 0
  }

  local rdir
  rdir="$(recipients_dir)"
  local team_dir="$rdir/team"
  mkdir -p "$team_dir"

  while IFS= read -r username; do
    [[ -n "$username" ]] || continue

    if ls "$team_dir/${username}".* 1>/dev/null 2>&1; then
      continue
    fi

    local ssh_keys
    ssh_keys="$(gh api "users/$username/keys" -q '.[].key' 2>/dev/null)" || continue

    local first_key
    first_key="$(echo "$ssh_keys" | head -1)"
    if [[ -n "$first_key" ]]; then
      echo "$first_key" > "$team_dir/${username}.ssh"
      ok "added collaborator '$username' (ssh key)"
    else
      warn "no public keys found for collaborator '$username'"
    fi
  done <<< "$collaborators"
}

_push_gh_secrets() {
  if ! command -v gh >/dev/null 2>&1; then
    warn "gh CLI not found, skipping secret push"
    return 0
  fi

  local vdir
  vdir="$(vars_dir)"
  local sops_path
  sops_path="$(sops_yaml_path)"
  [[ -d "$vdir" ]] || return 0

  for f in "$vdir"/*.sops.json; do
    [[ -f "$f" ]] || continue
    local env_name
    env_name="$(basename "$f" .sops.json)"

    log "pushing secrets for env '$env_name'..."

    local decrypted
    decrypted="$(sops decrypt --config "$sops_path" "$f" 2>/dev/null)" || {
      warn "failed to decrypt $f, skipping"
      continue
    }

    # Set each top-level key as a GitHub Actions secret
    local keys
    keys="$(jq -r 'keys[]' <<< "$decrypted")"
    while IFS= read -r key; do
      [[ -n "$key" ]] || continue
      local value
      value="$(jq -r --arg k "$key" '.[$k]' <<< "$decrypted")"
      local secret_name
      secret_name="$(echo "${env_name}_${key}" | tr '[:lower:]' '[:upper:]' | tr '-' '_')"
      echo "$value" | gh secret set "$secret_name" --body - 2>/dev/null && \
        ok "set secret $secret_name" || \
        warn "failed to set secret $secret_name"
    done <<< "$keys"
  done
}
