# Findings: Linux Bubblewrap Sandbox Spike

NixOS, kernel 7.2.12, bubblewrap 0.12.0, cargo 1.98.1. The harness and fixture
are under `poc/`. Evidence for each claim is the command and its output below.

Reproduce from the repository root:

```
nix develop ./nix --command bash -c '
  bw=$(nix build --no-link --print-out-paths nixpkgs#bubblewrap)
  PATH="$bw/bin:$PATH" bash adrs/2026-10-31-linux-bwrap-sandbox/poc/all.sh'
```

`all.sh` builds and unit-tests the harness, then runs every harness. `timing.py`
runs last and needs `python3`.

## Did it work

Yes. A Rust fixture built, tested and ran inside the sandbox:

```
running 1 test
test result: ok. 1 passed; 0 failed
fixture: 5
```

The hostile half reports every check blocked, with the errno or exit status the
probe observed:

```
write-outside: denied (No such file or directory (os error 2))
write-store: denied (Read-only file system (os error 30))
read-ssh-key: denied (No such file or directory (os error 2))
connect: denied (Network is unreachable (os error 101))
loopback: ok
signal-host: denied (exit status: 1: kill: No such process)
child-write: denied (exit status: 1: touch: No such file or directory)
```

The control run without the sandbox reports leaks for the checks that can be
distinguished there: a canary credential planted in the host home is readable,
a file is written outside the workspace, a child writes, and the host pid is
signalable. The credential is planted before every run, so its denial measures
the view rather than an absent file. Artefacts are asserted absent on the host
and in the store after the hostile run.

## The view

```
tmpfs /                 (bwrap's empty root)
proc  /proc
dev   /dev
tmpfs /tmp              (mode 1777)
tmpfs /home
dir   /home/sandbox
ro    /nix/store
ro    /run/current-system, /etc/static
ro    /etc/hosts, /etc/resolv.conf, /etc/nsswitch.conf,
      /etc/passwd, /etc/group, /etc/ssl, /etc/localtime
ro    <env>  /usr/bin/env
ro    <bash> /bin/sh
rw    <workspace> /src
```

The environment starts from `--clearenv` and an allowlist: `PATH`, `TERM`,
`LANG`, `TZ`, build variables (`CC`, `CXX`, `AR`, `LD`, `CFLAGS`, `LDFLAGS`,
`PKG_CONFIG_PATH`, `RUSTFLAGS`, `RUSTC_WRAPPER`, `SOURCE_DATE_EPOCH`), and the
`NIX_`, `CARGO_`, `RUSTUP_`, `SSL_CERT_`, `GIT_SSL_` and `LC_` prefixes.
Credentials and API keys are not on the list. `HOME`, `TMPDIR`, `USER` and
`LOGNAME` are fixed by the harness and cannot be overridden by `--env`.

Grants are validated in both directions against protected roots (`/nix`,
`/etc`, `/run`, `/proc`, `/sys`, `/dev`, `/var`), the host home and `/tmp`.
The workspace may live under `/tmp`; no other read grant may. There are no
writable grants: the workspace is the only writable host tree. `HOME` must be
set and resolvable, so the home guard cannot fail open. bwrap itself is resolved
once to a canonical absolute path, checked root-owned and not group or world
writable, and that path is what runs.

Each grant is opened once with `openat2(O_PATH, RESOLVE_NO_SYMLINKS |
RESOLVE_NO_MAGICLINKS)` and held as a descriptor. bwrap mounts
`/proc/self/fd/N`, so the object validated is the object mounted; a path
swapped after validation cannot change what the sandbox sees. The command's
exit comes from bwrap's `--json-status-fd`: a setup failure that never starts
the child exits 70, while the workload's own status is passed through.
`--omit` exists only in the attribution feature build; the default binary
rejects it.

## Baseline attribution

`poc/attribution.sh` builds the fixture in a fresh workspace per case:

```
full: 0
omit-etc: 0
omit-system: 0
omit-shell: 0
omit-store: 1
```

The fixture builds without the `/etc` group, the system profile and the shell
binds. Removing the store fails with `execvp cargo: No such file or directory`,
which reproduces the dependency. The store is the one required host tree. The
full view stays the default because shebang scripts need the shell binds and
NixOS tools expect the system profile.

