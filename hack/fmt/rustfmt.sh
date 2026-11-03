#!/usr/bin/env bash
# Format Rust files with the edition declared by their crate.
#
# rustfmt's 2024 style edition sorts imports differently from the 2021 style
# edition. Passing a single --edition to every file makes treefmt disagree with
# `cargo fmt` in a mixed-edition workspace, so group files by the edition in
# their nearest Cargo.toml.

set -euo pipefail

edition_for() {
  local dir=$1

  while [[ $dir != "." && $dir != "/" ]]; do
    if [[ -f "$dir/Cargo.toml" ]]; then
      local edition
      edition=$(sed -n 's/^edition[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$dir/Cargo.toml" | head -n 1)
      if [[ -n $edition ]]; then
        printf '%s\n' "$edition"
        return
      fi
    fi
    dir=$(dirname "$dir")
  done
}

declare -A edition_by_file=()
declare -A editions=()

for file in "$@"; do
  edition=$(edition_for "$(dirname "$file")")
  # Fall back to Cargo's default for loose files outside a crate.
  edition=${edition:-2021}
  edition_by_file[$file]=$edition
  editions[$edition]=1
done

for edition in "${!editions[@]}"; do
  files=()
  for file in "$@"; do
    if [[ "${edition_by_file[$file]}" == "$edition" ]]; then
      files+=("$file")
    fi
  done
  rustfmt --edition "$edition" --config skip_children=true "${files[@]}"
done
