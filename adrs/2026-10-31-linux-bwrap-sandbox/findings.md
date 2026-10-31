# Findings: Linux Bubblewrap Sandbox Spike

NixOS, kernel 7.2.10, bubblewrap 0.12.0, cargo 1.98.1. The harness and fixture
are under `poc/`. Evidence for each claim is the command and its output below.

Reproduce from the repository root:

```
nix develop ./nix --command bash -c '
  bw=$(nix build --no-link --print-out-paths nixpkgs#bubblewrap)
  PATH="$bw/bin:$PATH" bash adrs/2026-10-31-linux-bwrap-sandbox/poc/run.sh'
```

`lifecycle.sh`, `golden.sh` and `timing.py` take the same wrapper.

## Did it work

Yes. A Rust fixture built, tested and ran inside the sandbox:

```
running 1 test
test result: ok. 1 passed; 0 failed
fixture: 5
```

The hostile half reports every check blocked, and the negative control run
without the sandbox reports leaks, so the checks measure the sandbox:

```
write-outside: denied (No such file or directory (os error 2))
write-store: denied (Read-only file system (os error 30))
read-ssh-key: denied (No such file or directory (os error 2))
connect: denied (Network is unreachable (os error 101))
child-write: denied (write blocked)
signal-host: denied (pid absent)
control: leaked without the sandbox
```

The toolchain came from the host Nix store, bound read-only. The workspace was
the only host tree bound writable.

## The view

```
tmpfs /                 (bwrap's empty root)
proc  /proc
dev   /dev
tmpfs /tmp              (mode 1777)
tmpfs /home
dir   /home/sandbox
ro    /nix/store
ro    /run/current-system, /etc/static, /etc/hosts, /etc/resolv.conf,
      /etc/nsswitch.conf, /etc/passwd, /etc/group, /etc/ssl, /etc/localtime
ro    <env>  /usr/bin/env
ro    <bash> /bin/sh
rw    <workspace> /src
```

The store, the system profile and the `/etc` entries were included before the
first run. Each is plausible for a Nix toolchain; this spike did not remove
them one at a time to prove necessity. That experiment is open.

`env` resolves through the PATH to coreutils' multicall binary. Binding it as
`/usr/bin/env` preserves the argv[0] dispatch, so shebangs work.

## Denials

The mechanism is absence, not permission:

| Check | Errno | Why |
|---|---|---|
| write outside the workspace | `ENOENT` | the host home is not in the view |
| write to the Nix store | `EROFS` | the store is a read-only bind |
| read a host credential | `ENOENT` | the host home is not in the view |
| outbound connection | `ENETUNREACH` | the network namespace has loopback only |
| signal a host process | `ESRCH` | the process is outside the pid namespace |

There is no bwrap audit log. The operator gets the child's errno and the
failing path; unlike Seatbelt, there is no per-rule denial record. Diagnostics
are the command, the errno and the view.

## Isolation assertions

- The store accepts reads and rejects writes.
- The host home does not appear in the view.
- The pid namespace is fresh: `readlink /proc/self/ns/pid` was
  `pid:[4026535474]`, and `ls /proc` showed five numeric entries.
- The cgroup namespace was `cgroup:[4026535475]`, against the host's
  `cgroup:[4026531835]`.
- `/dev` contained only `core fd full null ptmx pts random shm stderr stdin
  stdout tty urandom zero`.
- A nested bwrap started a new namespace and found an empty root; its command
  must be bound or it fails with `execvp ...: No such file or directory`.

## Lifecycle

`poc/lifecycle.sh`, with the sandbox in its own session and process group:

```
lifecycle: frozen at 10, resumed to 20, killed cleanly
```

`SIGSTOP` and `SIGCONT` against the group freeze and resume the whole sandbox.
`SIGKILL` to the group reaps it. `--new-session` is omitted: it gives the child
a new session, which removes it from the group the harness can signal. A
production design that needs a session should freeze through cgroup v2 instead.

## Cost

`poc/timing.py`, 30 runs of `true`:

| | median | p95 |
|---|---|---|
| bare exec | 0.6ms | 0.8ms |
| bwrap | 6.2ms | 6.9ms |

Confinement costs about 5.6ms per command. No supervisor is needed for
latency.

## Portability

Unprivileged user namespaces work on this NixOS host. Fedora and Ubuntu were
not available to test here, and Ubuntu's AppArmor restriction on unprivileged
user namespaces is a known distribution risk that this spike did not verify.
The map's open question (T-008) is therefore answered for NixOS only.

## Open risks

1. User namespaces are required. A host that restricts them fails at bwrap
   start; the harness exits non-zero and never runs the command.
2. Egress beyond loopback needs a proxy. bwrap offers a namespace, not a
   destination filter.
3. The environment is inherited. `HOME` and `TMPDIR` are overridden, while the
   rest of the host environment passes through, including any secrets in it. A
   production implementation must clear it and forward an allowlist.
4. The `/etc` and system-profile bind list is host-specific. It was written for
   NixOS and needs a discovery pass on other distributions.
5. `--new-session` is omitted for lifecycle control. The trade-off is recorded
   above.
6. There is no seccomp or Landlock layer. The sandbox is the namespace and
   mount view; the syscall surface is the kernel's.
7. The kernel is shared. A kernel exploit escapes the sandbox.
8. The store is read-only, so `nix build` inside the sandbox fails by design.
9. There are no cgroup limits. A command can consume host CPU and memory.
10. The view is a host-specific set of paths. Moving to another distribution
    means repeating the failure-driven baseline work.
