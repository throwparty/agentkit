# Sandbox Lifecycle Consistency and Recovery Tasks

## Tasks

### T-001: [x] Add paused, create-mode and teardown contract types

Add `Error::Paused { name }` with a message naming `litterbox resume <name>` and map it to `FailureClass::Conflict`. Add `CreateMode { Fresh, ReuseExisting }` to `CreateRequest` and set `Fresh` in every existing constructor and fixture. Add `Teardown { compute_stopped, state_paths, detail }` and change `Backend::delete` to return it. Update Docker and TestBackend to compile with the new signatures, and update `sandbox/mapping.rs` documentation so conflict covers a taken name and a paused state.


| Field | Value |
|-------|-------|
| Success Criteria | The workspace builds. A paused error reports exit class 4 and MCP surface class invalid params. Every `CreateRequest` constructor names a mode. Every `Backend` implementation returns `Teardown` from delete. Existing tests compile and the contract tests for class mapping pass. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On |  |
| References | paused-class-parity,neutral-failure-classes,backward-compatibility |

### T-002: [x] Add the version-tolerant pid probe

Add a small `RecordedPids` value in `payload.rs` that reads only the launcher and sandbox pids and their start times, whatever the payload version. Change `validate` to probe those first: a gone pid is `Absent`; a live pid with an unsupported or unreadable payload is `Unavailable`, not `Missing`; a payload with no readable pid is `Unavailable`. Do not let the probe feed a join, a signal or a cgroup write.


| Field | Value |
|-------|-------|
| Success Criteria | A version 1 payload with a dead pid validates as Absent and inspect reports Missing. A version 1 payload with a live pid validates as Unavailable. A payload with no readable pid validates as Unavailable. A version 2 payload takes the existing full-validation path unchanged. Unit tests cover all four cases. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-001 |
| References | no-payload-dependence,unreadable-payload-with-workspace,state-directory-corrupt |

### T-003: [x] Add orchestrator outcome and inspection types

Add `orchestrator/outcome.rs` with `Recovery`, `Performed<T>`, `InspectionReport` and `Deletion`. `InspectionReport` carries either an `Inspection` or a status-only report whose status is `SandboxStatus::Unknown` or `SandboxStatus::Error` with a reason. Change `create`, `exec`, `upload`, `download` and `snapshot` to return `Performed<T>`, and update `SandboxIo` and every call site so the crate compiles.


| Field | Value |
|-------|-------|
| Success Criteria | A successful operation returns `Performed` with no recovery. A recovered operation returns exactly one `Recovery`. `InspectionReport` can carry Unknown and Error and is serialisable where MCP needs it. `cargo check -p agentkit-litterbox` passes. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-001 |
| References | recovery-warning,unknown-status,neutral-failure-classes |

### T-004: [x] Serialise repair per sandbox

Add `orchestrator/lock.rs` with a per-project-per-name guard. It creates `<records_dir>/.locks`, takes an in-process async guard keyed by name, then takes `flock(LOCK_EX)` on `<name>.lock` in a blocking task. Hold the guard from the status re-check through the record write. Add a non-unix fallback that uses the in-process guard alone.


| Field | Value |
|-------|-------|
| Success Criteria | Two concurrent calls for the same missing sandbox result in one repair and one observation of active. A lock is released on normal return and when the task is dropped. The lock directory is ignored by record enumeration. A unit test exercises contention at the lock seam. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-001 |
| References | repair-serialised,atomic-repair,no-new-dependency,concurrent-calls,concurrent-repair-single-holder |

### T-005: [x] Split create and add the ensure-live gate

Split `create` into branch creation, provisioning resolution and compute creation. Add `Sandboxes::ensure_live(name, policy)` that loads the record, selects the owning backend, inspects, and returns active, refuses paused when the policy says so, repairs missing, or returns the backend failure. Repair takes the lock, re-inspects, confirms the branch through SCM, resolves provisioning, stages the sandbox branch as the fallback source, and calls create with `ReuseExisting`. Add the failure paths for a missing branch, a record without a branch, a backend that cannot be asked and a preparation refusal.


