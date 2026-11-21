# Decouple VCS From Orchestration Tasks

## Tasks

### T-001: [x] Contract module

Add `src/vcs/{mod,types,capabilities,conflict,error}.rs`: `BranchName`, `Revision`, `TreeId`, `Workspace` with `from_branch`, `Prepared`, `Capabilities` with `ConflictModel`/`BindingSupport`/`ExportSupport`, `Merge`, `Conflict`, `ConflictSides`, `HistoryPolicy`, `RestoreMode`, and `Error` with `Exists`, `NotFound`, `Unsupported`, `Backend` and `Io`. Declare the object-safe synchronous trait with every method the plan lists. No implementation and no caller; no behaviour change.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo check -p agentkit-litterbox` passes; a unit test builds `Arc<dyn Vcs>` from a stub; a search of `src/vcs` for `git2` or `jj` returns nothing; the trait has no generics, no associated types and only `&self` receivers. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On |  |
| References | vcs-contract,object-safe,open-for-extension,unimplemented-operation,binding-is-configured,signature-types |

### T-002: [x] Git common module

Add `src/vcs/git_common/{ignore,filter,archive,unpack,naming}.rs`. Move `IgnoreFiles`, `SnapshotFilter`, `branch_name_for`, the archive builder and `git2::Tree` walk, and the tar unpack loop out of `scm` and the orchestrator, keeping the `SnapshotSelector` implementations and the path confinement. It may use `git2`. Make `scm` and the orchestrator delegate temporarily so behaviour is unchanged. Add no binding or masking code.


| Field | Value |
|-------|-------|
| Success Criteria | Existing `scm` and `vcs_store` tests still pass; `git_common` imports neither `vcs::git` nor `vcs::jj`; the orchestrator's create and snapshot tests are unchanged; `cargo test -p agentkit-litterbox` passes. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-001 |
| References | git-common,mechanical-move |

### T-003: [x] Move the Git backend

Move `scm/mod.rs`, `vcs_store/{mod,remote,path}.rs` and their tests under `src/vcs/git/`. Rename `GitScm`/`ThreadSafeScm` to `GitVcs`, keep the mutex and the branch-resolution rule (bare clone first, host fallback), and implement the trait. Add `GitVcsConfig { remote, project_slug, host }`. Move the `expose_store` and `store_path` bodies into `vcs::git`, leaving temporary orchestrator wrappers so the CLI still compiles. Delete `apply_patch` and `repo_prefix`.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo test -p agentkit-litterbox` passes; `scm` and `vcs_store` modules no longer exist; outside `src/vcs/git/`, `git2` is confined to `src/vcs/git_common/` and `domain::ScmError`, which T-004 removes; the moved Git tests pass unchanged. |
| Complexity | 🔴 High |
| Effort | 1-1.5d |
| Depends On | T-001,T-002 |
| References | git-backend,existing-operations,mechanical-move,git2-confinement |

### T-004: [x] Move the error boundary

Move `domain::ScmError` into `vcs::git_common` as `GitError`; add `SandboxError::Vcs { message }` and remove `SandboxError::Scm`. Convert `GitError` into `vcs::Error::Backend` preserving every display string, and implement the orchestrator's `From<vcs::Error> for SandboxError` mapping for `Exists`, `NotFound`, `Io` and the rest. Keep the `SCM error:` prefix so contract failure messages are byte-identical.


| Field | Value |
|-------|-------|
| Success Criteria | A search of `src/domain` for `git2` returns nothing; tests assert the previous message and class for a missing repository, a missing branch and an existing branch; `cargo test -p agentkit-litterbox` passes. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-003 |
| References | error-boundary,git2-confinement,behaviour-parity |

### T-005: [x] Recording TestVcs

Add `TestVcs` to `src/testing.rs` beside `TestBackend`. It implements every trait method, records `prepare`, `materialise`, `snapshot`, `delete_branch`, `expose` and `prune`, and returns configured `Prepared`, `Revision`, `sandboxes` and `tip` values. Keep it `#[cfg(test)]`.


| Field | Value |
|-------|-------|
| Success Criteria | The double implements `Vcs` completely; unit tests cover a recorded create sequence and a configured `NotFound`; `cargo test -p agentkit-litterbox` passes. |
| Complexity | 🟢 Low |
| Effort | 3-4h |
| Depends On | T-001 |
| References | object-safe,unimplemented-operation,moved-git-tests |

### T-006: [x] Rewire the orchestrator

