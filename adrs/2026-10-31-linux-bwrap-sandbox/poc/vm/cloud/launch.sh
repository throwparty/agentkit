#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
    echo "usage: launch.sh <cloud-image> [userns=on|off] [lsm=apparmor|none]" >&2
    exit 64
fi

image=$(realpath "$1")
userns=${2:-userns=off}
lsm=${3:-lsm=apparmor}
probe=${4:-probe=raw}
probe_script=${probe#probe=}
case "$probe_script" in
    raw) probe_script=probe.sh ;;
    full) probe_script=full-probe.sh ;;
    *) echo "probe must be raw or full" >&2; exit 64 ;;
esac
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../../../../.." && pwd)

cloud_utils=$(nix build --no-link --print-out-paths nixpkgs#cloud-utils)
export PATH="$cloud_utils/bin:$PATH"

case "$userns" in
    userns=on) userns_bool=1 ;;
    userns=off) userns_bool=0 ;;
    *) echo "userns must be on or off" >&2; exit 64 ;;
esac
case "$lsm" in
    lsm=apparmor) apparmor_bool=1 ;;
    lsm=none|lsm=selinux) apparmor_bool=0 ;;
    *) echo "lsm must be apparmor, selinux or none" >&2; exit 64 ;;
esac

work=$(mktemp -d /tmp/lbx-cloud.XXXXXX)
trap 'rm -rf "$work"' EXIT

qemu-img create -q -f qcow2 -F qcow2 -b "$image" "$work/disk.qcow2" 20G
sed -e "s/@USERNS_BOOL@/$userns_bool/" \
    -e "s/@APPARMOR_BOOL@/$apparmor_bool/" \
    -e "s/@PROBE@/$probe_script/" \
    "$here/cloud-init.yaml.in" >"$work/user-data"
printf 'instance-id: lbx\nlocal-hostname: lbx\n' >"$work/meta-data"
cloud-localds "$work/seed.iso" "$work/user-data" "$work/meta-data"

timeout 1200s qemu-system-x86_64 \
    -machine q35,accel=kvm:tcg \
    -m 4096 -smp 4 \
    -nographic -no-reboot \
    -nic user,model=virtio-net-pci \
    -drive file="$work/disk.qcow2",if=virtio,format=qcow2 \
    -drive file="$work/seed.iso",if=virtio,format=raw \
    -virtfs "local,path=$repo,mount_tag=host0,security_model=none,readonly=on"