| Field | Value |
|-------|-------|
| Success Criteria | Create on an active or paused name returns conflict. Create on an orphan branch returns conflict and does not adopt it. A missing sandbox with a live branch is repaired through `ReuseExisting`. A missing branch returns unavailable with the delete-then-create remedy. A preparation refusal returns its structured provisioning failure. No repair creates or deletes a branch. |
| Complexity | 🔴 High |
| Effort | 8-12h |
| Depends On | T-001,T-002,T-003,T-004 |
| References | ensure-live-or-repair,repair-is-create,repair-preserves-workspace,repair-failure,branch-missing,record-without-branch,preparation-refuses-during-repair |

### T-006: [x] Implement bubblewrap reuse-mode create

Teach `BubblewrapBackend::create` about `CreateMode`. `Fresh` keeps the current conflict and copy-from-source path. `ReuseExisting` checks liveness through the pid probe: absent proceeds, live is conflict, no readable pid is unavailable with the delete remedy. Reuse an existing workspace; otherwise create it from the request source and mark it as materialised. Build the view and environment from the resolved request, start the holder, wait for readiness, apply limits, run sandbox steps only when the workspace was materialised, overwrite stale status and diagnostic files, and write the payload atomically after readiness. On failure tear down the holder and remove only a workspace this attempt created.


| Field | Value |
|-------|-------|
| Success Criteria | A dead v1 record with an existing workspace repairs in place and preserves workspace files. A missing workspace is materialised from the branch and the sandbox steps run. A reused workspace does not run the sandbox steps. A failed readiness or step run leaves the old payload and workspace and removes only the new holder. No repair reads the full v1 payload. |
| Complexity | 🔴 High |
| Effort | 10-14h |
| Depends On | T-002,T-005 |
| References | repair-preserves-workspace,repair-rebuilds-environment,workspace-missing,setup-after-workspace-rebuild,stale-state-files,repair-spawn-fails,unreadable-payload-with-workspace |

### T-007: [x] Report teardown facts from backend delete

Return `Teardown` from bubblewrap delete with the state directory and whether compute was stopped. A readable payload tears down a live sandbox and reports true. An unreadable or unsupported payload reports false with the reason and still removes the state directory. Adapt Docker and TestBackend to return their own teardown facts without changing their delete behaviour.


| Field | Value |
|-------|-------|
| Success Criteria | Delete returns the state paths bubblewrap removed. An unreadable payload returns compute_stopped false and the reason. Docker and TestBackend compile with the new return type. Existing delete idempotency tests pass. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-001 |
| References | delete-corrupt-record,corrupt-record-removable,unreadable-payload-delete-force |

### T-008: [x] Refuse paused commands before the join

In `Sandboxes::exec`, inspect before the join. A paused sandbox returns `Error::Paused` and never reaches the backend exec. Upload, download and snapshot use the allow policy and continue because they do not join. Pause, resume, list and forwards keep their paths. Add a test that a paused exec performs no join and returns immediately.


| Field | Value |
|-------|-------|
| Success Criteria | A paused shell returns exit 4 and the resume command. A paused MCP command tool returns class conflict with the reason and remedy. Upload, download and snapshot still work on a paused sandbox. No backend exec is called for a paused sandbox. |
| Complexity | 🟢 Low |
| Effort | 2-4h |
| Depends On | T-001,T-003 |
| References | paused-refusal,paused-returns-immediately,paused-host-side-operations,repair-while-paused |

### T-009: [x] Enumerate records first and report unknown

Add `metadata_store::entries_in` that returns every `.toml` record, including corrupt and unreadable files. Change `Sandboxes::list` to fill current and corrupt records first, sort by name, then enumerate branches and add only names with no record as orphans. Record a branch enumeration failure instead of dropping records. Change `inspect_listing` to return `InspectionReport`: a backend selection failure is Unknown for every record, a per-record refusal is Error, and only a successful inspect is Inspected.


