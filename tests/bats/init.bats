#!/usr/bin/env bats

load test_helper

setup() {
  setup_test_dir
}

teardown() {
  teardown_test_dir
}

@test "init creates himitsu directory structure" {
  run_himitsu init
  [ "$status" -eq 0 ]
  [ -d ".meta/himitsu" ]
  [ -d ".meta/himitsu/.keys" ]
  [ -d ".meta/himitsu/vars" ]
  [ -d ".meta/himitsu/recipients" ]
  [ -f ".meta/himitsu/.himitsu.yaml" ]
  [ -f ".meta/himitsu/data.json" ]
  [ -f ".meta/himitsu/.sops.yaml" ]
}

@test "init generates age keypair" {
  run_himitsu init
  [ "$status" -eq 0 ]
  [ -f ".meta/himitsu/.keys/age.txt" ]
  grep -q "AGE-SECRET-KEY" ".meta/himitsu/.keys/age.txt"
}

@test "init creates .gitignore for .keys" {
  run_himitsu init
  [ "$status" -eq 0 ]
  [ -f ".meta/himitsu/.gitignore" ]
  grep -q ".keys/" ".meta/himitsu/.gitignore"
}

@test "init fails if directory already exists" {
  run_himitsu init
  [ "$status" -eq 0 ]

  run_himitsu init
  [ "$status" -ne 0 ]
  [[ "$output" == *"already exists"* ]]
}

@test "init with custom --dir" {
  run_himitsu --dir "custom/secrets" init
  [ "$status" -eq 0 ]
  [ -d "custom/secrets" ]
  [ -f "custom/secrets/.keys/age.txt" ]
}

@test "init data.json has correct structure" {
  run_himitsu init
  [ "$status" -eq 0 ]

  local apps groups
  apps="$(jq '.apps' .meta/himitsu/data.json)"
  groups="$(jq '.groups' .meta/himitsu/data.json)"
  [ "$apps" = "{}" ]
  [ "$groups" = "{}" ]
}
