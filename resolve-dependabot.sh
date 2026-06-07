#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ALERT="${1:-}"

usage() { echo "usage: resolve-dependabot.sh <alert-number-or-url>" >&2; exit 1; }

[[ -n "$ALERT" ]] || usage
NUM="${ALERT##*/}"; NUM="${NUM:-$ALERT}"
[[ "$NUM" =~ ^[0-9]+$ ]] || usage

echo "--- fetch alert #$NUM ---"
json=$(gh api "/repos/$(gh repo view --json nameWithOwner --jq .nameWithOwner)/dependabot/alerts/$NUM")
ecosystem=$(echo "$json" | jq -r '.dependency.package.ecosystem')
[[ "$ecosystem" == "cargo" || "$ecosystem" == "rust" ]] || { echo "unsupported ecosystem: $ecosystem" >&2; exit 1; }

manifest=$(echo "$json" | jq -r '.dependency.manifest_path')
crate_dir="$(cd "$HERE/$(dirname "$manifest")" && pwd)"

pkg=$(echo "$json" | jq -r '.dependency.package.name')
vuln=$(echo "$json" | jq -r '.security_vulnerability.vulnerable_version_range')
fixed=$(echo "$json" | jq -r '.security_vulnerability.first_patched_version.identifier')

echo "  crate:   $crate_dir"
echo "  package: $pkg"
echo "  vuln:    $vuln"
echo "  fix:     $fixed"

# --- semver ---
ver_cmp() {
  local a=$1 b=$2; local IFS=.
  read -ra pa <<< "$a"; read -ra pb <<< "$b"
  for i in 0 1 2; do
    local va="${pa[$i]:-0}" vb="${pb[$i]:-0}"
    va="${va%%-*}"; vb="${vb%%-*}"
    (( va < vb )) && return 1; (( va > vb )) && return 2
  done; return 0
}

in_vuln_range() {
  local ver=$1 gte= gt= lte= lt=
  local IFS=,
  for part in $vuln; do
    part="${part#"${part%%[! ]*}"}"
    local op="${part%%[!><=]*}"
    local val="${part#$op}"; val="${val# }"
    case "$op" in ">=") gte=$val;; ">") gt=$val;; "<=") lte=$val;; "<") lt=$val;; esac
  done
  [[ -z "$gte" ]] || { ver_cmp "$ver" "$gte"; (( $? != 1 )) || return 1; }
  [[ -z "$gt"  ]] || { ver_cmp "$ver" "$gt";  (( $? == 2 )) || return 1; }
  [[ -z "$lte" ]] || { ver_cmp "$ver" "$lte"; (( $? != 2 )) || return 1; }
  [[ -z "$lt"  ]] || { ver_cmp "$ver" "$lt";  (( $? == 1 )) || return 1; }
  return 0
}

locked_versions() {
  grep -A2 "^name = \"$1\"" "$lock" | grep '^version[[:space:]]*=' | sed 's/version[[:space:]]*=[[:space:]]*"\(.*\)"/\1/'
}

# --- scan Cargo.lock ---
echo "--- scanning Cargo.lock ---"
lock="$crate_dir/Cargo.lock"
toml="$crate_dir/Cargo.toml"
[[ -f "$lock" ]] || { echo "  Cargo.lock not found: $lock" >&2; exit 1; }

all_versions=$(locked_versions "$pkg")
[[ -n "$all_versions" ]] || { echo "  $pkg not found in Cargo.lock" >&2; exit 1; }

vuln_version=
while IFS= read -r ver; do
  status="  $pkg $ver"
  if in_vuln_range "$ver"; then
    echo "$status is vulnerable"
    vuln_version=$ver
  else
    echo "$status is not vulnerable"
  fi
done <<< "$all_versions"

[[ -n "$vuln_version" ]] || { echo "  No vulnerable version found"; exit 0; }

# --- try: cargo update (latest compatible) ---
cd "$crate_dir"
echo ""
echo "--- cargo update -p $pkg@$vuln_version (latest compatible) ---"
cargo update -p "${pkg}@${vuln_version}" 2>&1

still_vuln=
while IFS= read -r ver; do
  if in_vuln_range "$ver"; then
    echo "  $pkg $ver still vulnerable"
    still_vuln=1
  else
    echo "  $pkg $ver is now safe"
  fi
done <<< "$(locked_versions "$pkg")"

[[ -z "${still_vuln:-}" ]] && echo "  fixed by compatible update" && exit 0

# --- try: upstream cargo update ---
echo ""
echo "  compatible update didn't help (still $vuln_version, needs $fixed+)"
echo "  Looking up who depends on $pkg …"

# Use cargo's dependency graph rather than parsing Cargo.lock text heuristically.
upstreams=$(
  cargo tree --edges normal --invert "$pkg" 2>/dev/null \
  | sed '1d' \
  | grep -E '^    [^ ]' \
  | grep -vE '^        ' \
  | sed -E 's/^    ([^ ]+).*/    \1/' \
  | sort -u
)
if [[ -n "$upstreams" ]]; then
  echo "  Directly pulled in by:"
  echo "$upstreams"
else
  echo "  (could not determine upstream crate)"
fi

echo ""
echo "  This is a transitive dependency. Options:"
echo "    1. cargo update -p <upstream-crate>  — bump the crate that pulls in $pkg"
echo "    2. Add $pkg = \"$fixed\" to [dependencies] to force a compatible version"
echo "    3. If the upstream pins $pkg tightly, its Cargo.toml needs a version bump"
exit 1