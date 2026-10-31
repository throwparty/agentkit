# Distribution VMs

The NixOS VM test in `poc/vm/default.nix` covers userns denial, userns
acceptance and setuid on NixOS. Distro-specific policy needs a distro guest;
the Ubuntu 24.04 image below reproduces the AppArmor restriction.

## Image provenance

`fetch-images.sh` downloads and verifies the images:

| Guest | Source | SHA-256 |
|---|---|---|
| Ubuntu 24.04.5 | `cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img` | `04e8f2461a16a44e78a370284471f29616cdd9baf1893df25807b71f985b6c94` |
| Fedora 44 | `download.fedoraproject.org/.../Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2` | `28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f` |

Both hashes match the vendors' published `SHA256SUMS`/`CHECKSUM` files. The
checksum files are clearsigned, but the GPG signatures were not verified, so
provenance rests on HTTPS plus the vendor-hosted digest. Pin the Ubuntu image
by digest and re-run `fetch-images.sh`; the `current` URL moves.

Each boot uses a fresh copy-on-write overlay over the base image. The repository
is shared read-only over 9p; the guest cannot write the host checkout. QEMU is
the trusted computing base for the probe, so the guest-to-host attack surface is
QEMU itself.


## Ubuntu 24.04

```bash
curl -L -o /tmp/ubuntu-noble.img \
  https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img

# Default: unprivileged user namespaces blocked
poc/vm/cloud/launch.sh /tmp/ubuntu-noble.img userns=off lsm=apparmor

# Relaxed: unprivileged user namespaces allowed
poc/vm/cloud/launch.sh /tmp/ubuntu-noble.img userns=on lsm=none
```

`launch.sh` resolves `cloud-localds` from nixpkgs, creates a copy-on-write
overlay, writes a cloud-init seed, shares the repository at `/mnt/host`, runs
`probe.sh`, and powers the guest off. QEMU and KVM are the only host
requirements.

## Observed on 2026-11-01

Ubuntu 24.04.5, kernel 6.8.0-147, bwrap 0.9.0.

`userns=off lsm=apparmor`:

```
kernel.unprivileged_userns_clone: 0
kernel.apparmor_restrict_unprivileged_userns: 1
unshare -U as ubuntu: exit 1
bwrap userns as ubuntu: exit 1
bwrap proc as ubuntu: exit 1
host euid as ubuntu: 0
sandbox euid as ubuntu: unavailable (bwrap exit 1)
```

`userns=on lsm=none`:

```
kernel.unprivileged_userns_clone: 1
kernel.apparmor_restrict_unprivileged_userns: 0
unshare -U as ubuntu: exit 0
bwrap userns as ubuntu: exit 0
bwrap proc as ubuntu: exit 0
host euid as ubuntu: 0
sandbox euid as ubuntu: 1000
```

The first table is the failure the harness maps to exit 70; the second is the
working case with a setuid binary neutralised by the sandbox.

## Fedora 44 / SELinux

```bash
poc/vm/cloud/launch.sh /tmp/fedora44.img userns=on lsm=selinux probe=raw
```

Observed: Fedora Linux 44, kernel 6.19.10, SELinux **Enforcing**, bwrap 0.12.0.
`unshare -U`, `bwrap --unshare-user` and `bwrap --proc` all succeed as the
unprivileged user. A setuid-root helper prints euid 0 on the host and 1000 in
the sandbox.

## The Rust harness in Ubuntu

`probe=full` builds `lbx-bwrap` inside the guest with rustup and runs it against
the distro bwrap:

```
harness userns on: exit 0
harness userns off: exit 70
harness userns off: sentinel absent
harness proc mount denied: exit 1
```

`userns off` is the setup-failure path: the harness exits 70 and the command
never runs. The last line is the remaining limitation: with `mount` blocked by
seccomp, bwrap writes `child-pid` before the failing mount, so the harness sees
a child status of 1 rather than a setup failure. Detecting that needs a
readiness signal from the command side.

## SELinux

Use a Fedora Cloud image with the same launcher and `lsm=selinux`; the probe
prints `getenforce` and the namespace results.

