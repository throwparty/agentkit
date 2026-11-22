# Evidence

## Gates

| Gate | Result |
|------|--------|
| `cargo test -p agentkit-litterbox` | 588 passed, 0 failed, 14 ignored |
| `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings` | clean |
| `arch-lint check` | 431 files, 0 errors |
| `bash hack/arch-lint-negative.sh` | every plant rejected and the tree restored |

## Coverage

- The pure plan: `orchestrator::source` table tests cover the axes, the classification and every action.
- The refs and identity: `vcs::git::vcs_tests` covers the three refs, legacy initialisation, delete, `tree`, `observe`, the snapshot split, the compare-and-swap and ancestry.
- The host commands: `vcs::git::reconcile_tests` covers the report base, the marker tree, the index stages, `MERGE_HEAD`, the untouched checkout, the second-stage refusal, finish and discard.
- The wiring: `orchestrator::service` tests cover a read that skips observation, a command that snapshots once, and a divergence that records `Pending` and holds the branch.
- Real backends: the bound bubblewrap and Docker seam tests still pass. A bound workspace is observed in place rather than downloaded, which keeps the masked `.git` volume out of the transfer.
- Locality: `vcs::git::remote` covers the exposed remote's fetch refspec for `refs/litterbox/workspaces/*`.

## Not run here

A manual CLI smoke under both `snapshot-mode` values was not run in this environment. `create` is an MCP tool rather than a CLI command, and the real-backend tests need the local Docker and bubblewrap setup. The automated coverage exercises the same orchestrator path.
