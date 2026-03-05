#!/usr/bin/env bash
# himitsu backend — manage remote git-backed storage for encrypted secrets.

cmd_backend() {
  local subcmd="${1:-}"
  shift || true

  case "$subcmd" in
    create) _backend_create "$@" ;;
    push)   _backend_push "$@" ;;
    pull)   _backend_pull "$@" ;;
    status) _backend_status "$@" ;;
    "")     _backend_usage; exit 1 ;;
    *)      die "unknown backend command: $subcmd" ;;
  esac
}

_backend_usage() {
  cat <<EOF
Usage: himitsu backend <command>

Commands:
  create <provider>    Create a remote backend (providers: github)
  push                 Push encrypted secrets to the remote backend
  pull                 Pull latest secrets from the remote backend
  status               Show backend status

Create options:
  --name <repo>        Repository name (default: himitsu-secrets)
  --org <org>          GitHub organization (omit for personal account)
  --description <d>    Repository description
EOF
}

_backend_create() {
  local provider="${1:-}"
  shift || true

  case "$provider" in
    github) _backend_create_github "$@" ;;
    "")     die "usage: himitsu backend create <provider> (supported: github)" ;;
    *)      die "unsupported backend provider: $provider (supported: github)" ;;
  esac
}

_backend_create_github() {
  require_cmd gh
  require_cmd git

  gh auth status >/dev/null 2>&1 || die "gh CLI not authenticated — run 'gh auth login' first"

  local repo_name="himitsu-secrets"
  local org=""
  local description="Encrypted secrets managed by himitsu"

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --name)        repo_name="$2"; shift 2 ;;
      --org)         org="$2"; shift 2 ;;
      --description) description="$2"; shift 2 ;;
      *)             die "unknown option: $1" ;;
    esac
  done

  if _backend_is_configured; then
    local existing
    existing="$(yq -r '.backend.repo' "$HIMITSU_DIR/$HIMITSU_CONFIG_FILE")"
    die "backend already configured ($existing). Remove backend config from .himitsu.yaml first."
  fi

  local full_name="$repo_name"
  [[ -n "$org" ]] && full_name="$org/$repo_name"

  log "creating private GitHub repository '$full_name'..."

  if ! gh repo create "$full_name" --private --description "$description" >/dev/null 2>&1; then
    if gh repo view "$full_name" >/dev/null 2>&1; then
      warn "repository '$full_name' already exists — using it"
    else
      die "failed to create repository '$full_name'"
    fi
  else
    ok "created repository '$full_name'"
  fi

  local clone_url
  clone_url="$(gh repo view "$full_name" --json sshUrl -q '.sshUrl')" || \
    die "failed to get repository URL"

  # Initialize git in himitsu directory if needed
  if [[ ! -d "$HIMITSU_DIR/.git" ]]; then
    git -C "$HIMITSU_DIR" init -b main >/dev/null 2>&1
    log "initialized git repository"
  fi

  _backend_setup_gitignore

  # Configure remote
  if git -C "$HIMITSU_DIR" remote get-url origin >/dev/null 2>&1; then
    git -C "$HIMITSU_DIR" remote set-url origin "$clone_url"
  else
    git -C "$HIMITSU_DIR" remote add origin "$clone_url"
  fi
  log "remote set to $clone_url"

  _backend_save_config "github" "$full_name" "$clone_url"

  # Initial commit and push
  git -C "$HIMITSU_DIR" add -A
  if ! git -C "$HIMITSU_DIR" diff --cached --quiet 2>/dev/null; then
    git -C "$HIMITSU_DIR" commit -m "initialize himitsu secrets store" >/dev/null 2>&1
    ok "created initial commit"
  fi

  git -C "$HIMITSU_DIR" push -u origin main 2>/dev/null || \
    warn "push failed — run 'himitsu backend push' after resolving"

  ok "github backend configured: $full_name"
}

# ---------------------------------------------------------------------------
# push / pull / status
# ---------------------------------------------------------------------------

_backend_push() {
  _backend_require_configured

  local message
  message="update secrets $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -m|--message) message="$2"; shift 2 ;;
      *)            die "unknown option: $1" ;;
    esac
  done

  log "pushing to backend..."

  git -C "$HIMITSU_DIR" add -A

  if git -C "$HIMITSU_DIR" diff --cached --quiet 2>/dev/null; then
    log "nothing to push — working tree clean"
    return 0
  fi

  git -C "$HIMITSU_DIR" commit -m "$message" >/dev/null 2>&1
  git -C "$HIMITSU_DIR" push >/dev/null 2>&1 || die "failed to push to backend"

  ok "pushed to backend"
}

_backend_pull() {
  _backend_require_configured

  log "pulling from backend..."
  git -C "$HIMITSU_DIR" pull --rebase >/dev/null 2>&1 || die "failed to pull from backend"

  ok "pulled latest from backend"
}

_backend_status() {
  if ! _backend_is_configured; then
    log "no backend configured"
    log "run 'himitsu backend create github' to set up a backend"
    return 0
  fi

  local config_path="$HIMITSU_DIR/$HIMITSU_CONFIG_FILE"
  local btype brepo burl
  btype="$(yq -r '.backend.type' "$config_path")"
  brepo="$(yq -r '.backend.repo' "$config_path")"
  burl="$(yq -r '.backend.url' "$config_path")"

  log "backend"
  echo "  type:  $btype"
  echo "  repo:  $brepo"
  echo "  url:   $burl"
  echo ""

  if [[ -d "$HIMITSU_DIR/.git" ]]; then
    local dirty
    dirty="$(git -C "$HIMITSU_DIR" status --short 2>/dev/null)"
    if [[ -z "$dirty" ]]; then
      echo "  status: clean"
    else
      echo "  status: uncommitted changes"
      while IFS= read -r line; do
        echo "    $line"
      done <<< "$dirty"
    fi
  fi
}

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

_backend_is_configured() {
  local config_path="$HIMITSU_DIR/$HIMITSU_CONFIG_FILE"
  [[ -f "$config_path" ]] && [[ "$(yq -r '.backend.type // ""' "$config_path")" != "" ]]
}

_backend_require_configured() {
  _backend_is_configured || die "no backend configured — run 'himitsu backend create github' first"
  [[ -d "$HIMITSU_DIR/.git" ]] || die "git not initialized in himitsu directory"
}

_backend_setup_gitignore() {
  local gitignore="$HIMITSU_DIR/.gitignore"

  local patterns=(
    ".keys/"
    "vars/.*.json"
  )

  for p in "${patterns[@]}"; do
    if [[ -f "$gitignore" ]] && grep -qF "$p" "$gitignore" 2>/dev/null; then
      continue
    fi
    echo "$p" >> "$gitignore"
  done
}

_backend_save_config() {
  local type="$1" repo="$2" url="$3"
  local config_path="$HIMITSU_DIR/$HIMITSU_CONFIG_FILE"

  if [[ -f "$config_path" ]]; then
    yq -i ".backend.type = \"$type\" | .backend.repo = \"$repo\" | .backend.url = \"$url\"" "$config_path"
  else
    cat > "$config_path" <<YAML
keys_dir: ".keys"
vars_dir: "vars"
recipients_dir: "recipients"
backend:
  type: $type
  repo: $repo
  url: $url
YAML
  fi
}
