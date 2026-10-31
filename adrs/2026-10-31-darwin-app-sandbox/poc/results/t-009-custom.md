# T-009 custom end-to-end run

macOS 27.2 (26B123). Workspace `/private/tmp/lbx-custom`, toolchain read from the
Nix store and `~/.cargo`, synthetic home at `/private/tmp/lbx-home-custom`.

Reproduce:

```
nix develop ./nix --command ./poc/run.sh ./poc/custom/target/release/sandbox-custom custom custom
```

## Workload

```
cargo build, cargo test, cargo run
fixture: 5
```

The fixture's own tests run; the lib target reports `running 2 tests`.

## Hostile checks

```
write-outside: denied
write-home: denied
read-ssh-key: denied
read-keychain: denied
connect: denied
child-write: denied
```

Each check also stats its target and reports `leak` if an artefact exists
regardless of the syscall result. The run exits non-zero if any check fails, so a
green run is evidence rather than decoration.

## Negative control

The same fixture binary run without the sandbox reports `leak` for every
filesystem check, so the checks measure the sandbox rather than the fixture's
error handling.

## What the workload forced into the profile

- `/nix/store` and `/Applications/Xcode.app/Contents/Developer` in the read
  baseline. Every Nix binary dlopens a sibling store path, and the linker needs
  the SDK.
- `TMPDIR` inside the workspace, set by the helper and created if absent.
- `(allow file-read* (literal "/"))` plus `file-read-metadata`. Without them
  every exec dies with `SIGABRT` and no message at all.
- A write grant for `/dev/null`.
