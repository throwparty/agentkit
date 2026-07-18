_common_setup() {
  if [[ -n "$BATS_TEST_DIRNAME" ]]; then
    PROJECT_ROOT="$BATS_TEST_DIRNAME/.."
  else
    PROJECT_ROOT="."
  fi
}