| Field | Value |
|-------|-------|
| Success Criteria | A record without a branch appears in list. A branch without a record still appears as an orphan. A corrupt record is not reported as an orphan. A backend that cannot be selected reports every record as unknown and exits 1. A per-record refusal prints error, not unknown. Missing remains exit 0. |
| Complexity | 🟡 Medium |
| Effort | 5-7h |
| Depends On | T-003 |
| References | unknown-status,list-records-and-branches,list-shows-unpaired,list-without-backend,record-without-branch,orphan-branch,backend-unavailable |

### T-010: [x] Implement delete confirmation and corrupt removal

Move the delete policy into `Sandboxes::delete(name, force)`. Inspect a current record: active, missing, unknown or error without force returns Denied with the record, workspace and branch names and the unmerged-work warning; paused keeps its current path. With force, delete compute, branch and record and return `Deletion`. A corrupt or unreadable record with force skips payload parsing, removes the record file and deterministic branch, calls the backend best-effort, and reports compute it could not stop.


| Field | Value |
|-------|-------|
| Success Criteria | Delete on missing without force exits 6 and names the record, workspace and branch. With force it removes them and prints what it removed. A corrupt record is removed with force and reports that compute could not be stopped. An unreadable payload delete without force is refused. A second delete succeeds. Paused delete keeps its current behaviour. |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On | T-003,T-007 |
| References | delete-missing-force,delete-corrupt-record,delete-missing-needs-force,corrupt-record-removable,corrupt-delete-without-force,unreadable-payload-delete-force,paused-delete |

### T-011: [x] Update the CLI surface

Pass `force` to `Sandboxes::delete` and print the `Deletion` fields. Render the record-first listing and the three inspection reports. Print the recovery warning to stderr before normal output. Update the `delete --force` help to cover active, missing and undetermined records and the unmerged-work risk. Keep every existing command and exit class except the documented delete confirmation.


| Field | Value |
|-------|-------|
| Success Criteria | `litterbox list` prints active, paused, missing, unknown, error, corrupt and orphan entries correctly. `litterbox shell` on a missing sandbox prints the warning and runs. `litterbox shell` on a paused sandbox exits 4. `litterbox delete` without force refuses missing and undetermined records with exit 6. No command is added. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-008,T-009,T-010 |
| References | paused-refusal,paused-class-parity,list-shows-unpaired,delete-missing-force,recovery-warning,no-new-surface,no-new-tool-or-verb |

### T-012: [x] Add MCP recovery blocks and paused error data

Add a helper that appends `Recovery` as a second `ContentBlock::json` block. Make the command helpers return `Performed<T>` so a repair inside read, write, patch, ls, glob, grep or bash is not lost. Append the recovery block to those tools and to `sandbox-create` when the orchestrator reports it. Add `reason` and `remedy` to the paused error data. Keep the tool list and argument names unchanged.


| Field | Value |
|-------|-------|
| Success Criteria | A recovered bash, read, write, patch, ls, glob and grep result carries the normal content plus one recovery block. `sandbox-ports` never carries one. A paused command returns invalid params with class conflict, reason and remedy. The tool names and argument test still passes unchanged. |
| Complexity | 🟡 Medium |
| Effort | 6-8h |
| Depends On | T-003,T-008 |
| References | paused-class-parity,recovery-warning,cli-mcp-class-parity,no-new-surface,no-new-tool-or-verb |

### T-013: [x] Wire create-on-missing through MCP

Make `sandbox-create` return `Performed<SandboxRecord>`. On a missing name it follows the repair path, returns the live inspection and carries the recovery block. On an active or paused name it returns conflict. On an orphan branch it returns conflict without adopting the branch.


| Field | Value |
|-------|-------|
| Success Criteria | `sandbox-create` on a missing name returns a live inspection plus a recovery block naming the branch and prior state. It does not create a second branch or record. Active, paused and orphan names return conflict. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-005,T-012 |
| References | create-on-missing,create-reports-recovery,create-active-or-paused,orphan-branch |

