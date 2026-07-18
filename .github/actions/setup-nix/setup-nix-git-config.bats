load '../../workflows/test_helper/common'

setup() {
  _common_setup
}

@test "adds insteadOf entries for all SSH URL formats" {
  HOME="$BATS_TEST_TMPDIR" run "${BATS_TEST_DIRNAME}/setup-nix-git-config.sh"
  echo "status=$status output=$output" >&2
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"git+ssh://git@github.com/"* ]]
}

@test "entries are idempotent when run twice" {
  HOME="$BATS_TEST_TMPDIR" run "${BATS_TEST_DIRNAME}/setup-nix-git-config.sh"
  [[ "$status" -eq 0 ]]
  HOME="$BATS_TEST_TMPDIR" run "${BATS_TEST_DIRNAME}/setup-nix-git-config.sh"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"git+ssh://git@github.com/"* ]]
}