Replace `Sandboxes.scm` with `vcs: Arc<dyn Vcs>` and `remote: bool`. Rewrite create, repair, snapshot, delete, list and failure cleanup through the trait; delete `with_branch_scm`, `snapshot_scm`, `stage`, `stage_reference`, `open_scm`, `scm()` and the temporary wrappers. Repair uses `tip` and maps `NotFound` to the existing Unavailable refusal; delete calls `prune` only for a remote record. Migrate the six orchestrator test sites to `TestVcs` and move Git assertions into `vcs::git` tests.


| Field | Value |
|-------|-------|
| Success Criteria | A search of `src/orchestrator` finds no `git2`, `ThreadSafeScm`, `VcsStore`, `ScmMode`, `crate::scm`, `crate::vcs::git` or `crate::vcs::git_common`; `Sandboxes` holds `Arc<dyn Vcs>`; `cargo test -p agentkit-litterbox` passes. |
| Complexity | 🔴 High |
| Effort | 1.5-2d |
| Depends On | T-003,T-004,T-005 |
| References | orchestrator-port,existing-operations,mode-and-record,orchestrator-neutral,behaviour-parity,moved-git-tests |

### T-007: [x] Compose and surfaces

Add `src/compose.rs` with `open(config)`: compute the project slug, read `config.git.snapshot_mode` (default Remote), build `GitVcsConfig`, construct `GitVcs`, and return `Sandboxes::with_vcs(config, Arc::new(git), remote)`. Point `mcp.rs` and `cli.rs::project` at it. Replace the free-function calls in `handle_remote` with `Sandboxes::expose_remote`. Delete `Sandboxes::open` and the temporary `expose_store` and `store_path` wrappers.


| Field | Value |
|-------|-------|
| Success Criteria | `cli.rs` and `mcp.rs` call `compose::open`; neither imports `vcs::git` or `vcs::git_common`; `remote` still prints the store path and the `git fetch` line; `cargo test -p agentkit-litterbox` passes. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-006 |
| References | composition-root,single-construction,binding-is-configured,mode-and-record |

### T-008: [x] Arch-lint boundary

Add scopes `litterbox-vcs`, `litterbox-vcs-git-common`, `litterbox-vcs-git` and `litterbox-compose`; remove `litterbox-scm` and `litterbox-vcs-store` and replace them in every deny list. Add allow-lists: `vcs` to contract; `git-common` to contract and vcs; `git` to vcs, git-common and contract; `compose` to vcs, git, orchestrator, config, shared and contract. Add the deny rules for the orchestrator, git-common, CLI and MCP, vcs and the contract-facing modules. `litterbox-vcs` excludes the two subdirectories.


| Field | Value |
|-------|-------|
| Success Criteria | `arch-lint check` passes on the new layout; planting `use crate::vcs::git::GitVcs as _;` in `orchestrator` fails; `litterbox-scm` and `litterbox-vcs-store` appear nowhere in `arch-lint.toml`. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-007 |
| References | arch-scopes,arch-rules,open-for-extension,empty-jj-scope,inline-path |

### T-009: [x] Negative fixture

Update `hack/arch-lint-negative.sh`: remove the `crate::scm` plant and add plants for orchestrator to `vcs::git` and `vcs::git_common`, `vcs::git_common` to `vcs::git`, CLI and MCP to `vcs::git`, MCP to `vcs::git_common`, and one backend to `vcs::git`. Expect the named rule in each failure and keep the restore trap.


| Field | Value |
|-------|-------|
| Success Criteria | `bash hack/arch-lint-negative.sh` exits 0; every plant is named in its output; the working tree is unchanged after the run. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-008 |
| References | arch-negative,fixture-bites |

### T-010: [x] Verification

Run the full gate and a parity smoke on a scratch repository.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo test -p agentkit-litterbox`, `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings`, `arch-lint check` and `bash hack/arch-lint-negative.sh` all pass. With `snapshot-mode` set to direct and then remote: create, exec, snapshot, list, delete, and `remote <name>` followed by the printed `git fetch`. The commands and their output are recorded in `adrs/2026-11-19-decouple-vcs-from-orchestration/evidence.md`. The orchestrator, CLI and MCP contain no `git2`; within the VCS tree `git2` appears only under `git/` and `git_common/`. |
| Complexity | 🟢 Low |
| Effort | 3-4h |
| Depends On | T-009 |
| References | suite-passes,lints-pass,parity-proven,checked-in-ci,behaviour-parity |

