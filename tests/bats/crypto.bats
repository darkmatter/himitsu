#!/usr/bin/env bats

load test_helper

setup() {
  setup_test_dir
  run_himitsu init
  export HIMITSU_DIR=".meta/himitsu"
  export SOPS_AGE_KEY_FILE="$TEST_TMPDIR/.meta/himitsu/.keys/age.txt"
}

teardown() {
  teardown_test_dir
}

@test "encrypt creates sops file from plaintext" {
  echo '{"DB_HOST": "localhost", "DB_PORT": "5432"}' > .meta/himitsu/vars/dev.json

  run_himitsu --dir ".meta/himitsu" encrypt
  [ "$status" -eq 0 ]
  [ -f ".meta/himitsu/vars/dev.sops.json" ]

  # Verify it's actually encrypted (contains sops metadata)
  grep -q "sops" ".meta/himitsu/vars/dev.sops.json"
}

@test "decrypt creates plaintext from sops file" {
  echo '{"API_KEY": "secret123"}' > .meta/himitsu/vars/staging.json
  run_himitsu --dir ".meta/himitsu" encrypt
  [ "$status" -eq 0 ]

  rm .meta/himitsu/vars/staging.json

  run_himitsu --dir ".meta/himitsu" decrypt
  [ "$status" -eq 0 ]
  [ -f ".meta/himitsu/vars/.staging.json" ]

  local value
  value="$(jq -r '.API_KEY' .meta/himitsu/vars/.staging.json)"
  [ "$value" = "secret123" ]
}

@test "version flag works" {
  run_himitsu --version
  [ "$status" -eq 0 ]
  [[ "$output" == *"0.1.0"* ]]
}

@test "help flag works" {
  run_himitsu --help
  [ "$status" -eq 0 ]
  [[ "$output" == *"Usage"* ]]
}
