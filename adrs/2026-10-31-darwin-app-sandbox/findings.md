# Findings: Darwin App Sandbox Spike

macOS 27.2 (26B123), Apple silicon, SDK 27.2. Two throwaway prototypes under
`poc/`: one built directly on `sandbox_init`, one on nono 0.81.0. Evidence for
each claim is in `poc/results/`.

Reproduce with:

```
poc/run.sh       ./poc/custom/target/release/sandbox-custom custom custom
poc/run.sh       ./poc/nono/target/release/sandbox-nono     nono   nono
poc/lifecycle.sh ./poc/custom/target/release/sandbox-custom custom
poc/egress.sh    ./poc/custom/target/release/sandbox-custom custom
```

Run them from this directory. The custom binary needs `cargo build --release` in
`poc/custom`; the fixture and
nono prototypes likewise.

## Did it work

Yes. Both prototypes confine a real Rust workload: `cargo build`, `cargo test`
and `cargo run` complete inside the sandbox, and the hostile checks report
`denied` for a write outside the workspace, a write to `$HOME`, a credential
read, a keychain read, an outbound connection and a child process attempting the
same write. Each check also asserts no artefact landed, and the run exits
non-zero if any check leaks.

`poc/results/t-009-custom.md`, `t-012-nono.md`, `t-013-lifecycle.md`,
`t-014-egress.md`.

## The minimum usable profile

Smaller than expected, once four things are right:

1. `(allow file-read* (literal "/"))` and `(allow file-read-metadata)`. Without
   either, **every exec dies with SIGABRT and no output at all** — not a denial,
   not an error.
2. The read baseline must include the toolchain's closure. A baseline of `/usr`,
   `/System`, `/Library`, `/bin` breaks every Nix binary, because coreutils
   `dlopen`s a sibling store path and aborts in dyld. It also needs the active
   developer directory, or the linker cannot find the SDK.
3. `(allow file-write* (literal "/dev/null"))`. Without it every `> /dev/null`
   fails and a harness that redirects its own output reports false negatives.
4. `TMPDIR` must exist and be inside the workspace. The Nix cc wrapper creates
   scratch there and fails if the parent is missing.

## Capability surface: what is emitted, and what is not

`render` composes four layers, in this order: the baseline (`process-exec`,
`process-fork`, `process-info*` and `signal` restricted to the same sandbox, a
`file-read*` baseline, `sysctl-read`, and `mach-lookup` over the thirteen names
in `MACH_SERVICES`); a `file-read-metadata` rule scoped to the baseline, the
grants and their ancestors; the caller's grants (`file-read*` extended with
reads and the workspace, `file-write*` including `/dev/null`, and one
`network-outbound` rule per connect grant); and finally the invariant denies.
Order is semantic: later rules win, which is why the denies come last.

Deliberately **not** emitted, with reasons:

| Capability | Why it is absent |
|---|---|
| Per-path `process-exec` | The workload needs to exec arbitrary toolchain binaries discovered at runtime; an allowlist would have to be host-derived and is not worth it until the toolchain is pinned. |
| `network-bind` / `network-inbound` | No scenario needs to listen yet; the dev-server case is descoped (see below). |
| Unix socket grants | Descoped. An AF_UNIX socket needs two rules (bind and outbound) plus a `system-socket` grant, and no scenario in this spike needed one. |
| `sysctl-read` | Required: the Rust runtime reads sysctls to size thread stacks, and removing the rule aborts startup with "failed to allocate a guard page". Left unqualified. |
| Per-service `mach-lookup` | Narrowed from a wildcard to twelve named services, each added because a run needed it. `securityd`, `trustd` and `ocspd` are excluded so a framework that needs them fails closed rather than becoming a confused deputy for the path policy. `coreservices.launchservicesd` is excluded because an application started through LaunchServices is not a descendant of the sandbox and would not inherit the profile; `open -a` is expected to fail as a result. Remaining denials (`configd`, `coreservicesd`, `mds`) are tolerated by this workload and must be triaged per toolchain. |
| `/usr/local` | Not in the baseline. It holds Homebrew configuration and whatever secrets a user has placed there; `/usr` is narrowed to `bin`, `lib`, `share` and `libexec`. |
| `pseudo-tty`, device and `iokit` rules | No scenario needs them. |

So the comparison below is about the profile language as accessible to a small
helper, not a claim that the helper already emits the whole surface.

## Lifecycle

Process groups work. `setsid` plus `SIGSTOP`/`SIGCONT`/`SIGKILL` gives working
pause, resume and delete, and the group kill reaped the loop's `sleep` child.
`poc/lifecycle.sh` now fails if the pid is not its own process-group leader, and
counts survivors by sentinel rather than by pattern-matching a global `pkill`.

The known escape is real: a process that calls `setsid` survives the group kill.
It stays confined — the sandbox is inherited and cannot be dropped — but it can
no longer be addressed by handle. Both prototypes leave it behind.