### T-014: [x] Adapt Docker and TestBackend

Docker reports `ReuseExisting` as Unsupported with the delete-then-create remedy and returns `Teardown` from delete. TestBackend gains a missing state, records the `CreateMode` it received, counts exec calls, supports resurrection through `ReuseExisting`, and returns `Teardown`. Update the conformance suite and every fixture for the new signatures.


| Field | Value |
|-------|-------|
| Success Criteria | The conformance suite passes against TestBackend. Docker's existing tests pass unchanged. A missing TestBackend record repairs only through `ReuseExisting`. Docker on a missing name returns unsupported with the delete remedy, not a panic or a silent create. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-001,T-003,T-007 |
| References | backward-compatibility,repair-failure,backend-unavailable |

### T-015: [x] Build the lifecycle matrix and incident regression tests

Add table-driven tests for every cell of the CLI and MCP matrices: active, paused, missing, corrupt and orphan records against list, shell, pause, resume, delete, the command tools, sandbox-ports and sandbox-create. Add the incident fixture: a current record, a live branch, a state directory with a workspace, dead pids and a v1 payload. Add tests for a branch that is gone, a record without a branch, a preparation refusal, a stale status file, an unreadable-payload delete without force, and two concurrent repairs. Add an ignored host test that creates a real bubblewrap sandbox, kills it, runs a command, and checks the workspace survived.


| Field | Value |
|-------|-------|
| Success Criteria | Every matrix cell has a named test. The v1 incident repairs and reports recovery with the workspace preserved. The branch-gone case returns the delete-then-create remedy. Concurrent repairs spawn one sandbox. The ignored host test passes where bubblewrap is available and names the missing requirement where it is not. |
| Complexity | 🔴 High |
| Effort | 10-14h |
| Depends On | T-005,T-006,T-008,T-009,T-010,T-013,T-014 |
| References | test-matrix,matrix-tests,workspace-tests-recovers,concurrent-repair-single-holder,paused-returns-immediately,list-shows-unpaired,unreadable-payload-with-workspace,workspace-missing,branch-missing,state-directory-corrupt,backend-unavailable,concurrent-calls,repair-spawn-fails,orphan-branch,record-without-branch,paused-host-side-operations,paused-delete,create-active-or-paused,corrupt-delete-without-force,preparation-refuses-during-repair,list-without-backend,repair-while-paused,stale-state-files,setup-after-workspace-rebuild,unreadable-payload-delete-force |

### T-016: [x] Update lifecycle docs and regenerate references

Update `lifecycle.mdx`: the state diagram, the CLI and MCP matrices, the failure-class table, the delete confirmation, corrupt removal and record-first listing. Mirror the operation tables and recovery prose in `src/docgen.rs`. Regenerate `reference/cli.mdx` and `reference/mcp.mdx` with the existing docgen commands. Add a docgen test that asserts the new rows and prose are present.


| Field | Value |
|-------|-------|
| Success Criteria | The lifecycle page states that missing repairs, paused refuses with conflict, delete on missing or undetermined needs force, corrupt records are removable with force, and forwards and sandbox-ports are record-only. Both generated reference pages contain the same tables and prose. The docgen test fails if a row or state is removed. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-011,T-012,T-013 |
| References | lifecycle-documentation,generated-documentation,reference-regenerated |

### T-017: [x] Run final verification

Run `cargo test -p agentkit-litterbox` and `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings`. Check the MCP tool list, the CLI command list and the generated reference pages for drift. Check that every spec slug appears in at least one task's refs.


| Field | Value |
|-------|-------|
| Success Criteria | Tests and clippy pass. The MCP tool list is unchanged. No command is added. The generated pages match a fresh docgen run. Every FR, NFR, AC and EC slug is referenced by at least one task. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-015,T-016 |
| References | verification,tests-and-clippy,no-new-tool-or-verb |

