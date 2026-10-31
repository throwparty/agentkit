# VM probes: host-policy cases

These are NixOS VM tests, built from `nixpkgs` (the registry flake in this
expression; swap in the repository's flake input to pin it). QEMU and KVM are
the only runtime requirements; there is no cloud image, cloud-init seed or
`cloud-localds`.

## Run

```bash
nix build --impure --expr 'import ./adrs/2026-10-31-linux-bwrap-sandbox/poc/vm/default.nix'
```

The build runs two guests:

- `enabled` — unprivileged user namespaces allowed. Checks `unshare -U`,
  `bwrap --unshare-user`, a setuid-root helper on the host and the same helper
  through a bwrap read-only bind, and `bwrap --proc /proc`.
- `disabled` — `user.max_user_namespaces = 0`. Checks that both `unshare -U` and
  `bwrap --unshare-user` fail. The harness maps that failure to exit 70.

The setuid check compiles a helper that prints `geteuid()`, installs it
root-owned and mode 4755, and asserts the host run is uid 0 while the sandbox
run is the unprivileged uid. This is the test that cannot run on the
workstation without root.

## Distro guests

NixOS cannot reproduce Ubuntu's AppArmor userns restriction or Fedora's SELinux
policy. `poc/vm/cloud/` boots a distro cloud image with cloud-init and the
repository over 9p. The Ubuntu 24.04 run and its results are in
`poc/vm/cloud/README.md`; SELinux was not run.

The `/proc` check confirms `bwrap --proc` works in the enabled guest. Forcing the
mount to fail needs a profile that denies the mount syscall; the userns
`disabled` case already exercises the setup-failure exit path.
