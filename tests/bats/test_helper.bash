#!/usr/bin/env bash
# Shared test helpers for himitsu bats tests.

TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$TESTS_DIR/../.." && pwd)"
HIMITSU_BIN="$PROJECT_ROOT/src/bin/himitsu"
export HIMITSU_LIB="$PROJECT_ROOT/src/lib"

setup_test_dir() {
  TEST_TMPDIR="$(mktemp -d)"
  export TEST_TMPDIR
  cd "$TEST_TMPDIR"

  git init -q .
  git config user.name "test"
  git config user.email "test@test.com"
}

teardown_test_dir() {
  if [[ -n "${TEST_TMPDIR:-}" && -d "$TEST_TMPDIR" ]]; then
    rm -rf "$TEST_TMPDIR"
  fi
}

run_himitsu() {
  run "$HIMITSU_BIN" "$@"
}
