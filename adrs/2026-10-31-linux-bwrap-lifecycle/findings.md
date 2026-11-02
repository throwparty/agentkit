# Findings: Linux Bubblewrap Lifecycle and Limits

NixOS, kernel 7.2.12, systemd user manager, cgroup v2, bubblewrap 0.12.0.

Reproduce from the repository root:

```
nix develop ./nix --command bash -c '
  bw=$(nix build --no-link --print-out-paths nixpkgs#bubblewrap)
  PATH="$bw/bin:$PATH" bash adrs/2026-10-31-linux-bwrap-lifecycle/poc/lifecycle-cgroup.sh
  PATH="$bw/bin:$PATH" bash adrs/2026-10-31-linux-bwrap-lifecycle/poc/limits.sh'
```

## Baseline

The isolation PoC is copied unchanged. The harness, argv builder and fixtures
are identical to `adrs/2026-10-31-linux-bwrap-sandbox`; that ADR records their
tests. This extension adds launchers and findings.

## Verification status

| Claim | Evidence | State |
|---|---|---|
| own session | scope run with `--new-session` | observed |
| freeze / thaw | `lifecycle-cgroup.sh`: 12 → 12 → 18 | observed |
| memory limit | `limits.sh`: 256 MiB allocation failed | observed |
| task limit | `limits.sh`: 64-thread hog failed | observed |
| fallback script | copied `lifecycle.sh`: frozen at 10, resumed to 16, killed cleanly | observed |
| systemd-less selection | `XDG_RUNTIME_DIR=/nonexistent`: exit 69, then the fallback ran | observed |
| delegation absent | none | not exercised |
| launcher cost | direct 23.8ms, scope 47.5ms median | observed |

## Cgroup lifecycle

`lifecycle-cgroup.sh` starts the harness under
`systemd-run --user --scope --unit=lbx-<pid>` with `--new-session`. The scope is
the lifecycle handle:

```
cgroup: scope lbx-poc-1557547.scope frozen at 12, resumed to 18
```

`systemctl --user freeze` stopped the loop writing to `/src/loop`;
`systemctl --user thaw` resumed it; `systemctl --user stop` removed the scope.
`unfreeze` is not a systemd verb, so the script uses `thaw`. A frozen unit
cannot be stopped, so cleanup thaws before stopping.

The sandbox command runs in its own session, which closes the shared-terminal
problem recorded in the isolation ADR.

## Resource limits

`limits.sh` applies `MemoryMax=64M`, `MemorySwapMax=0` and `TasksMax=16` to the
scope:

```
limits: memory hog bounded
limits: task hog bounded
```

A 256 MiB allocation fails, and a 64-thread creation fails, while the host stays
responsive. Both failures surface as a failed scope, so the caller sees a
non-zero workload exit.

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
so, and the copied `lifecycle.sh` then froze at 10, resumed to 16 and killed
cleanly. The fallback keeps the sandbox but loses session isolation and
resource limits.

## Shared code

No harness or argv change. The launchers wrap the same binary; the diff between
the baseline commit and this one is two scripts, the findings and the task
state.

## Open risks

1. Cgroup freeze requires a systemd user manager with delegation. Without it the fallback has no limits and shares the caller's session.
2. `systemctl --user thaw` is the correct verb; `unfreeze` does not exist.
3. A frozen unit cannot be stopped; cleanup must thaw first.
4. Memory limits surface as a failed scope or exit 137, so a policy limit looks like a workload failure.
5. `TasksMax` counts bwrap, the harness and the shell; the limit needs headroom.
6. Scope names use the pid; concurrent runs need a random suffix in production.
7. The scope launcher adds about 24ms median (Cost); `systemd-run` and `systemctl` startup dominate the difference.
8. Hosts without systemd get the reduced-protection mode: process-group lifecycle, no session isolation and no limits.
