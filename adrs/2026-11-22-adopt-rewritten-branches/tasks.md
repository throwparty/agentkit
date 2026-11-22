# Reconcile rewritten branches tasks

## Tasks

### T-001: [x] Source state and plan

Add `src/orchestrator/source.rs`. Define `Base` and `Workspace` and `MergeAxis` and `SourceState` and `Action` and `Step`. `classify` takes the two tips and the four trees and the recorded merge state and returns the state. `plan` takes the state and the action and the conflict model and returns ordered steps. Keep it free of I/O and locks and clocks.


| Field | Value |
|-------|-------|
| Success Criteria | A table test covers every combination of three base values and two workspace values and three merge values and five actions and two conflict models. The module imports only `crate::vcs::ConflictModel`. `cargo test -p agentkit-litterbox` passes. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On |  |
| References | source-state,tree-identity,the-plan,pure-and-total |

### T-002: [x] Contract additions

Extend `vcs` with `tree` and `sandbox_tip` and `set_sandbox_tip` and `advance` and `observe` and `is_ancestor` and `ensure_refs` and `report` and `stage` and `finish` and `discard_stage`. Add neutral `Report` and `Staged` and `Finished` in `vcs::reconcile`. Implement every method in the `vcs/mod.rs` stub and in `src/testing/vcs.rs`. Git keeps the conflict and history methods unsupported.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo check -p agentkit-litterbox` passes. The trait stays object safe and no Git type appears in a signature. The stub and `TestVcs` implement every method. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On |  |
| References | backend-writes |

### T-003: [x] Git refs and locality

Add `src/vcs/git/refs.rs` with `branch_ref` and `sandbox_ref` and `synced_ref` resolved through `repo_for_branch`. `prepare` creates all three at the base. `delete_branch` removes all three. `ensure_refs` initialises the two bookkeeping refs from the branch tip and logs it. `install_remote_in` adds the workspaces refspec. `open` sets `core.logAllRefUpdates=always` on the bare store.


| Field | Value |
|-------|-------|
| Success Criteria | A test creates a sandbox and asserts the three refs. A legacy record with no refs is initialised once and logged. Delete removes all three. Under remote a fetch of `refs/litterbox/workspaces/<name>` succeeds. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-002 |
| References | three-refs,creation-and-removal,legacy-record,reflog-keeps,missing-refspec |

### T-004: [x] Tree identity and snapshot split

In `src/vcs/git/scm.rs` implement `tree` and `observe` and `sandbox_tip` and `set_sandbox_tip` and `advance` and `set_tip` and `synced` and `set_synced` and `is_ancestor`. Change `snapshot` to commit on the sandbox branch only. Keep the compare-and-swap and the retry.


| Field | Value |
|-------|-------|
| Success Criteria | Tests cover a revision tree and an observed tree with no commit and the sandbox tip and the shared branch moving separately. The race test still passes. A snapshot under a held branch advances only the sandbox branch. |
| Complexity | 🔴 High |
| Effort | 1-1.5d |
| Depends On | T-002,T-003 |
| References | tree-identity,observing-the-workspace,snapshot,recorded-base,no-revert |

### T-005: [x] Record the merge state

Add `merge: Option<StoredMerge>` to `StoredSandbox` with `base` and `ours` and `theirs`. Use `serde(default)`. Keep the field across load and store.


| Field | Value |
|-------|-------|
| Success Criteria | A legacy record with no field loads as `None`. A round trip keeps the state. `cargo test -p agentkit-litterbox` passes. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | source-state,divergence,legacy-record |

### T-006: [x] The reconciler

Add `src/orchestrator/reconcile.rs`. `state` calls `ensure_refs` and reads `synced` and the branch tip and the sandbox tip and the record. `observe` downloads the workspace with the ignore selector and calls `Vcs::observe`. The pre-plan observes only when the branch moved or `Merge` is `Pending` or the action is `Snapshot` and passes `Clean` when it defers. `execute` runs `AdoptHistory` and `AdoptContent` and `Snapshot` and `Block` and `Proceed`. `AdoptContent` materialises the branch tip and uploads it and deletes the paths the new tree lacks and reports the old tip when it is not an ancestor. `Block` writes `Pending`. Clear `Pending` when the sandbox tip is an ancestor of the branch tip.


| Field | Value |
|-------|-------|
| Success Criteria | Orchestrator tests with `TestVcs` cover a squash and a content rewrite and a divergence and the `Pending` exit. Adoption keeps ignored files. The observation is skipped for a read on a synced sandbox. |
| Complexity | 🔴 High |
| Effort | 1.5-2d |
| Depends On | T-001,T-002,T-003,T-004,T-005 |
| References | lazy-planning,adopt-history,adopt-content,divergence,no-tool-refused,ignored-files-survive,empty-workspace,repeated-rewrites,snapshot-after-merge |

### T-007: [x] Wire the operations

In `src/orchestrator/service.rs` call the reconciler from `create` and `exec` and `upload` and `download` and `snapshot` and `delete`. `exec` and `upload` run the pre-plan and the post-plan. `download` runs the pre-plan only. `snapshot` runs one `Snapshot` plan. Remove the explicit snapshot from the MCP `write` tool. Return the post-plan state on `Performed`. `delete` removes any staging worktree.


| Field | Value |
|-------|-------|
| Success Criteria | A read on a synced sandbox makes no snapshot. A write snapshots once. A divergence leaves the branch and records `Pending`. `create` writes no merge state. The existing suite passes. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-006 |
| References | lazy-planning,no-tool-refused,backend-writes,creation-and-removal,branch-gone |

### T-008: Report and stage in Git

Add `src/vcs/git/reconcile.rs`. `report` runs `git merge-tree --write-tree --merge-base=<synced>` and parses the conflict paths and names the branch reflog. `stage` creates `data_dir()/reconcile/<slug>/<sandbox>` with `git worktree add` on a temporary branch then `read-tree -m` and `checkout --conflict=merge` then writes `MERGE_HEAD` and `MERGE_MSG`. `finish` checks a clean worktree and a descendant merge commit and moves the branch or prints the fast-forward. `discard_stage` removes the worktree and the temporary branch. Every command runs with `-c core.hooksPath` pointing at an empty directory.


| Field | Value |
|-------|-------|
| Success Criteria | A rebase makes the report use `synced` rather than the inferred base. Staging leaves the user's checkout unchanged and writes markers and index stages and `MERGE_HEAD`. A repository hook does not run. Finish fast-forwards and abort leaves the branch. A second stage refuses. |
| Complexity | 🔴 High |
| Effort | 1.5-2d |
| Depends On | T-003,T-004 |
| References | report,staging-worktree,workflow,recorded-base,no-hooks,second-stage,dirty-worktree,stale-worktree |

### T-009: Operator surface

Add the orchestrator wrappers and the CLI commands `status <name>` and `reconcile <name>` and `reconcile --finish <name>` and `reconcile --abort <name>`. Support `--json` on `status`. Print the worktree path and the conflicting paths.


| Field | Value |
|-------|-------|
| Success Criteria | `litterbox status` on a diverged sandbox names `synced` and both tips and the conflicting paths and the reflog command. `reconcile` prints the worktree path. `--finish` lands the merge or prints `git merge --ff-only`. `--abort` removes the worktree. Errors name the remedy. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-007,T-008 |
| References | report,workflow,dirty-checkout,consent |

### T-010: MCP tool

Add `sandbox-reconcile` to `src/mcp.rs` taking `{ sandbox }` and calling the orchestrator's stage. Return the worktree path and the conflict paths. No finish.


| Field | Value |
|-------|-------|
| Success Criteria | The tool appears in the router and its schema takes only the sandbox name. A test asserts the tool name list. The tool does not expose finish or abort. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On | T-009 |
| References | consent,staging-worktree |

### T-011: Verification

Run the gates and the smoke on a scratch repository under both `snapshot-mode` values.


| Field | Value |
|-------|-------|
| Success Criteria | The four gates pass. The smoke covers create and snapshot and rebase adoption and divergence and status and reconcile and finish and the next adoption. Under remote the refs live in the store and the fetch refspec carries the sandbox branch. The output is recorded beside the ADR. |
| Complexity | 🟢 Low |
| Effort | 3-4h |
| Depends On | T-009,T-010 |
| References | gates-pass,both-localities,staging-isolated,finish-lands,abort-leaves,squash-adopts-history,content-rewrite-adopted,divergence-holds,report-base,no-refusal,local,no-new-dependency |

