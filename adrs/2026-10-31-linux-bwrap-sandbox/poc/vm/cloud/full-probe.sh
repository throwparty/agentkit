#!/usr/bin/env bash
set -u

. /etc/os-release
user=ubuntu
id "$user" >/dev/null 2>&1 || user=fedora

as_user() {
    su "$user" -c "$*"
}

echo "== full probe"
echo "os: ${PRETTY_NAME}"
echo "kernel: $(uname -r)"
echo "lsm: $(cat /sys/kernel/security/lsm 2>/dev/null || echo unknown)"
command -v getenforce >/dev/null && echo "selinux: $(getenforce)"

workspace=/tmp/lbx-ws
rm -rf "$workspace"
mkdir -p "$workspace"
chown "$user:$user" "$workspace"

sysctl -w kernel.unprivileged_userns_clone=1 >/dev/null 2>&1 || true
sysctl -w kernel.apparmor_restrict_unprivileged_userns=0 >/dev/null 2>&1 || true

as_user 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal' >/dev/null 2>&1
if ! as_user 'export PATH=$HOME/.cargo/bin:$PATH; rm -rf /tmp/lbx-src; cp -a /mnt/host/adrs/2026-10-31-linux-bwrap-sandbox/poc/bwrap /tmp/lbx-src; rm -rf /tmp/lbx-src/target; cd /tmp/lbx-src; cargo build --release'; then
    echo "harness: build failed"
    exit 1
fi
harness=/tmp/lbx-src/target/release/lbx-bwrap

set +e
as_user "$harness --workspace $workspace -- /usr/bin/true"
echo "harness userns on: exit $?"

sysctl -w kernel.unprivileged_userns_clone=0 >/dev/null 2>&1 || true
sysctl -w kernel.apparmor_restrict_unprivileged_userns=1 >/dev/null 2>&1 || true
rm -f "$workspace/sentinel"
as_user "$harness --workspace $workspace -- touch $workspace/sentinel"
echo "harness userns off: exit $?"
if [[ -e "$workspace/sentinel" ]]; then
    echo "harness userns off: command ran"
else
    echo "harness userns off: sentinel absent"
fi

sysctl -w kernel.unprivileged_userns_clone=1 >/dev/null 2>&1 || true
sysctl -w kernel.apparmor_restrict_unprivileged_userns=0 >/dev/null 2>&1 || true

gcc -O2 -o /tmp/mount-deny /mnt/host/adrs/2026-10-31-linux-bwrap-sandbox/poc/vm/cloud/mount-deny.c
as_user "/tmp/mount-deny $harness --workspace $workspace -- /usr/bin/true"
echo "harness proc mount denied: exit $?"
set -e
