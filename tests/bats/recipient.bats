#!/usr/bin/env bats

load test_helper

setup() {
  setup_test_dir
  run_himitsu init
  run_himitsu --dir ".meta/himitsu" group add team
}

teardown() {
  teardown_test_dir
}

@test "recipient add --self creates key from local age keypair" {
  run_himitsu --dir ".meta/himitsu" recipient add --self --group team
  [ "$status" -eq 0 ]

  local username
  username="$(whoami)"
  [ -f ".meta/himitsu/recipients/team/${username}.age" ]
}

@test "recipient add with explicit age key" {
  local pubkey="age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p"
  run_himitsu --dir ".meta/himitsu" recipient add deploy-bot --age-key "$pubkey" --group team
  [ "$status" -eq 0 ]
  [ -f ".meta/himitsu/recipients/team/deploy-bot.age" ]
  grep -q "$pubkey" ".meta/himitsu/recipients/team/deploy-bot.age"
}

@test "recipient add is idempotent" {
  run_himitsu --dir ".meta/himitsu" recipient add --self --group team
  [ "$status" -eq 0 ]
  run_himitsu --dir ".meta/himitsu" recipient add --self --group team
  [ "$status" -eq 0 ]
}

@test "recipient rm removes key file" {
  run_himitsu --dir ".meta/himitsu" recipient add --self --group team
  [ "$status" -eq 0 ]

  local username
  username="$(whoami)"
  run_himitsu --dir ".meta/himitsu" recipient rm "$username" --group team
  [ "$status" -eq 0 ]
  [ ! -f ".meta/himitsu/recipients/team/${username}.age" ]
}

@test "recipient rm fails for nonexistent recipient" {
  run_himitsu --dir ".meta/himitsu" recipient rm nobody --group team
  [ "$status" -ne 0 ]
  [[ "$output" == *"not found"* ]]
}

@test "recipient ls shows recipients" {
  run_himitsu --dir ".meta/himitsu" recipient add --self --group team
  run_himitsu --dir ".meta/himitsu" recipient ls team
  [ "$status" -eq 0 ]

  local username
  username="$(whoami)"
  [[ "$output" == *"$username"* ]]
}

@test "recipient add with --label overrides name" {
  local pubkey="age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p"
  run_himitsu --dir ".meta/himitsu" recipient add --label "ci-bot" --age-key "$pubkey" --group team
  [ "$status" -eq 0 ]
  [ -f ".meta/himitsu/recipients/team/ci-bot.age" ]
}
