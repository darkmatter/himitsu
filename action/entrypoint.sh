#!/usr/bin/env bash
set -euo pipefail

ACTION_PATH="$(cd "$(dirname "$0")/.." && pwd)"

HIMITSU_BIN="$(nix build "${ACTION_PATH}#himitsu" --no-link --print-out-paths 2>/dev/null)/bin/himitsu"

if [[ ! -x "$HIMITSU_BIN" ]]; then
	echo "::error::Failed to build himitsu from flake"
	exit 1
fi

OP="${HIMITSU_OP:-sync}"
REMOTE_ARGS=()
if [[ -n "${HIMITSU_REMOTE:-}" ]]; then
	REMOTE_ARGS=(-r "$HIMITSU_REMOTE")
fi

case "$OP" in
sync)
	"$HIMITSU_BIN" "${REMOTE_ARGS[@]}" sync
	;;

add-recipient)
	NAME="${HIMITSU_RECIPIENT_NAME:-}"
	KEY="${HIMITSU_RECIPIENT_KEY:-}"
	GROUP="${HIMITSU_GROUP:-team}"

	if [[ -z "$NAME" ]]; then
		echo "::error::recipient-name is required for add-recipient"
		exit 1
	fi

	args=("${REMOTE_ARGS[@]}" recipient add "$NAME" --group "$GROUP")
	if [[ -n "$KEY" ]]; then
		args+=(--age-key "$KEY")
	else
		args+=(--self)
	fi

	"$HIMITSU_BIN" "${args[@]}"
	"$HIMITSU_BIN" "${REMOTE_ARGS[@]}" encrypt

	if [[ "${HIMITSU_AUTO_COMMIT:-true}" == "true" ]]; then
		"$HIMITSU_BIN" "${REMOTE_ARGS[@]}" remote push
	fi
	;;

rm-recipient)
	NAME="${HIMITSU_RECIPIENT_NAME:-}"
	GROUP="${HIMITSU_GROUP:-team}"

	if [[ -z "$NAME" ]]; then
		echo "::error::recipient-name is required for rm-recipient"
		exit 1
	fi

	"$HIMITSU_BIN" "${REMOTE_ARGS[@]}" recipient rm "$NAME" --group "$GROUP"
	"$HIMITSU_BIN" "${REMOTE_ARGS[@]}" encrypt

	if [[ "${HIMITSU_AUTO_COMMIT:-true}" == "true" ]]; then
		"$HIMITSU_BIN" "${REMOTE_ARGS[@]}" remote push
	fi
	;;

*)
	echo "::error::Unknown operation: $OP"
	exit 1
	;;
esac