## Denials

| Check | Observation | Why |
|---|---|---|
| write outside the workspace | `ENOENT` | the host home is not in the view |
| write to the Nix store | `EROFS` | the store is a read-only bind |
| read a host credential | `ENOENT` | the host home is not in the view |
| outbound connection | `ENETUNREACH` | the network namespace has loopback only |
| host echo server | control reaches it; sandbox gets `ENETUNREACH` | the namespace has no route to the host address |
| loopback inside the namespace | connects | the loopback device is usable |
| signal a host process | `kill: No such process` | the process is outside the pid namespace |
| child process repeats the write | `touch: No such file or directory` | confinement is inherited |

There is no bwrap audit log. The operator gets the child's errno or exit status
and the failing path. Diagnostics are the command, the argv on failure, and the
view.

## Isolation assertions

- The store accepts reads and rejects writes.
- The host home does not appear in the view.
- The pid namespace is fresh: `readlink /proc/self/ns/pid` differs from the host, and `ls /proc` shows five numeric entries.
- The cgroup namespace is fresh: `cgroup:[4026535475]` against the host's `cgroup:[4026531835]`.
- `/dev` contains only `core fd full null ptmx pts random shm stderr stdin stdout tty urandom zero`.
- A nested bwrap is blocked: `--disable-userns` makes inner namespace creation fail with ENOSPC.
- The workspace contains no setuid binary, and `/src` reports `nosuid` in `/proc/self/mountinfo`.

## Process lifetime

The sandbox lives as long as the command bwrap was asked to run. A command that
replaces itself with `exec` holds it: `sh -c 'exec sleep 1'` took 1.01s and a
direct `sleep 1` took 1.01s. A background child does not: `sh -c 'sleep 1 &'`
returned in 12ms and the child was killed at teardown, with no survivor found by
`pgrep`. A long-running service must be the top-level command of its own
invocation.

## Lifecycle

`poc/lifecycle.sh`, with the harness becoming a session leader itself:

```
lifecycle: leader pgid 1538737, frozen at 10, resumed to 16, killed cleanly
```

The harness calls `setsid` before `exec`, so the pid is its own process-group
leader; the script asserts that before signalling. `SIGSTOP` and `SIGCONT`
against the group freeze and resume the sandbox, and `SIGKILL` reaps it.
`--new-session` is available but off by default: it gives the child a new
session, which removes it from the group the lifecycle uses. A production design
that needs a session should freeze through cgroup v2.

## Cost

`poc/timing.py`, 30 runs after 3 warmup runs:

| | median | p95 |
|---|---|---|
| bare exec | 3.7ms | 4.3ms |
| bwrap | 38.3ms | 40.6ms |

Timings vary with load; these are one run of 30.

The sandboxed figure includes a `bwrap --version` probe per invocation, argv
construction, the environment allowlist and bwrap setup. The probe and the
environment size are the obvious production targets; no supervisor is needed
for latency.

## Version floor

The harness requires bwrap 0.8.0 or newer. The floor covers the flags it emits:
`--unshare-cgroup`, `--disable-userns`, `--die-with-parent`, `--ro-bind-try`
and `--perms`.
The version parser accepts `0.12.0`, `0.11.0-dev`, `0.9.1.1` and `v0.12.0`.
A missing or untrusted bwrap and a below-floor bwrap each exit 69 with a message
naming the floor. A bwrap that is not root-owned or is group or world writable
is refused rather than executed.

## Host requirement

Unprivileged user namespaces work on this NixOS host: `unshare -U true` exits 0
on kernel 7.2.12, and bwrap 0.12.0 runs. Ubuntu 24.04.5 (kernel 6.8, bwrap
0.9.0) blocks them for an unprivileged user by default through
`kernel.unprivileged_userns_clone=0` and
`kernel.apparmor_restrict_unprivileged_userns=1`; relaxing both makes them work.
The hard requirement is the host policy, and the failure is bwrap refusing to
create the namespace.

## Distribution VMs

Two VM harnesses cover the host-policy cases the workstation cannot.

`poc/vm/default.nix` is a NixOS test with two guests. The `disabled` guest sets
`user.max_user_namespaces=0`; `unshare -U` and `bwrap --unshare-user` fail. The
`enabled` guest runs both, and a setuid-root helper prints euid 0 on the host
and a non-zero euid through bwrap.

