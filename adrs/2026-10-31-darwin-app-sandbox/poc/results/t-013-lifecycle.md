# T-013 lifecycle

`poc/lifecycle.sh`, run against both prototypes. The sandbox is its own process
group: the prototype calls `setsid()` before applying the profile, checks the
return value, and exits 70 if it could not establish one.

| Check | Custom | Nono |
|---|---|---|
| pid is its own process-group leader | yes (harness fails otherwise) | yes |
| pause (SIGSTOP to the group) freezes the loop | yes | yes |
| resume (SIGCONT) continues | yes | yes |
| delete (SIGKILL to the group) | gone, 0 survivors by sentinel | gone, 0 survivors |
| process that calls `setsid` survives the group kill | yes, 1 survivor | yes, 1 survivor |

The harness exits non-zero if pause does not freeze, resume does not resume, the
group signal is not addressed to the group, or a sentinel survivor is found after
the kill. A `kill -X "-$PID"` that fails is a failure, not a silent fallback to
the single pid.

## Notes

- Survivors are counted by a per-run sentinel argument rather than a global
  `pgrep -f` pattern, and cleanup targets the pids the harness started.
- The `setsid` case is the escape the spec predicted: the survivor is still
  confined — the sandbox is inherited and cannot be dropped — but it can no
  longer be addressed by process group, so a supervisor cannot reap it.
- nono emits `Error opening /private/var/select/sh` during the shell invocation
  (gap 4). It does not affect the lifecycle behaviour.

## Consequence for a wrapper

Delete must be able to reap more than one process group, or the sandbox handle
must record the pid of every process that leaves the group. Neither prototype
does this; both leave an orphan behind, confined but unkillable by handle.
