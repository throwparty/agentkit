# T-017 teardown check

## Isolation from the suite

- No agentkit crate or the root manifest references `sandbox-custom`,
  `sandbox-nono` or `darwin-app-sandbox/poc`. The root workspace lists its
  members explicitly and carries `exclude = ["adrs"]`, so neither prototype can
  be built or linked by the suite even by accident.
- Both prototype manifests declare an empty `[workspace]` table, so they are
  their own workspace roots and do not inherit the agentkit dependency graph.

## Docker path still green

```
test result: ok. 141 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`cargo test -p agentkit-litterbox` is unchanged by this work.

## Removal

Deleting the spike is:

1. `rm -rf adrs/2026-10-31-darwin-app-sandbox/poc`
2. Keep `spec.toon`, `plan.toon`, `tasks.toon`, `findings.md` and their
   markdown: these are the ADR. `findings.md` is the deliverable, not scratch.

Nothing else references the prototypes, nothing is installed, no service is
registered, and no launch agent or privileged helper was created. Scratch the
scripts create, each removed by its own trap where noted:

| Path | Created by | Removed |
|---|---|---|
| `/private/tmp/lbx-<label>` | `run.sh` | no (kept for inspection) |
| `/private/tmp/lbx-home-<label>` | `run.sh` | no (synthetic credential home) |
| `/private/tmp/lbx-life-<label>` | `lifecycle.sh` | yes, on exit |
| `/private/tmp/lbx-egress-<label>` | `egress.sh` | yes, on exit |
| `/private/tmp/lbx-golden` | golden test | no |
| `/private/tmp/lbx-<label>-profile.sbpl` | `run.sh` | no (kept as the applied-profile record) |
| `/private/tmp/lbx-failclosed-<label>` | `failclosed.sh` | yes, on exit |
| `/private/tmp/lbx-inside-<label>` | `inside-home.sh` | yes, on exit |
| `/private/tmp/lbx-ps-<label>` | `ps-capture.sh` | yes, on exit |
| `/private/tmp/lbx-timing-<label>` | `timing.py` | yes, on exit |
| `/private/tmp/lbx-escape-<label>` | fixture hostile check | attempted; denied inside the sandbox |
| `/private/tmp/lbx-escape` | fixture hostile check | attempted, denied |

## What would have to change before this became production

Not a teardown item, recorded here because it is the point of the spike: the
prototypes live outside the workspace on purpose, and none of the Litterbox
interfaces were touched. Anything production would need its own placement
decision, and the current `Compute` abstraction is still Docker-shaped.
