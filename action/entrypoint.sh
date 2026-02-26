#!/usr/bin/env bash
set -euo pipefail

ACTION_PATH="$(cd "$(dirname "$0")/.." && pwd)"

HIMITSU_BIN="$(nix build "${ACTION_PATH}#himitsu" --no-link --print-out-paths 2>/dev/null)/bin/himitsu"

if [[ ! -x "$HIMITSU_BIN" ]]; then
  echo "::error::Failed to build himitsu from flake"
  exit 1
fi

export HIMITSU_DIR="${HIMITSU_DIR:-.meta/himitsu}"

OP="${HIMITSU_OP:-ci}"

case "$OP" in
  ci)
    if [[ "${HIMITSU_AUTO_COMMIT:-true}" == "true" ]]; then
      git config user.name "himitsu[bot]"
      git config user.email "himitsu[bot]@users.noreply.github.com"
      "$HIMITSU_BIN" --dir "$HIMITSU_DIR" ci
    else
      "$HIMITSU_BIN" --dir "$HIMITSU_DIR" ci --no-commit
    fi
    ;;

  sync)
    "$HIMITSU_BIN" --dir "$HIMITSU_DIR" sync
    ;;

  add-recipient)
    NAME="${HIMITSU_RECIPIENT_NAME:-}"
    KEY="${HIMITSU_RECIPIENT_KEY:-}"
    TYPE="${HIMITSU_RECIPIENT_TYPE:-age}"
    GROUP="${HIMITSU_GROUP:-team}"

    if [[ -z "$NAME" ]]; then
      echo "::error::recipient-name is required for add-recipient"
      exit 1
    fi

    args=(--label "$NAME" --group "$GROUP" --type "$TYPE")
    if [[ -n "$KEY" ]]; then
      case "$TYPE" in
        age) args+=(--age-key "$KEY") ;;
        ssh)
          tmp_key="$(mktemp)"
          echo "$KEY" > "$tmp_key"
          args+=(--ssh-path "$tmp_key")
          ;;
        gpg) args+=(--gpg "$KEY") ;;
      esac
    fi

    "$HIMITSU_BIN" --dir "$HIMITSU_DIR" recipient add "${args[@]}"
    "$HIMITSU_BIN" --dir "$HIMITSU_DIR" sync

    if [[ "${HIMITSU_AUTO_COMMIT:-true}" == "true" ]]; then
      git config user.name "himitsu[bot]"
      git config user.email "himitsu[bot]@users.noreply.github.com"
      git add "$HIMITSU_DIR"
      git commit -m "chore(himitsu): add recipient '$NAME' to group '$GROUP'" || true
    fi
    ;;

  rm-recipient)
    NAME="${HIMITSU_RECIPIENT_NAME:-}"
    GROUP="${HIMITSU_GROUP:-team}"

    if [[ -z "$NAME" ]]; then
      echo "::error::recipient-name is required for rm-recipient"
      exit 1
    fi

    "$HIMITSU_BIN" --dir "$HIMITSU_DIR" recipient rm "$NAME" --group "$GROUP"
    "$HIMITSU_BIN" --dir "$HIMITSU_DIR" sync

    if [[ "${HIMITSU_AUTO_COMMIT:-true}" == "true" ]]; then
      git config user.name "himitsu[bot]"
      git config user.email "himitsu[bot]@users.noreply.github.com"
      git add "$HIMITSU_DIR"
      git commit -m "chore(himitsu): remove recipient '$NAME' from group '$GROUP'" || true
    fi
    ;;

  codegen)
    "$HIMITSU_BIN" --dir "$HIMITSU_DIR" codegen
    ;;

  *)
    echo "::error::Unknown operation: $OP"
    exit 1
    ;;
esac
