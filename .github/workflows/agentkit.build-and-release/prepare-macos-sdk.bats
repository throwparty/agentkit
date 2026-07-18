load '../test_helper/common'

setup() {
  _common_setup
}

@test "creates sdk directory without downloading when SDK_CACHED=true" {
  tmpdir="$BATS_TEST_TMPDIR"
  mkdir -p "$tmpdir/macos-sdk/MacOSX26.1.sdk/usr/lib"
  touch "$tmpdir/macos-sdk/MacOSX26.1.sdk/usr/lib/libSystem.tbd"

  SDK_CACHED=true RUNNER_TEMP="$tmpdir" run "${BATS_TEST_DIRNAME}/prepare-macos-sdk.sh"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"MacOSX26.1.sdk"* ]]
  [[ ! -f "$tmpdir/macos-sdk/MacOSX.sdk.tar.xz" ]]
}

@test "writes env vars to file when env_file argument given" {
  tmpdir="$BATS_TEST_TMPDIR"
  mkdir -p "$tmpdir/macos-sdk/MacOSX26.1.sdk"

  env_file=$(mktemp)
  SDK_CACHED=true RUNNER_TEMP="$tmpdir" run "${BATS_TEST_DIRNAME}/prepare-macos-sdk.sh" "$env_file"
  [[ "$status" -eq 0 ]]
  run grep "SDKROOT" "$env_file"
  [[ "$output" == "SDKROOT=$tmpdir/macos-sdk/MacOSX26.1.sdk" ]]
  run grep "MACOSX_DEPLOYMENT_TARGET" "$env_file"
  [[ "$output" == "MACOSX_DEPLOYMENT_TARGET=12.0" ]]
  run grep "ZIG_SYSTEM_LIB_DIR" "$env_file"
  [[ "$output" == "ZIG_SYSTEM_LIB_DIR=$tmpdir/macos-sdk/MacOSX26.1.sdk/usr/lib" ]]
  run grep "CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS" "$env_file"
  [[ "$output" == "CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS=-C link-arg=-F$tmpdir/macos-sdk/MacOSX26.1.sdk/System/Library/Frameworks -C link-arg=-L$tmpdir/macos-sdk/MacOSX26.1.sdk/usr/lib" ]]
  run grep "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS" "$env_file"
  [[ "$output" == "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS=-C link-arg=-F$tmpdir/macos-sdk/MacOSX26.1.sdk/System/Library/Frameworks -C link-arg=-L$tmpdir/macos-sdk/MacOSX26.1.sdk/usr/lib" ]]
}
