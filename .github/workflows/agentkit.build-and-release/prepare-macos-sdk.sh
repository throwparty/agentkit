#!/usr/bin/env bash
set -euo pipefail

main() {
  local env_file="${1:-${GITHUB_ENV:-}}"
  local sdk_url="${SDK_URL:-https://github.com/joseluisq/macosx-sdks/releases/download/26.1/MacOSX26.1.sdk.tar.xz}"
  local sdk_cached="${SDK_CACHED:-false}"
  local sdk_dir="${RUNNER_TEMP:-/tmp}/macos-sdk"

  mkdir -p "$sdk_dir"

  if [[ "$sdk_cached" != "true" ]]; then
    curl -L "$sdk_url" -o "$sdk_dir/MacOSX.sdk.tar.xz"
    tar -xf "$sdk_dir/MacOSX.sdk.tar.xz" -C "$sdk_dir"
  fi

  local sdkroot
  sdkroot=$(find "$sdk_dir" -maxdepth 1 -type d -name 'MacOSX*.sdk' | head -n 1)

  local framework_dir="$sdkroot/System/Library/Frameworks"
  local syslib_dir="$sdkroot/usr/lib"

  if [[ -n "$env_file" ]]; then
    {
      printf "SDKROOT=%s\n" "$sdkroot"
      echo "MACOSX_DEPLOYMENT_TARGET=12.0"
      echo "ZIG_SYSTEM_LIB_DIR=$syslib_dir"
      echo "CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS=-C link-arg=-F$framework_dir -C link-arg=-L$syslib_dir"
      echo "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS=-C link-arg=-F$framework_dir -C link-arg=-L$syslib_dir"
    } >> "$env_file"
  else
    export SDKROOT="$sdkroot"
    export MACOSX_DEPLOYMENT_TARGET=12.0
    export ZIG_SYSTEM_LIB_DIR="$syslib_dir"
    export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-F$framework_dir -C link-arg=-L$syslib_dir"
    export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-F$framework_dir -C link-arg=-L$syslib_dir"
  fi

  printf '%s\n' "$sdkroot"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
