#!/usr/bin/env bash
# Bump the himitsu version, build, tag, and push.
#
# Usage: scripts/bump-version.sh [patch|minor|major|<semver>] [flags]
#
# Default behavior: bump → build → tag → push (full workflow).
#
# Flags:
#   --dry-run     Print what would happen, change nothing.
#   --no-push      Bump, build, and tag, but don't push.
#   --no-tag       Bump and build only, no tag or push.
#
# The optional first positional arg selects the bump type (default: patch)
# or an explicit semver (e.g. 1.2.3).
set -euo pipefail

cd "$(dirname "$0")/.."

BUMP="patch"
DRY_RUN=false
DO_TAG=true
DO_PUSH=true

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=true  ;;
    --no-push) DO_PUSH=false ;;
    --no-tag)  DO_TAG=false; DO_PUSH=false ;;
    patch|minor|major) BUMP="$arg" ;;
    *)
      if [[ "$arg" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
        BUMP="$arg"
      else
        echo "usage: $0 [patch|minor|major|<semver>] [--dry-run|--no-push|--no-tag]" >&2
        exit 1
      fi
      ;;
  esac
done
# 0. Require a clean index and worktree so the tag commit only contains
#    the version bump (Cargo.toml + Cargo.lock), not unrelated changes.
if ! git diff --quiet --cached || ! git diff --quiet; then
  echo "error: working tree has staged or unstaged changes — commit or stash first" >&2
  git status --short >&2
  exit 1
fi
# 1. Parse current version from Cargo.toml.
CURRENT=$(grep -m1 '^    version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
if [[ -z "$CURRENT" ]]; then
  echo "error: could not find version in Cargo.toml" >&2
  exit 1
fi

# 2. Compute next version.
IFS='.' read -ra PARTS <<< "$CURRENT"
MAJOR="${PARTS[0]}"
MINOR="${PARTS[1]}"
PATCH="${PARTS[2]}"

case "$BUMP" in
  major) NEXT="$((MAJOR + 1)).0.0" ;;
  minor) NEXT="$MAJOR.$((MINOR + 1)).0" ;;
  patch) NEXT="$MAJOR.$MINOR.$((PATCH + 1))" ;;
  *)
    # Explicit semver — use as-is.
    NEXT="$BUMP" ;;
esac

TAG="v$NEXT"

if $DRY_RUN; then
  echo "[dry-run] bump: $CURRENT → $NEXT ($BUMP)"
  echo "[dry-run] set version in Cargo.toml"
  echo "[dry-run] cargo build --release"
  $DO_TAG  && echo "[dry-run] git add Cargo.toml Cargo.lock && git commit -m 'chore: bump version to $NEXT' && git tag $TAG"
  $DO_PUSH && echo "[dry-run] git push origin HEAD && git push origin $TAG"
  echo "[dry-run] done: $NEXT"
  exit 0
fi

echo "bump: $CURRENT → $NEXT ($BUMP)"

# 3. Set version in Cargo.toml.
scripts/set-version.sh "$NEXT"

# 4. Build.
echo "building..."
cargo build --release

# 5. Tag.
if $DO_TAG; then
  echo "tagging $TAG"
  git add Cargo.toml Cargo.lock
  git commit -m "chore: bump version to $NEXT"
  git tag "$TAG"
fi

# 6. Push.
if $DO_PUSH; then
  echo "pushing..."
  git push origin HEAD
  if $DO_TAG; then
    git push origin "$TAG"
  fi
fi

echo "done: $NEXT"
