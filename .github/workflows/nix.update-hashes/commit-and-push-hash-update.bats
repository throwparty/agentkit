load '../test_helper/common'

setup() {
  _common_setup
}

@test "commits when nix/flake.nix has changes" {
  cd "$BATS_TEST_TMPDIR"
  git init
  git config user.email test@test
  git config user.name test
  mkdir -p nix
  echo 'old' > nix/flake.nix
  git add .
  git commit -m "init"

  echo 'new' > nix/flake.nix

  run env GITHUB_TOKEN=token GITHUB_REPOSITORY=user/repo "${BATS_TEST_DIRNAME}/commit-and-push-hash-update.sh"
  [[ "$output" == *"Update Nix cargo hash"* ]]
}

@test "skips commit when nix/flake.nix unchanged" {
  cd "$BATS_TEST_TMPDIR"
  git init
  git config user.email test@test
  git config user.name test
  mkdir -p nix
  echo 'same' > nix/flake.nix
  git add .
  git commit -m "init"

  run env GITHUB_TOKEN=token GITHUB_REPOSITORY=user/repo "${BATS_TEST_DIRNAME}/commit-and-push-hash-update.sh"
  [[ "$status" -eq 0 ]]
  [[ -z "$output" ]]
}
