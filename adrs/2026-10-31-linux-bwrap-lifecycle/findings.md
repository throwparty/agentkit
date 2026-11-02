# Findings: Linux Bubblewrap Lifecycle and Limits

NixOS, kernel 7.2.12, systemd user manager, cgroup v2, bubblewrap 0.12.0.

Reproduce from the repository root:

```
nix develop ./nix --command bash -c '
  bw=$(nix build --no-link --print-out-paths nixpkgs#bubblewrap)
  PATH="$bw/bin:$PATH" bash adrs/2026-10-31-linux-bwrap-lifecycle/poc/all.sh'
```

`all.sh` runs every probe and skips the cgroup and limit probes with a clear message when no usable user manager exists.

## Baseline

The isolation PoC is copied unchanged. The harness, argv builder and fixtures
are identical to `adrs/2026-10-31-linux-bwrap-sandbox`; that ADR records their
tests. This extension adds launchers and findings.

## Verification status

| Claim | Evidence | State |
|---|---|---|
| own session | scope run with `--new-session` | observed |
| freeze / thaw | `lifecycle-cgroup.sh`: 12 → 12 → 20 | observed |
| memory limit | `limits.sh`: 256 MiB allocation OOM-killed; 8 MiB control succeeds | observed |
| task limit | `limits.sh`: 64-thread hog refused; 4-thread control succeeds | observed |
| fallback script | copied `lifecycle.sh`: frozen at 10, resumed to 20, killed cleanly | observed |
| systemd-less selection | `XDG_RUNTIME_DIR=/nonexistent`: exit 69, then the fallback ran | observed |
| delegation absent | none: the probe checks only `systemctl --user show-environment` | accepted, unverified |
| launcher cost | direct 23.8ms, scope 47.5ms median | observed |

## Cgroup lifecycle

`lifecycle-cgroup.sh` starts the harness under
`systemd-run --user --scope --unit=lbx-<pid>` with `--new-session`. The scope is
the lifecycle handle:

```
cgroup: scope lbx-poc-45129.scope frozen at 12, resumed to 20
```

`systemctl --user freeze` stopped the loop writing to `/src/loop`;
`systemctl --user thaw` resumed it; `systemctl --user stop` removed the scope.
The baseline is sampled after freeze completes, and the frozen samples are
compared with each other; the same pattern is used after stop. `unfreeze` is
not a systemd verb, so the script uses `thaw`. A frozen unit cannot be stopped,
so cleanup thaws before stopping.

The sandbox command runs in its own session, which closes the shared-terminal
problem recorded in the isolation ADR.

## Resource limits

`limits.sh` applies `MemoryMax=64M` and `MemorySwapMax=0` to the memory probe,
and `TasksMax=16` only to the task probe:

```
limits: memory hog bounded (scope result oom-kill)
limits: task hog bounded (thread creation refused)
```

Each probe has a below-limit control under the same properties: an 8 MiB
allocation and four threads succeed. A 256 MiB allocation is OOM-killed by the
scope, and a 64-thread creation is refused with the error printed by the
Python workload. The memory probe requires the scope result `oom-kill` and the
task probe requires the workload's `thread-limit` error, so a scope that starts
but enforces nothing, or fails to start, fails the probe instead of passing it.

## Cost

Ten runs after two warmup runs:

| path | median | p95 |
|---|---|---|
| direct harness | 23.8ms | 26.4ms |
| systemd scope | 47.5ms | 56.2ms |

The scope adds about 24ms at the median. That is the cost of `systemd-run` and
the transient unit, not of the sandbox view.

## Fallback

`lifecycle-cgroup.sh` checks `systemctl --user show-environment` and exits 69
when there is no usable user manager. With `XDG_RUNTIME_DIR=/nonexistent` it did
so, and the copied `lifecycle.sh` then froze at 10, resumed to 20 and killed
cleanly. The fallback waits for the stopped and running process states before
sampling. It keeps the sandbox but loses session isolation and resource limits.

## Shared code

No harness or argv change. The launchers wrap the same binary; the diff between
the baseline commit and this one is the launchers, the findings and the task
state.

## Open risks

1. Cgroup freeze requires a systemd user manager with cgroup delegation. Delegation absence is not detected: the scope start fails and the probe exits non-zero; the caller then selects the process-group fallback, which has no limits and shares the caller's session.
2. `systemctl --user thaw` is the correct verb; `unfreeze` does not exist.
3. A frozen unit cannot be stopped; cleanup must thaw first.
4. Memory limits surface as an OOM kill or failed scope, so a policy limit looks like a workload failure.
5. `TasksMax` counts bwrap, the harness and the shell; the limit needs headroom.
6. Scope names use the pid; concurrent runs need a random suffix in production.
7. The scope launcher adds about 24ms median (Cost); `systemd-run` and `systemctl` startup dominate the difference.
8. Hosts without systemd get the reduced-protection mode: process-group lifecycle, no session isolation and no limits.