`poc/vm/cloud/launch.sh` boots a distro cloud image with a cloud-init seed and
the repository over 9p. On Ubuntu 24.04.5:

| Configuration | `unshare -U` / bwrap as `ubuntu` | Setuid euid (host / sandbox) |
|---|---|---|
| `userns=off`, AppArmor restriction on | both fail with `EPERM` | 0 / sandbox unavailable |
| `userns=on`, restriction off | both succeed | 0 / 1000 |

That is the Ubuntu AppArmor policy the NixOS test cannot reproduce. A Fedora
44 guest with SELinux enforcing (kernel 6.19.10, bwrap 0.12.0) allows
`unshare -U`, bwrap and `--proc` as an unprivileged user; a setuid helper prints
0 on the host and 1000 in the sandbox.

The Rust harness also builds and runs inside Ubuntu 24.04 against the distro
bwrap: userns on exits 0; userns off exits 70 with the sentinel absent. With
`mount` blocked by seccomp, the harness exits 1: bwrap has already written
`child-pid` before the failing mount, so that late setup failure is
indistinguishable from a workload exit of 1.

## Fail closed

`poc/failclosed.sh` rejects every invalid configuration with the documented
code, the command sentinel is absent, and a positive control proves the
sentinel path works:

| Case | Exit |
|---|---|
| missing workspace | 64 |
| workspace under the host home | 64 |
| read grant for the host home | 64 |
| read grant for `/etc` | 64 |
| unknown argument | 64 |
| no command | 64 |
| bwrap missing | 69 |
| bwrap untrusted | 69 |

A bwrap that starts but fails setup before the child exists exits 70; the path
is implemented and is not deterministically exercisable on this host.

## Assessment

The argv builder is the sensitive artefact and it is small: a typed view, a
validator, a fixed baseline and one renderer. The policy is legible in the
emitted argv, and the tests exercise the view rather than a shell contract. The
time-of-check/time-of-use gap is closed by mounting descriptors: a path swapped
after validation cannot change the object in the sandbox. The cost of ownership
is the baseline and the denial model; bwrap itself is maintained and its flags
are stable.

Configuration effort is moderate. The store is the only host tree the toolchain
requires; the rest of the baseline is defensive. The bind list is host-specific
and will need attribution on a new distribution.

Recommendation: proceed to a production isolation module, lifting the validator,
pinned bwrap resolution, environment allowlist and renderer. Keep this harness as
the oracle for that module. Add a proxy for egress, cgroup v2 for lifecycle and
session control, and a distinct exit code for confinement setup versus workload
failure.

## Open risks

1. User namespaces are required. A host that restricts them fails at bwrap start; the harness exits non-zero and never runs the command.
2. Egress beyond loopback needs a proxy. bwrap offers a namespace, not a destination filter.
3. The kernel is shared. A kernel exploit escapes the sandbox.
4. The store is read-only, so `nix build` inside the sandbox fails by design.
5. There are no cgroup limits. A command can consume host CPU and memory; the follow-up ADR covers limits and cgroup freeze.
6. `--disable-userns` blocks workloads that create their own user namespaces, such as some browsers. A per-workload decision may be needed.
7. `--new-session` is off by default for lifecycle control. A command shares the caller's session unless the flag is set; the follow-up ADR covers session creation with cgroup freeze.
8. The setup-failure exit path catches failures before bwrap writes `child-pid`; a mount failure after that (forced with a seccomp `mount` block) surfaces as exit 1. A readiness signal from the command side would close the gap.
9. The NixOS VM test covers userns denial and setuid; Ubuntu 24.04 covers AppArmor and runs the harness; Fedora 44 covers SELinux. A crafted setuid binary and a proc failure were exercised in the distro guests.
10. The attribution result is workload-specific: a fixture with C dependencies or network needs will require more of the baseline.
11. The environment allowlist is a heuristic. The Nix provider's activation capture will replace it with the variables the devShell declares plus a secret-name denylist.
12. The harness requires a root-owned bwrap on PATH. A user-owned copy is skipped in favour of a later trusted candidate; a PATH with only untrusted copies fails with the install guidance.
