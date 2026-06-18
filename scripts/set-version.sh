#!/usr/bin/env bash
# Set the crate version in Cargo.toml (flake.nix reads from Cargo.toml).
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <semver>" >&2
  exit 1
fi

VERSION="$1"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
  echo "error: invalid semver: $VERSION" >&2
  exit 1
fi

python3 <<PY
import re
from pathlib import Path

version = "${VERSION}"
cargo = Path("Cargo.toml")
text = cargo.read_text()
new_text, count = re.subn(
    r'^(    version = )"[^"]*"',
    rf'\1"{version}"',
    text,
    count=1,
    flags=re.MULTILINE,
)
if count != 1:
    raise SystemExit("failed to update version in Cargo.toml")
cargo.write_text(new_text)
print(f"set version to {version} in Cargo.toml")
PY
