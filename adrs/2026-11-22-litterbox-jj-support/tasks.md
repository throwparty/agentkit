# jj as a Second VCS Backend

## Tasks

### T-001: The tool selector

Add `[vcs] tool` to the configuration and read it in `compose::open`. With no selector, `open` fails naming the file and both values; `git` builds `GitVcs` as today and `jj` routes to a placeholder `vcs::jj` module that returns `Unsupported`. The committed project file cannot set it (a `[vcs]` key there is ignored with a notice).


| Field | Value |
|-------|-------|
| Success Criteria | `cargo check -p agentkit-litterbox` passes. `open` with no selector errors naming `[vcs] tool`, `git` and `jj`. With `[vcs] tool = 'git'` every existing test passes. A committed `[vcs] tool` produces the ignore notice. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On |  |
| References | tool-selector |

### T-002: Type the `[vcs.jj]` section

Add `vcs::jj::config` with `JjVcsConfig { binding }`, `deny_unknown_fields`, and no `snapshot-mode`; a mode for jj is refused. Parse it only when jj is selected. `[vcs.git]` keeps its existing shape.


| Field | Value |
|-------|-------|
| Success Criteria | A `[vcs.jj]` section with `binding` parses; an unknown key or a `snapshot-mode` under `[vcs.jj]` is refused naming the section; the `[vcs.git]` tests are unchanged. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-001 |
| References | vcs-sections |

### T-003: Record the tool per sandbox

Add `StoredSandbox.vcs: Option<String>` with `serde(default)` and an accessor that reads an absent field as `git`. Create writes the selected tool; repair preserves the recorded value; reading never rewrites it.


| Field | Value |
|-------|-------|
| Success Criteria | A record without the field reads as `git` and round-trips without being rewritten; create writes the tool; repair keeps it. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | recorded-tool,legacy-record |

### T-004: Refuse a record this process cannot serve

In `orchestrator/service.rs`, refuse every source operation when a record's `vcs` differs from the selected tool, before any VCS call; `list` reports it with the reason. Make the binding misconfiguration message name the selected tool's section instead of hardcoding `[vcs.git]`.


| Field | Value |
|-------|-------|
| Success Criteria | A jj record under a Git process refuses with an error naming both tools and the file; `list` reports the mismatch; the binding message names `[vcs.jj]` when jj is selected. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-001,T-003 |
| References | tool-mismatch |

### T-005: The arch-lint boundary

Add the `litterbox-vcs-jj` scope (`vcs/jj/**`), add it to every deny list that names the Git scopes and to the `litterbox-vcs` deny, add it to `litterbox-compose`'s allow list, and add the `[[independent]]` rule over `litterbox-vcs-git` and `litterbox-vcs-jj`. Add a negative-script case for each direction.


| Field | Value |
|-------|-------|
| Success Criteria | `arch-lint check` passes; `bash hack/arch-lint-negative.sh` fails on each new case for the expected reason; the scope is non-empty because `vcs/jj/config.rs` exists. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-002 |
| References | boundaries,backends-apart |

### T-006: The clone

Add `vcs::jj::clone`: resolve `vcs/jj/<slug>`, create it with `jj git clone <user-repo> <dest>` when absent, lay out `workspaces/<name>` and `reconcile/<name>`, and hold a clone-level file lock that serialises snapshot, bookmark moves and staging.


| Field | Value |
|-------|-------|
| Success Criteria | `JjVcs` can create the clone from a scratch repository and add a workspace; a second create reuses it; the lock is exclusive across two handles. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-002 |
| References | jj-clone,clone-serialisation |

### T-007: The subprocess driver

Add `vcs::jj::ops`: run `jj` and `git`, probe and pin a minimum jj version, and read facts via `jj log --no-graph -T commit_id`, `git rev-parse <rev>^{tree}` and `jj resolve --list -r <rev>`. Map exit codes and stderr to `vcs::Error`.


| Field | Value |
|-------|-------|
| Success Criteria | A missing or too-old jj fails with an error naming the prerequisite; commit ids and tree ids parse from a scratch repo; a failing jj command maps to `vcs::Error::Backend` with its stderr. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-002 |
| References | git-view,local-and-lazy,missing-prerequisite |

