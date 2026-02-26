#!/usr/bin/env bats

load test_helper

setup() {
  setup_test_dir
  run_himitsu init
}

teardown() {
  teardown_test_dir
}

@test "group add creates directory and updates data.json" {
  run_himitsu --dir ".meta/himitsu" group add admins
  [ "$status" -eq 0 ]
  [ -d ".meta/himitsu/recipients/admins" ]

  local has_group
  has_group="$(jq 'has("groups") and (.groups | has("admins"))' .meta/himitsu/data.json)"
  [ "$has_group" = "true" ]
}

@test "group add is idempotent" {
  run_himitsu --dir ".meta/himitsu" group add team
  [ "$status" -eq 0 ]
  run_himitsu --dir ".meta/himitsu" group add team
  [ "$status" -eq 0 ]
}

@test "group rm removes directory and data.json entry" {
  run_himitsu --dir ".meta/himitsu" group add staging
  [ "$status" -eq 0 ]
  [ -d ".meta/himitsu/recipients/staging" ]

  run_himitsu --dir ".meta/himitsu" group rm staging
  [ "$status" -eq 0 ]
  [ ! -d ".meta/himitsu/recipients/staging" ]

  local has_group
  has_group="$(jq '.groups | has("staging")' .meta/himitsu/data.json)"
  [ "$has_group" = "false" ]
}

@test "group rm refuses to delete common" {
  run_himitsu --dir ".meta/himitsu" group rm common
  [ "$status" -ne 0 ]
  [[ "$output" == *"reserved"* ]]
}

@test "group rm fails for nonexistent group" {
  run_himitsu --dir ".meta/himitsu" group rm nonexistent
  [ "$status" -ne 0 ]
  [[ "$output" == *"does not exist"* ]]
}

@test "group ls lists groups" {
  run_himitsu --dir ".meta/himitsu" group add team
  run_himitsu --dir ".meta/himitsu" group add admins
  run_himitsu --dir ".meta/himitsu" group ls
  [ "$status" -eq 0 ]
  [[ "$output" == *"team"* ]]
  [[ "$output" == *"admins"* ]]
}
