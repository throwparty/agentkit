load '../test_helper/common'

setup() {
  _common_setup
}

source_hash_script() {
  source "${BATS_TEST_DIRNAME}/update-cargo-hash.sh"
}

@test "extract_hash parses hash from nix output" {
  source_hash_script
  result=$(extract_hash 'error: hash mismatch in fixed-output derivation
       specified: sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=
       got:       sha256-Base64ABC123DEF456GHIJ789KLM012NOP3456789')
  [ "$result" = "sha256-Base64ABC123DEF456GHIJ789KLM012NOP3456789" ]
}

@test "extract_hash returns empty for output without hash mismatch" {
  source_hash_script
  result=$(extract_hash "All checks passed")
  [ -z "$result" ]
}

@test "update_flake_nix replaces cargoHash in flake file" {
  source_hash_script
  tmpdir=$(mktemp -d)
  cat > "$tmpdir/flake.nix" <<'EOF'
cargoHash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
EOF
  run update_flake_nix "sha256-NEWHASH1234567890123456789012345678901234" "$tmpdir/flake.nix"
  [ "$status" -eq 0 ]
  run grep 'cargoHash' "$tmpdir/flake.nix"
  [[ "$output" == 'cargoHash = "sha256-NEWHASH1234567890123456789012345678901234";' ]]
}

@test "update_flake_nix handles indented cargoHash" {
  source_hash_script
  tmpdir=$(mktemp -d)
  cat > "$tmpdir/flake.nix" <<'EOF'
  cargoHash = "sha256-OLDhashABC123=";
EOF
  run update_flake_nix "sha256-NEWhashXYZ789=" "$tmpdir/flake.nix"
  [ "$status" -eq 0 ]
  run grep 'cargoHash' "$tmpdir/flake.nix"
  [[ "$output" == '  cargoHash = "sha256-NEWhashXYZ789=";' ]]
}

@test "update_flake_nix fails when cargoHash not found" {
  source_hash_script
  tmpdir=$(mktemp -d)
  cat > "$tmpdir/flake.nix" <<'EOF'
someOtherKey = "value";
EOF
  run update_flake_nix "sha256-NEWHASH" "$tmpdir/flake.nix"
  [ "$status" -ne 0 ]
}