### T-008: JjVcs: prepare, identity and snapshot

Implement the core of `JjVcs`: `capabilities` (Blocking, op_log true, Both, GitExport), `masked_paths` (`.jj`), `bound_root`, `identity`, `materialise` (git archive), `snapshot_filter` and `ignore_files` (git_common), `prepare`, `snapshot` (sync the copy into the workspace under `Copied`, then jj snapshots), `tip`/`set_tip`, `synced`/`set_synced`, `tree` (rev-parse), `sandbox_tip`/`set_sandbox_tip`, `advance` (move the bookmark), `observe`, `is_ancestor` and `ensure_refs`.


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests against a scratch repo cover create, snapshot under both bindings, adoption, the snapshot split and the compare-and-swap race; no Git type and no `vcs::git` import appears in `vcs/jj`. |
| Complexity | 🔴 High |
| Effort | 1.5-2d |
| Depends On | T-005,T-006,T-007 |
| References | three-revisions,prepare,snapshot,adoption-and-hold,masking,capabilities |

### T-009: JjVcs: conflicts, report, stage, finish, discard

Implement `conflicts` (`jj resolve --list -r <rev>`), `report`, `stage` (a workspace at `reconcile/<name>` running `jj new <synced>` + `jj restore --from <tip>` per side then `jj new <theirs'> <ours'>`), `finish` and `discard_stage`. `merge`, `resolve`, `abort`, `history` and `restore` stay `Unsupported`.


| Field | Value |
|-------|-------|
| Success Criteria | Staging produces a conflict set matching `git merge-tree --write-tree --merge-base=<synced>` and leaves the sandbox commit and bookmark unmoved; finish moves the bookmark; discard removes the workspace and duplicates; no `jj op restore` appears in the op log. |
| Complexity | 🔴 High |
| Effort | 1-1.5d |
| Depends On | T-008 |
| References | conflicted-tip-fallback,staging,finish-and-discard |

### T-010: JjVcs: removal and exposure

Implement `state_path` (the clone's `.git`), `expose` (git remote add in the user's checkout), `delete_branch` (forget the workspace, delete the bookmark), `prune` (stale workspaces and an empty clone) and `sandboxes` (the `litterbox/*` bookmarks, never `default`).


| Field | Value |
|-------|-------|
| Success Criteria | Deleting a sandbox removes its workspace and bookmark; deleting the last one prunes the clone; `sandboxes` never reports `default`; `expose` adds the clone as a remote the user's checkout can fetch. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-008 |
| References | removal,git-view |

### T-011: Hold a conflicted shared branch

In `orchestrator/reconcile.rs`, ask `Vcs::conflicts` for the shared branch during classification; a non-empty answer holds at `synced` and the sandbox branch without adopting, and the operation still runs. `Error::Unsupported` reads as no check; any other error fails.


| Field | Value |
|-------|-------|
| Success Criteria | A conflicted shared branch is held, tools still run, and no adoption happens; a Git process is unchanged because Git returns `Unsupported` for `conflicts`. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-009 |
| References | conflicted-tip-fallback |

### T-012: The workspace block

In `src/mcp.rs`, append a `workspace` block (`state`, `reason`, `synced`, `branch`, `sandbox`, `conflicts`, `next`) to every tool that operates on one sandbox, derived from the post-plan state. A conflicted tip reports `held` with reason `shared branch is conflicted`; a `Pending` divergence reports `held` with its own reason.


| Field | Value |
|-------|-------|
| Success Criteria | Every single-sandbox tool returns the block; a test asserts the block shape and the two `held` reasons. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-011 |
| References | workspace-block |

### T-013: Tests, gates and smoke

Add the remaining unit and orchestrator tests, then run the gates and the smoke: the jj selector end to end (create, snapshot, rewrite adoption, dirty hold, conflicted-tip hold, stage, resolve, finish, discard) and the Git selector under both `snapshot-mode` values.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo test -p agentkit-litterbox`, `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings`, `arch-lint check` and `bash hack/arch-lint-negative.sh` pass; the jj and Git smoke runs are recorded beside the ADR. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-004,T-008,T-009,T-010,T-011,T-012 |
| References | gates,git-unchanged |

