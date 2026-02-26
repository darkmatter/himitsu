#!/usr/bin/env bash
# himitsu init — scaffold a new himitsu directory.

cmd_init() {
  if [[ -d "$HIMITSU_DIR" ]]; then
    die "himitsu directory already exists: $HIMITSU_DIR"
  fi

  log "initializing himitsu in $HIMITSU_DIR"

  mkdir -p "$HIMITSU_DIR"/{.keys,vars,recipients}

  # Generate age keypair
  require_cmd age-keygen
  local key_file="$HIMITSU_DIR/.keys/age.txt"
  age-keygen -o "$key_file" 2>/dev/null
  local pubkey
  pubkey="$(grep '^# public key:' "$key_file" | sed 's/^# public key: //')"
  ok "generated age keypair (public: $pubkey)"

  # Default config
  cat > "$HIMITSU_DIR/.himitsu.yaml" <<'YAML'
keys_dir: ".keys"
vars_dir: "vars"
recipients_dir: "recipients"
YAML

  # Default data.json
  cat > "$HIMITSU_DIR/data.json" <<'JSON'
{
  "apps": {},
  "groups": {}
}
JSON

  # Seed .sops.yaml
  cat > "$HIMITSU_DIR/.sops.yaml" <<SOPS
creation_rules:
  - path_regex: 'vars/.*\.json$'
    age: "$pubkey"
SOPS

  # Ensure .keys is gitignored
  _ensure_gitignore "$HIMITSU_DIR" ".keys/"

  # Also gitignore decrypted plaintext files
  _ensure_gitignore "$HIMITSU_DIR" "vars/.*\.json"
  _ensure_gitignore "$HIMITSU_DIR" "!vars/*.sops.json"

  ok "initialized himitsu in $HIMITSU_DIR"
  log "your age key is at $key_file — keep it safe and never commit it"
}

_ensure_gitignore() {
  local dir="$1"
  local pattern="$2"
  local gitignore="$dir/.gitignore"

  if [[ -f "$gitignore" ]]; then
    if grep -qF "$pattern" "$gitignore"; then
      return
    fi
  fi

  echo "$pattern" >> "$gitignore"
}
