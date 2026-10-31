#!/usr/bin/env bash
set -u

. /etc/os-release
user=ubuntu
id "$user" >/dev/null 2>&1 || user=fedora

as_user() {
    su "$user" -c "$*"
}

echo "== host"
echo "os: ${PRETTY_NAME}"
echo "kernel: $(uname -r)"
echo "lsm: $(cat /sys/kernel/security/lsm 2>/dev/null || echo unknown)"
if [[ -r /sys/fs/selinux/enforce ]]; then
    echo "selinux enforce: $(cat /sys/fs/selinux/enforce)"
fi
command -v getenforce >/dev/null && echo "getenforce: $(getenforce)"
for key in kernel.unprivileged_userns_clone kernel.apparmor_restrict_unprivileged_userns; do
    value=$(sysctl -n "$key" 2>/dev/null || true)
    [[ -n "$value" ]] && echo "$key: $value"
done

set +e
as_user unshare -U true
echo "unshare -U as $user: exit $?"
bwrap --version
as_user bwrap --unshare-user --ro-bind / / -- /usr/bin/true
echo "bwrap userns as $user: exit $?"
as_user bwrap --unshare-user --ro-bind / / --proc /proc -- /usr/bin/true
echo "bwrap proc as $user: exit $?"
set -e

cat >/tmp/euid.c <<'EOF'
#include <stdio.h>
#include <unistd.h>
int main(void) { printf("%d\n", geteuid()); return 0; }
EOF
gcc -o /usr/local/bin/euid-suid /tmp/euid.c
chown root:root /usr/local/bin/euid-suid
chmod 4755 /usr/local/bin/euid-suid

echo "host euid as $user: $(as_user /usr/local/bin/euid-suid)"
set +e
sandbox_euid=$(as_user "bwrap --ro-bind / / -- /usr/local/bin/euid-suid" 2>/dev/null)
status=$?
set -e
if [[ $status -eq 0 ]]; then
    echo "sandbox euid as $user: $sandbox_euid"
else
    echo "sandbox euid as $user: unavailable (bwrap exit $status)"
fi
