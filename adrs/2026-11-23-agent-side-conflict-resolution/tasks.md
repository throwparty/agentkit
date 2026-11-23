# Agent-side conflict resolution

## Tasks

### T-001: [x] Richer conflict values

Add `hunks`, `sides` and `base` to `vcs::Conflict` and update every construction site: `vcs/mod.rs` stubs, `testing/vcs.rs`, `JjVcs::conflicts` and the reconcile adapter that maps conflicts to paths.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo test -p agentkit-litterbox` passes; a unit test asserts that a modify/modify conflict reports `sides: 2` and `base: true` on both backends. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | conflict-report |

### T-002: [x] Git marker scan

Implement `GitVcs::conflicts` by scanning the tree a snapshot would commit for lines beginning with the three conflict markers and counting regions per path. Replace the `Unsupported` return at `vcs/git/scm.rs:886`.


| Field | Value |
|-------|-------|
| Success Criteria | A tree carrying markers reports one conflict with the right region count and a clean tree reports none; an absent revision returns an error. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-001 |
| References | git-conflict-scan,conflict-report,git-no-index |

### T-003: [x] jj conflict counts

Parse `path    N-sided conflict` in `vcs/jj/ops.rs` so the side count is kept rather than dropped, and count conflict regions in the materialised tree.


| Field | Value |
|-------|-------|
| Success Criteria | `JjVcs::conflicts` returns one entry per path with its side and region counts; the existing exit-2 handling still reads a clean revision as clean. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On | T-001 |
| References | conflict-report,introducing-commit |

### T-004: [x] Add Vcs::blob

Add `blob(tree, path) -> Option<Vec<u8>>` to the contract and implement it on both backends, using git2 for Git and the clone's Git backend for jj.


| Field | Value |
|-------|-------|
| Success Criteria | A round-trip test reads a known file from a known tree on both backends and gets its bytes; an absent path returns None. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On | T-001 |
| References | conflict-sides,no-new-dependency |

### T-005: [x] Git conflict sides

Implement `GitVcs::conflict_sides` from the stage entries of a `merge-tree` run against the recorded base.


| Field | Value |
|-------|-------|
| Success Criteria | A modify/modify conflict returns base, ours and theirs as distinct trees; an add/add conflict returns no base. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-004 |
| References | conflict-sides,no-base-side |

### T-006: jj conflict sides

Implement `JjVcs::conflict_sides` from the parents of the resolution revision, replacing the `Unsupported` return at `vcs/jj/mod.rs:669`.


| Field | Value |
|-------|-------|
| Success Criteria | A two-sided conflict returns base, ours and theirs; a conflict with no base reports it absent. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-004 |
| References | conflict-sides,no-base-side |

### T-007: [x] The resolution record and the three actions

Add `orchestrator/resolve.rs` with begin, finish and abandon entry points, plus the stored field that records an open resolution with its `synced` revision. The live revision is read fresh on every operation and never stored.


| Field | Value |
|-------|-------|
| Success Criteria | A stub backend drives begin, a second begin, finish and abandon; the record survives a reopen, clears on finish and abandon, and begin on a sandbox with no conflict fails naming the state. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-001 |
| References | open-resolution,begin-resolution,second-begin,begin-without-conflict,introducing-commit |

### T-008: [x] Materialise the conflict

Begin writes the conflicted revision into the workspace directory: `materialise` with the snapshot filter for Git and jj `Copied`, and `jj new <shared> <sandbox>` for jj `Bound`.


| Field | Value |
|-------|-------|
| Success Criteria | A Git sandbox and both jj bindings end with markers in `/src`, ignored paths untouched, and snapshots committing to the revision under resolution. |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On | T-007 |
| References | resolution-context,bound-resolution,copied-resolution,snapshot-during-resolution |

### T-009: [x] Abandon

Abandon rebuilds the merge from its recorded parents, writes it again, and restores the sandbox branch tree when no merge was needed.


| Field | Value |
|-------|-------|
| Success Criteria | After abandon `/src` holds the branch tree, no conflict is open, the shared branch has not moved, and no operation log restore ran. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-008 |
| References | abandon-resolution,abandon-restores |

### T-010: [x] Finish and the per-side report

Finish re-reads the conflict state, fails when the backend still reports one, computes the difference from base, ours and theirs, performs the host write, and returns the report alongside the new tip.


| Field | Value |
|-------|-------|
| Success Criteria | A clean resolution lands and reports the per-side difference; a resolution that drops a side lands and reports it; a stale tip is abandoned and replanned. |
| Complexity | 🔴 High |
| Effort | 8-12h |
| Depends On | T-005,T-006,T-008 |
| References | finish-resolution,resolution-report,no-lost-writes,introducing-commit,branch-moves-during-resolution |

### T-011: [x] sandbox-reconcile carries the actions

Add `action` with the values `begin`, `finish` and `abandon` to the MCP tool and stop it staging a host worktree.


| Field | Value |
|-------|-------|
| Success Criteria | An MCP call creates no host worktree; each action returns its result with the workspace block appended. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-007 |
| References | begin-resolution,finish-resolution,no-host-writes,mcp-host-isolation |

### T-012: [x] Add sandbox-status

New MCP tool returning the source state, the revisions, and per-path hunk and side counts, with no region text.


| Field | Value |
|-------|-------|
| Success Criteria | A conflicted sandbox reports its paths with counts, a clean sandbox reports none, and the output size does not grow with file content. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-002,T-003 |
| References | conflict-report,stalled-resolution,bounded-reports |

### T-013: [x] read gains a side

Add `side` to `ReadArgs`. When it is present, answer from `conflict_sides` and `blob` instead of running `cat` in the sandbox.


| Field | Value |
|-------|-------|
| Success Criteria | `read` with each side returns that side's bytes; a side with no base fails naming it; `read` without a side behaves as it does today. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-005,T-006 |
| References | conflict-sides,no-base-side |

### T-014: The workspace block

Add the `resolution` object to the block appended by every mutating tool, derived from the post-plan state.


| Field | Value |
|-------|-------|
| Success Criteria | A tool call during resolution reports the revision and the conflicts with counts; a tool call outside resolution reports the block unchanged. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On | T-007,T-001 |
| References | state-reporting |

### T-015: Delete during resolution

Remove the workspace and the record when a sandbox with an open resolution is deleted.


| Field | Value |
|-------|-------|
| Success Criteria | Deleting mid-resolution leaves no branch, no host directory and no record. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-007 |
| References | delete-during-resolution,open-resolution |

### T-016: The acceptance suite

End-to-end tests for a Git divergence, a jj divergence and a jj conflicted tip, plus the main workspace build check.


| Field | Value |
|-------|-------|
| Success Criteria | The three end-to-end criteria pass, and a command in the sandbox succeeds whenever no resolution is open. |
| Complexity | 🔴 High |
| Effort | 8-12h |
| Depends On | T-009,T-010,T-011,T-012,T-013,T-014 |
| References | git-divergence-resolved,jj-divergence-resolved,jj-tip-fixed,main-workspace-buildable,lost-side-reported |

### T-017: Boundaries and gates

Add the new modules to the arch-lint scopes and confirm that no Git type crosses into `vcs::jj` or the orchestrator.


| Field | Value |
|-------|-------|
| Success Criteria | `arch-lint check`, `hack/arch-lint-negative.sh`, clippy with `-D warnings` and the full test suite pass. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On | T-016 |
| References | no-new-dependency,gates,gates-pass |

