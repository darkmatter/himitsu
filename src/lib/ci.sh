#!/usr/bin/env bash
# himitsu ci — validate and auto-fix recipient state in CI.

source "$HIMITSU_LIB/sops.sh"

cmd_ci() {
  local auto_commit=true
  local check_only=false

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --no-commit) auto_commit=false; shift ;;
      --check)     check_only=true; shift ;;
      -*)          die "unknown option: $1" ;;
      *)           shift ;;
    esac
  done

  log "running CI checks..."

  # 1. Fetch GH collaborators if available
  if [[ "$check_only" == false ]]; then
    source "$HIMITSU_LIB/sync.sh"
    _sync_gh_collaborators
  fi

  # 2. Regenerate .sops.yaml
  regenerate_sops_yaml

  # 3. Run sops updatekeys
  local vdir
  vdir="$(vars_dir)"
  local sops_path
  sops_path="$(sops_yaml_path)"
  local needs_update=false

  if [[ -d "$vdir" ]]; then
    for f in "$vdir"/*.sops.json "$vdir"/*.sops.yaml; do
      [[ -f "$f" ]] || continue
      log "checking keys for $(basename "$f")"
      if ! sops updatekeys --config "$sops_path" -y "$f" 2>/dev/null; then
        warn "updatekeys failed for $f"
      fi
    done
  fi

  # 4. Check if anything changed
  if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if [[ -n "$(git diff --name-only "$HIMITSU_DIR" 2>/dev/null)" ]]; then
      needs_update=true
    fi
    if [[ -n "$(git ls-files --others --exclude-standard "$HIMITSU_DIR" 2>/dev/null)" ]]; then
      needs_update=true
    fi
  fi

  if [[ "$check_only" == true ]]; then
    if [[ "$needs_update" == true ]]; then
      err "CI check failed: recipient state is out of date"
      err "run 'himitsu sync' locally and commit the changes"
      exit 1
    fi
    ok "CI check passed: recipient state is up to date"
    return 0
  fi

  # 5. Auto-commit if changes detected
  if [[ "$needs_update" == true && "$auto_commit" == true ]]; then
    log "changes detected, committing..."
    git add "$HIMITSU_DIR"
    git commit -m "chore(himitsu): update recipient keys and sops config" || true
    ok "committed updated recipient state"
  elif [[ "$needs_update" == true ]]; then
    warn "changes detected but --no-commit specified"
  else
    ok "no changes needed"
  fi
}
