#!/usr/bin/env bash
set -euo pipefail

# ── Helpers ───────────────────────────────────────────────
HIMITSU_BIN="$(cd "$(dirname "$0")/.." && pwd)/target/release/himitsu"
DEMO_HOME="$(mktemp -d)"
export HIMITSU_HOME="$DEMO_HOME"

GREEN='\033[0;32m'
CYAN='\033[0;36m'
DIM='\033[2m'
BOLD='\033[1m'
RESET='\033[0m'

type_cmd() {
	printf "${BOLD}${GREEN}$ ${RESET}${BOLD}"
	local display="$*"
	for ((i = 0; i < ${#display}; i++)); do
		printf '%s' "${display:$i:1}"
		sleep 0.02
	done
	printf "${RESET}\n"
	sleep 0.1
}

# Display 'himitsu ...' but run the actual binary
h() {
	type_cmd "himitsu $*"
	"$HIMITSU_BIN" "$@"
	echo
	sleep 0.4
}

run() {
	type_cmd "$@"
	eval "$@"
	echo
	sleep 0.4
}

banner() {
	echo
	printf "${CYAN}${BOLD}# ── %s ──${RESET}\n" "$1"
	echo
	sleep 0.3
}

cleanup() { rm -rf "$DEMO_HOME"; }
trap cleanup EXIT

# ── Demo ──────────────────────────────────────────────────

clear
printf "${BOLD}${CYAN}"
cat <<'ART'
  _     _           _ _
 | |__ (_)_ __ ___ (_) |_ ___ _   _
 | '_ \| | '_ ` _ \| | __/ __| | | |
 | | | | | | | | | | | |_\__ \ |_| |
 |_| |_|_|_| |_| |_|_|\__|___/\__,_|

 age-based secrets management
ART
printf "${RESET}\n"
sleep 0.8

banner "1. Help"
h --help

banner "2. Initialize"
h init

banner "3. Create a local remote"
printf "${DIM}# In practice: himitsu remote add org/repo${RESET}\n"
printf "${DIM}# For this demo we create the structure locally.${RESET}\n\n"
sleep 0.3
REMOTE_DIR="$DEMO_HOME/data/demo/secrets"
mkdir -p "$REMOTE_DIR/vars" "$REMOTE_DIR/recipients/common"
echo '{"groups":["common"]}' >"$REMOTE_DIR/data.json"
PUBKEY=$(grep "# public key:" "$DEMO_HOME/keys/age.txt" | cut -d' ' -f4)
echo "$PUBKEY" >"$REMOTE_DIR/recipients/common/self.pub"
printf "${DIM}# Created remote demo/secrets with self as recipient${RESET}\n\n"
sleep 0.5

banner "4. Set secrets"
h -r demo/secrets set prod API_KEY "sk_live_abc123"
h -r demo/secrets set prod DB_PASSWORD "hunter2"
h -r demo/secrets set dev DB_PASSWORD "devpass"
h -r demo/secrets set common API_BASE_URL "https://api.example.com"

banner "5. Get secrets back"
h -r demo/secrets get prod API_KEY
h -r demo/secrets get prod DB_PASSWORD

banner "6. List environments"
h -r demo/secrets ls

banner "7. List keys in an environment"
h -r demo/secrets ls prod

banner "8. Re-encrypt for current recipients"
h -r demo/secrets encrypt

printf "${DIM}# Verify secrets survive re-encryption:${RESET}\n"
h -r demo/secrets get prod API_KEY

banner "9. Recipient management"
h -r demo/secrets recipient ls
h -r demo/secrets recipient add alice --age-key "$PUBKEY" --group common
h -r demo/secrets recipient ls

banner "10. Group management"
h -r demo/secrets group add admins
h -r demo/secrets group ls

banner "11. Search across remotes"
h search DB --refresh
h search API --refresh

banner "12. Decrypt is rejected (no plaintext at rest)"
type_cmd "himitsu decrypt"
"$HIMITSU_BIN" decrypt 2>&1 || true
echo
sleep 0.4

banner "13. File layout"
type_cmd "find \$HIMITSU_HOME -type f | sort"
find "$DEMO_HOME" -type f | sed "s|$DEMO_HOME|\~/.himitsu|" | sort
echo

printf "\n${BOLD}${GREEN}Done. All operations completed successfully.${RESET}\n\n"
sleep 1.5