`setsid` itself is not idempotent: it fails with EPERM when the process is
already a group leader, which is the normal case when the helper is launched as
a shell job. The helper accepts that failure when `getpgrp() == getpid()` and
prints a warning, because the command then shares the caller's session and
job-control signals cross the boundary.

## Egress

Ports are the only thing that can be granted. `localhost:8765` connects; a
second local server on `8766` is refused with `EPERM`, and with no grant at all
the granted port is refused with `EPERM`.

**A destination cannot be named at all.** The profile compiler accepts exactly
two host tokens, `*` and `localhost`:

```
(remote ip "127.0.0.1:8765")   -> host must be * or localhost in network address
(remote tcp "example.com:443") -> host must be * or localhost in network address
(remote tcp "*:443")           -> compiles
(remote ip "localhost:8765")   -> compiles
```

So egress beyond loopback is all or nothing: either `*:port` for every host, or
nothing. This is a property of the mechanism, not of the wrapper, and it is the
decisive argument for a proxy. Name resolution is a second reason, not the
first.

## Diagnostics

Three layers, all working:

- `EPERM` in the child.
- A kernel log line: `Sandbox: touch(14317) deny(1) file-write-create <path>`,
  visible only with `/usr/bin/log show --info --debug`. `log` is a zsh builtin,
  and without those flags the query silently returns nothing.
- A per-sandbox tag from `(deny default (with message "<tag>"))`, which the
  custom helper now emits from `--tag`. nono cannot set it, because it owns the
  profile; that is gap 7 and the flag is accepted only for CLI parity, with a
  warning.

`sandboxd` additionally emits structured JSON with pid, process path and errno.

## Cost per command

`poc/timing.py`, 30 runs of `/usr/bin/true` each:

| Prototype | median | p95 |
|---|---|---|
| Custom | 12.2ms | 13.1ms |
| Nono | 14.5ms | 15.6ms |

A bare `/usr/bin/true` measures 2.0ms, so confinement costs roughly 10–12ms per
command.

Apply plus exec is around 12–15ms for both. Nono's 39s cold build is a build
cost and predicts nothing about command latency.

The acceptance criteria are mapped to commands and verdicts in
`poc/results/acceptance.md`: eleven proven, one partial (host-specific egress,
which the language cannot express), one out of scope (pause/resume in the
prototypes, by decision), one not applicable (the Docker path was not touched).

## Qualitative comparison

**Custom.** No dependencies, 0.85s build, a policy model of about 300 lines plus
a 250-line binary, with validation, invariants and escaping in one place. Every
capability is available because there is no intermediate model to fight. The
cost is that we own the profile, the baseline and the SBPL dialect, and that
`sandbox_init` is deprecated with no replacement.

**nono.** 203 crates, 39s cold build, keyring and sigstore in the dependency
graph for features this prototype never used. File paths map cleanly. Network
maps to localhost ports only; `allow_tcp_connect` is Linux-only and errors on
macOS. Egress and `/dev/null` both need raw `platform_rule` calls, and a seventh
gap (the tag) has no workaround at all. It also rejects non-existent paths, so
the baseline is filtered at runtime with a warning per skip.

Would we own it? The custom prototype is smaller than the code needed to paper
over nono's gaps, and the papering is raw SBPL either way. nono's real argument
is Linux: one dependency covering Landlock and macOS.

## Open risks

1. `sandbox_init` is deprecated and undocumented. SDK 27 still ships the header;
   the named profiles are already rejected. `sandbox-exec` still exists on 27.2
   but was not cross-checked here.
2. A symlink in a rule path makes the rule inert. Every rule path must be
   `realpath`ed; this is load-bearing, not hygiene.
3. Rename-shaped bypasses are covered by denying `file-write*` on the
   credential path, not by `file-write-unlink` alone. Verified: a hard link
   from `~/.ssh/id_ed25519` into the workspace fails with EPERM because the
   kernel checks the source vnode, while the same link between two files inside
   the workspace succeeds. Volume isolation is therefore not required for this
   vector, and a read-only deny would not have been enough.
4. Hostname-based egress does not work. External network access needs a proxy.
5. Orphaned processes that leave the process group cannot be reaped by handle.
6. The read baseline is host-specific, including the Nix store and the Xcode
   developer directory.
7. `mach-lookup` is now an enumerated list, but the list was derived from one
   workload on one host. Adding a toolchain will add services; the list has to
   be treated as data, not a constant.
8. The credential deny names fourteen classes of store, not every store. A home
   read grant still exposes whatever is not named, and that list has to be kept
   current as toolchains change.
9. Descriptors above stdio are closed by sweeping 3–1024; `closefrom` is not
   available in this SDK, so a supervisor holding a higher descriptor could leak
   it after 1024.
