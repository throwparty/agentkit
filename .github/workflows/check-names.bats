load 'test_helper/common'

setup() {
  _common_setup
}

@test "passes when all steps have names" {
  cat > "$BATS_TEST_TMPDIR/test.yaml" <<'EOF'
jobs:
  test:
    steps:
      - name: Checkout
        uses: actions/checkout@v4
      - name: Run
        run: echo hello
EOF
  run "${BATS_TEST_DIRNAME}/check-names.sh" "$BATS_TEST_TMPDIR/test.yaml"
  [[ "$status" -eq 0 ]]
}

@test "fails when a step is missing name" {
  cat > "$BATS_TEST_TMPDIR/test.yaml" <<'EOF'
jobs:
  test:
    steps:
      - uses: actions/checkout@v4
EOF
  run "${BATS_TEST_DIRNAME}/check-names.sh" "$BATS_TEST_TMPDIR/test.yaml"
  [[ "$status" -eq 1 ]]
  [[ "$output" == *"missing name"* ]]
}
