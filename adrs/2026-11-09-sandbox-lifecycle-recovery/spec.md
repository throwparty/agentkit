# Sandbox Lifecycle Consistency and Recovery

**Status:** implemented  **Created:** 2026-11-09  **Author:** adrian

Litterbox pairs a name, a Git branch and a unit of compute. The branch is durable, the compute is not, and today nothing reconciles the two. The contract already declares `SandboxStatus::Unknown` but never constructs it (`crates/agentkit-litterbox/src/sandbox/contract.rs`), and the provider contract's FR-014 requires enumeration to report a sandbox as unknown when the backend cannot be asked (`adrs/2026-11-03-sandbox-provider-contract/spec.md`). The lifecycle page documents a `missing` state that only `delete` can leave (`docs/docs/user/litterbox/lifecycle.mdx`), and the generated reference repeats that prose from `crates/agentkit-litterbox/src/docgen.rs`. The orchestrator's `create` creates the branch before asking the backend (`src/orchestrator/service.rs`), so an existing branch returns `SandboxExists`, mapped to `Conflict`, and the backend is never reached; a missing name therefore cannot be recreated through either surface. `exec` performs no paused check before the join, so a command addressed to a paused sandbox can block until resume; a timeout inside the sandbox cannot fire while it is frozen. `list` enumerates branches rather than records, so a record whose branch is gone is invisible and a branch with no record is reported as an orphan. `stored()` returns a failure for a corrupt record and `delete` tolerates only `NotFound`, so a corrupt record cannot be removed by any command. The incident that motivates this specification is the `workspace-tests` sandbox under `data_dir/litterbox/bubblewrap/agentkit-02853408/`: the record, branch and workspace exist, both recorded pids are gone, and the payload is version 1 while the current release writes version 2. A repair that depends on reading that payload therefore cannot recover it. The existing ADR `2026-10-31-linux-bwrap-lifecycle` covers cgroup pause and resume, not lifecycle reconciliation; `2026-11-03-sandbox-provider-contract` covers status vocabulary, not recovery; `2026-11-07-linux-bwrap-provider` covers validation outcomes, not reconciliation. No existing ADR covers this topic.

**Amendment (2026-11-24).** Since this ADR was implemented, `adrs/2026-11-22-adopt-rewritten-branches` split the single branch into a shared branch, `litterbox/<name>`, and a sandbox branch, `refs/litterbox/workspaces/<name>`. The lifecycle requirements still describe the pre-split model. Repair now materialises from `tip`, the shared branch, which throws away the sandbox's own work when the branch advance is held. The operation matrix also has no state for a held advance or an open resolution, and `delete --force` conflates three independent guards. This amendment changes FR-003, FR-006, FR-008, FR-012–FR-016 and NFR-005/NFR-006, and adds FR-017–FR-018. It renames the model-facing state `held` to `diverged`. Tasks T-018–T-026 implement this amendment; the specification is implemented.


## Problem

A sandbox whose compute has exited is unrecoverable. Every command returns not found, inspect reports missing, delete succeeds, and no surface can bring the compute back. Creating the same name fails as a conflict because the branch exists. A paused sandbox blocks commands instead of refusing them. `Unknown` is declared but unreachable. `list` hides records whose branch is gone and reports unpaired branches as orphans. A corrupt record cannot be deleted. The system has no single rule for what happens when a record and its compute disagree.

## Goals

- Make every operation that needs live compute reconcile a record whose compute is missing, using the same name, branches and workspace.
- Preserve the workspace directory across a repair; use the sandbox branch only when the workspace is gone, and never replace the sandbox's work with the shared branch.
- Treat a held branch advance and an open resolution as source states that refuse no tool, while `pause`, `resume`, `list`, `sandbox-status` and delete still answer.
- Name each delete guard — running compute, undetermined compute, and unmerged work — and the flag that overrides it; state exactly what will be removed.
- Refuse a paused sandbox before any join, with one neutral failure class on CLI and MCP.
- Make `SandboxStatus::Unknown` reachable when the backend cannot be asked, and stop reporting per-record refusals as unknown.
- Report records and branches in one listing so a record without a branch is visible and addressable.
- Give a corrupt record a removal path that needs no parseable payload and report any compute it could not stop.
- Never repair silently: the CLI warns on stderr and MCP carries a structured recovery field.
- Enforce every cell of the lifecycle operation matrix with a test.

## Non-goals

- A new user-facing recovery verb, command or MCP tool.
- An MCP delete, pause, resume, list or inspect tool.
- Deleting, rewriting or creating a branch anywhere except the existing create and delete paths. Repair never recreates the sandbox branch and never rewrites the shared branch.
- Repairing a record whose shared branch and sandbox branch are both gone, or adopting an orphan branch that has no record.
- A `--force` alias: the three delete guards are named individually, and no alias overrides more than one.
- A new source-state verb or a second resolution workflow; `sandbox-reconcile` keeps its three actions.
- A standalone payload migration, a payload-version compatibility layer, or reading payload version 1; repair writes a current payload only when it recreates compute.
- Re-running sandbox provisioning steps when the workspace is reused; they are re-run only when the workspace had to be materialised from the sandbox branch.
- Replacing the branches as the durable halves, or making compute durable.
- Changing Docker's behaviour or adding a new backend.
- Egress posture, host networking or port-forwarding semantics; this ADR only fixes whether forwards and sandbox-ports repair or block.


## Functional Requirements

### FR-001: Ensure Live Or Repair

Every orchestrator operation that needs live compute first establishes whether the sandbox is live. The covered operations are exec, upload, download and snapshot. `forwards` and `sandbox-ports` answer from the record and never trigger repair. Active proceeds. Paused follows FR-008. Missing triggers repair through FR-002 to FR-006. A backend that cannot be asked or a record that cannot be read returns a neutral failure with a remedy, without attempting to repair. The step is idempotent: a second call on an active sandbox does not repair, and a second call after a successful repair sees active and proceeds.

The gate decides compute only. A sandbox whose shared-branch advance is held is `diverged`, and one with an open resolution is `resolving`; both are live and the gate neither refuses nor repairs them (FR-017).

**Slug:** `ensure-live-or-repair`

### FR-002: Repair Is Create

Repair uses the existing create path for the same name, project, branches and workspace. It is not a new lifecycle verb, command or MCP tool. It does not call `scm.create_branch`, does not create the sandbox branch, and does not delete or rewrite either branch. The orchestrator resolves the provisioning sequence from current project configuration, runs host preparation, and hands the backend a create request. The backend creates compute against the surviving state rather than copying over it. The repaired record copies the stored `merge` and `resolution` fields unchanged; repair never clears a held merge or an open resolution (FR-018). `sandbox-create` on a missing name follows this path. `sandbox-create` on an active or paused name remains Conflict, including when it is `diverged` or `resolving`. `sandbox-create` on a branch with no record remains Conflict and does not adopt the branch.

**Slug:** `repair-is-create`

### FR-003: Repair Preserves Workspace And Sandbox Branch

The backend locates its state directory by project and sandbox name, which is deterministic. If the workspace directory exists it is reused in place. If it does not exist the workspace is materialised from the **sandbox branch**, `refs/litterbox/workspaces/<name>`, not from HEAD and not from the shared branch `litterbox/<name>`. `<name>` is the sandbox name as it appears in the shared branch. `refs/litterbox/synced/<name>` records the revision both sides last agreed on.

The sandbox branch is the sandbox's own work: the agent's snapshots and, while a resolution is open, the revision under resolution. The shared branch is the user's branch. When the branch advance is held — a `Pending` merge or a `diverged` sandbox — the shared branch stays at the user tip and the sandbox branch carries the agent's work, so materialising from the shared tip throws that work away.

Repair reads `sandbox_tip` first. It uses that revision whenever the sandbox branch exists. When it differs from `tip` it is the only revision that preserves the sandbox's work; when it equals `tip` both name the same revision. When the sandbox branch is absent, repair falls back to `tip` and reports the fallback in the recovery outcome from FR-014, naming `refs/litterbox/workspaces/<name>` as the missing ref and the VCS recovery: Git `fsck --lost-found` and `update-ref`, jj `op log` and `workspace add`. The one exception is an open resolution: the sandbox branch is then the only addressable copy of the revision under resolution, so repair refuses instead of falling back (FR-006, FR-018). When both are absent, repair refuses with the unavailable remedy from FR-006.

The repair does not require the full payload to decide whether it can proceed; it reads only the recorded pids to prove compute is absent, so a version 1 payload does not block it. A payload with no readable pid cannot be proven absent and is refused with the delete remedy. If the repair fails it leaves the workspace, the record and the previous payload in their pre-repair state and removes only resources it created during the attempt. Stale state from the dead compute is not reused. A failed repair must not leave a half-repaired state directory.

**Slug:** `repair-preserves-workspace`

### FR-004: Repair Rebuilds Environment

Host preparation is re-run so environment values that are not persisted can be reconstructed. The resolved steps are passed to the backend, which rebuilds the sandbox's mounts and environment and writes a fresh payload in the current version. Stale status, diagnostic and payload files from the dead compute are replaced; the new payload is written only after the sandbox process is ready, so a failed attempt leaves the previous payload in place. Sandbox provisioning steps are re-run only when the workspace had to be materialised from the sandbox branch, because the branch does not contain setup effects that were never snapshotted; they are not re-run when the workspace is reused. A preparation refusal such as missing setup consent or an unbuildable devShell is reported as the structured provisioning failure it is, and no sandbox process starts.

**Slug:** `repair-rebuilds-environment`

### FR-005: Repair Is Serialised

The check-repair-write sequence is serialised per project and sandbox name across processes. The lock is a cross-process advisory lock on a file beneath the project records directory, taken before the status check and released after the record is written. An in-process guard covers the same name. A second caller blocks until the first finishes, then observes the repaired sandbox as active and proceeds without spawning a second sandbox process.

**Slug:** `repair-serialised`

### FR-006: Repair Failure

Repair is refused when the shared branch is gone and the sandbox branch is gone too, the state directory cannot be used, the record cannot be read, the selected backend is unavailable, or host preparation refuses. The failure uses a neutral class and a remedy that names the exact next command or tool call. Neither ref is recreated. A corrupt record is not guessed at. A repair that cannot preserve an open resolution refuses rather than clearing it, because the resolution revision is not addressable (FR-018). That refusal is a continuation, not a dead end: it names the recorded `synced` and `ours`, the VCS store and the recovery from FR-018.

The remedy for a record whose durable refs are gone is `litterbox delete <name>` with the flags the refusal names, followed by a create. The sandbox branch being absent on its own is not a failure: repair falls back to the shared branch and reports it (FR-003, FR-014). A failed spawn is `unavailable` or `failed` according to the backend's answer, and it leaves no half-repaired directory.

**Slug:** `repair-failure`

### FR-007: Create On Missing

`sandbox-create` on a missing name reconciles and reports that it did. It returns the same inspection it returns for a fresh create, and the MCP result carries the recovery field from FR-014. It does not silently create a second branch or a second record. The result names the shared branch, the source the workspace was materialised from, and the state it repaired from.

**Slug:** `create-on-missing`

### FR-008: Paused Refusal

A command addressed to a paused sandbox is refused before any namespace is opened or joined. The covered paths are `shell` on the CLI and `bash`, `read`, `write`, `patch`, `ls`, `glob` and `grep` over MCP, and the `sandbox-reconcile` actions `begin`, `finish` and `abandon`, which join the workspace to materialise or inspect the tree under resolution. No command is written into the sandbox and no caller blocks. The CLI exits 4 and prints `sandbox '<name>' is paused; run `litterbox resume <name>` first`. MCP returns invalid params with class `conflict`, the same reason and the same remedy.

`sandbox-status` does not join: it reads the record and the refs, answers while the sandbox is paused, and reports the source state and any open resolution. The other operations that do not join continue: `list`, `sandbox-ports`, `pause`, `resume`, `delete`, upload, download and snapshot. Pausing an already paused sandbox and resuming an already active one remain no-ops.

**Slug:** `paused-refusal`

### FR-009: Paused Class Is Shared

The paused refusal is one contract error variant mapped once to a failure class. The CLI exit code and the MCP class are derived from that mapping, not written per surface. The chosen class is `conflict`, because the sandbox exists and the request conflicts with its current state; retrying after resume succeeds. `conflict` continues to mean a name already in use and gains the state-conflict meaning in the documentation. The same variant covers a command tool and the three `sandbox-reconcile` actions. No surface reports a paused refusal as `failed` or `not-found`.

**Slug:** `paused-class-parity`

### FR-010: Unknown Is Reachable

`SandboxStatus::Unknown` is produced when enumeration cannot ask the backend: the backend cannot be selected, its readiness check fails, or it cannot be reached. The listing uses a status-only report and does not fabricate an `Inspection` with values it does not have. A per-record refusal from a backend that was asked is reported as `error` with the reason, not as `unknown`. `list` prints `unknown` only for the first case. `list` exits 1 when any record is unknown or in error, and exits 0 when the only non-live records are missing. A single-sandbox command that cannot ask the backend still returns the backend's failure rather than `Unknown`, because the command did not receive an answer.

**Slug:** `unknown-status`

### FR-011: List Records And Branches

`list` enumerates metadata records first and branches second. Every record is reported, including a record whose shared branch is gone; such a record remains addressable and its repair refuses with the remedy from FR-006 only when the sandbox branch is gone too. Every branch not represented by a current or corrupt record is still reported as an orphan. A record paired with an orphan branch is reported once. Output is sorted by name for stable tests. A record whose compute is active is reported even if its shared branch is gone. The listing is a compute report; the source state and an open resolution are reported by `sandbox-status` and by the `workspace` block (§FR-017).

**Slug:** `list-records-and-branches`

### FR-012: Delete Guards Are Per Condition

`delete` has three independent guards. Each holds for a different reason and each has its own flag. The command refuses and names every flag the record needs; one flag never satisfies another.

- **Running** — the compute is `active`. Flag `--ignore-running`. This is permission to stop it.
- **Invalid** — the compute cannot be determined: the status is `unknown` or `error`, or the record is corrupt or unreadable. Flag `--ignore-invalid`. Compute may be running and cannot be identified.
- **Unmerged** — durable work may be lost: the sandbox branch tip is not an ancestor of the shared branch tip, the shared ref is absent while the sandbox ref exists, an open resolution is recorded, or a staging workspace exists. Flag `--ignore-unmerged`.

`--force` is removed because it conflated the three. A `paused` or `missing` sandbox needs no compute flag, because nothing is running. A flag whose condition does not hold is accepted and reported as unused, so a script can pass a fixed set.

With every required flag, `delete` stops compute when it can, removes the backend state, the staging workspace, the shared branch, the sandbox branch and the synced revision, and removes the record. It prints each path and ref it removed and says when compute could not be stopped. `delete` on a paused sandbox keeps its current teardown behaviour. A retried delete still succeeds when both halves are already gone.

**Slug:** `delete-missing-force`

### FR-013: Delete Corrupt Record

`delete` on a corrupt or unreadable record requires `--ignore-invalid`, because compute may be running and cannot be identified. Without it the command refuses and says compute may not have been stopped. With it, delete skips payload parsing and removes the record file, the backend state directory best-effort, the shared branch, the sandbox branch and the synced revision. If compute cannot be stopped, the output says so and names the sandbox. `--ignore-unmerged` is required as well when the sandbox branch is addressable and holds work; `--ignore-running` is never required for a record whose status cannot be read. A corrupt record is never repaired or adopted.

**Slug:** `delete-corrupt-record`

### FR-014: Recovery Is Never Silent

Every repair names the sandbox and says it was recreated. The CLI writes a warning to stderr before the command's normal output. MCP appends a structured JSON content block to the tool result with the sandbox name, the state it came from, the action taken, the shared branch, the source the workspace was materialised from (`sandbox` or `shared`) and, when the sandbox branch was absent, a `fallback` string naming `refs/litterbox/workspaces/<name>`; it also writes a log line. The existing content of the tool result is unchanged. `sandbox-create` on a missing name carries the same field. A repair that fails carries the failure, not a recovery field.

The fields are additive. A caller that ignores them sees exactly the block it saw before; a caller that reads them can tell whether the sandbox's own work was preserved or whether repair had to use the shared branch. When the shared branch was used, `fallback` names the missing sandbox ref and the VCS recovery; it never implies the shared branch holds the sandbox's work.

**Slug:** `recovery-warning`

### FR-015: No New Surface

There is no new MCP tool or command. The existing MCP tool names and arguments are unchanged, and the existing CLI commands keep their names. The recovery warning is additive. The only intentional CLI changes are the three `delete` flags replacing `--force`. No lifecycle verb, pause/resume/list/inspect MCP tool or recovery command is added.

**Slug:** `no-new-surface`

### FR-016: Documentation And Matrix

`docs/docs/user/litterbox/lifecycle.mdx`, `src/docgen.rs` and both generated reference pages are updated together. The state table, lifecycle diagram and operation matrices state that a missing sandbox repairs on the next operation and materialises from the sandbox branch; that `diverged` and `resolving` are source states, not refusals; that a paused command, including the three `sandbox-reconcile` actions, is refused with class `conflict`, while `sandbox-status` answers; that `delete` has one flag per guard and `--force` is gone; that a corrupt record is removed with `--ignore-invalid`; that `list` reports records and branches including unpaired entries; and that `forwards` and `sandbox-ports` are record-only. The generated pages are produced by `cargo run -p agentkit-litterbox -- docgen cli` and `cargo run -p agentkit-litterbox -- docgen mcp`, never by hand.

The CLI matrix gains a `delete` column per guard, and both surfaces gain a second matrix over the source states `clean`, `diverged` and `resolving`. Every cell is additive to the existing compute-state matrix.

**Slug:** `lifecycle-documentation`

### FR-017: Diverged And Resolving Are Source States

Every sandbox has a compute state (`active`, `paused`, `missing`) and a source state (`clean`, `diverged`, `resolving`). The two are independent. `diverged` means the shared branch advance is held: the branch moved while the sandbox had unsaved work, or the shared tip carries a conflict. `resolving` means an open `sandbox-reconcile` resolution. When both hold, `resolving` is reported. Neither state refuses a command tool or freezes compute; only the branch advance waits.

`list` reports the compute state and the record's source state without inspecting the workspace. `pause` and `resume` change compute only and leave the source state as it was. Shell and the MCP command tools run; their snapshots commit to the sandbox branch and never advance the shared branch while the source state is not `clean`. `delete` applies its running and unmerged guards (FR-012). `sandbox-create` on a missing name repairs and lands in the same source state (FR-018). Repair materialises the sandbox branch and keeps the record's merge and resolution, so a `diverged` or `resolving` sandbox stays in that state after repair. `sandbox-status` reports `diverged` with the reason and `resolving` with the resolution. `sandbox-reconcile` with `begin` opens a resolution from `diverged` only when the divergence conflicts and the shared tip is clean; it fails when the merge is clean, fails from `resolving` because one is already open, and fails from `clean` because there is nothing to resolve; `finish` and `abandon` close an open resolution and fail when none is open. The three actions follow the paused refusal when the compute is paused (FR-008).

`sandbox-status` and `litterbox status` answer when only the sandbox revision is absent and a resolution is open. They report `resolving`, `sandbox: null`, the recorded `synced` and `ours`, and a `next` carrying the continuation from FR-018. They do not fail because the sandbox revision cannot be read; any other VCS error still propagates.

**Slug:** `source-state-matrix`

### FR-018: Repair With An Open Resolution

Repair keeps an open resolution when the sandbox branch exists. The workspace is materialised from the sandbox branch tip, which is the revision under resolution; the record's `resolution { synced, ours }` is copied to the repaired record unchanged. The record carries `synced`, the base both sides agreed on, and `ours`, the sandbox tip the resolution replaced; the revision under resolution is the merge of `ours` and the shared tip on that base, which is why `ours` must survive. The resolution is never cleared silently. Repair does not call `resolve_begin`, does not create the sandbox branch, and does not rewrite the shared branch.

If the sandbox branch or workspace is absent, repair refuses per FR-006 rather than clearing the resolution, because the revision under resolution is not addressable through the durable refs. The refusal is a continuation. It names the recorded `synced` and `ours`, the VCS store, and the recovery for the selected VCS: Git keeps the resolution merge as a dangling object after its ref is deleted, so `git fsck --lost-found` in the store names it and `git update-ref refs/litterbox/workspaces/<name> <commit>` restores the ref; jj keeps the resolution commit in its operation log, so `jj op log` and `jj log -r 'all()'` name it and `jj workspace add --name <name> --revision <commit>` re-attaches the workspace. If the merge is gone, restore the ref or workspace at `ours`, abandon the open resolution with `sandbox-reconcile abandon`, and run `sandbox-reconcile begin` again. Pushing the sandbox branch to the shared branch is not a recovery: it publishes a conflicted tree. If neither commit is recoverable, inspect the shared branch and then run `litterbox delete <name> --ignore-unmerged`, with `--ignore-invalid` when the record cannot be read, followed by a create. `finish` and `abandon` remain the only operations that clear the resolution.

**Slug:** `repair-open-resolution`

## Non-functional Requirements

### NFR-001: Neutral Failure Classes

Failure classes remain not-found, conflict, unsupported, denied, unavailable, misconfigured and failed. A temporary condition is not `failed`. A paused refusal is `conflict`. A refused delete names the guard and its flag and is `denied`. `Unknown` is a status, not a failure class. An impossible repair is `unavailable` when the mechanism or durable state is missing, or the structured provisioning class when preparation refuses. Every refusal names a remedy the user can run.

**Slug:** `neutral-failure-classes`

### NFR-002: No Payload Dependence

Repair must not require a fully readable payload. The state directory and workspace are located by project and sandbox name, and the branches are named by the record; only the recorded pids are read, to prove compute is absent. This is required because the motivating incident carries payload version 1, which the full reader refuses, and because a payload may be corrupt while the workspace and branches are sound. A payload with no readable pid is not repaired.

**Slug:** `no-payload-dependence`

### NFR-003: Atomic Repair

A repair either ends with a live sandbox and a new record, or leaves the record and workspace as they were. Resources created during a failed attempt are removed. A concurrently addressed sandbox is never half-repaired. The lock is released on every path, including panic-free early returns and process exit.

**Slug:** `atomic-repair`

### NFR-004: No New Dependency

The advisory lock uses the platform's existing file-locking primitive through `libc`, which is already a dependency. No new crate, daemon, service or runtime is added.

**Slug:** `no-new-dependency`

### NFR-005: Backward Compatibility

MCP tool names, arguments and existing result shapes are unchanged. Existing CLI commands and exit classes are unchanged except the documented `delete` confirmation, which becomes three flags; `--force` is removed with no alias, because one alias cannot say which condition it overrides, and a script that used it passes the flags the refusal names. Records written by this release remain readable. Payload version 1 records are not read in place; they remain addressable, removable and repairable from the sandbox branch and workspace, and repair replaces the payload only after a new sandbox process is ready. The source-state values change from `held`/`conflicted` to `diverged`/`resolving`, and the status responses gain a nullable `sandbox`, `ours` and `next`; existing readers must handle the new values and null. No field or tool is removed.

**Slug:** `backward-compatibility`

### NFR-006: Test Matrix

Every cell of the operation matrices in `lifecycle.mdx` has a test at the orchestrator or CLI/MCP seam. The compute matrix covers `list`, `shell`, `pause`, `resume`, `delete`, the MCP command tools, `sandbox-ports` and `sandbox-create` against active, paused, missing and corrupt records. The source matrix covers `list`, `shell` and the MCP command tools, `pause`, `resume`, `delete`, `sandbox-create`, `sandbox-status`, `sandbox-reconcile` and repair against `clean`, `diverged` and `resolving`. It includes a repair of a `Pending` sandbox that materialises from the sandbox branch and preserves its work, and a `sandbox-reconcile` action addressed to a paused sandbox that is refused with `conflict`. A regression test recreates the motivating incident: a current record, a live shared branch, a live sandbox branch with different work, dead recorded pids and an unreadable payload.

**Slug:** `test-matrix`

### NFR-007: Generated Documentation

`reference/cli.mdx` and `reference/mcp.mdx` are generated from `src/docgen.rs`, and both pages are regenerated in the change. A test asserts the lifecycle section names every operation and every state it claims to cover, including `diverged` and `resolving` and the `delete` flags.

**Slug:** `generated-documentation`

### NFR-008: Verification

`cargo test -p agentkit-litterbox` and `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings` pass. `arch-lint check` and `bash hack/arch-lint-negative.sh` pass, because the repair source rule adds no VCS code to the orchestrator. The ignored host suite is not required to prove the orchestration behaviour, but a test seam drives a fake backend through repair, paused refusal, unknown enumeration, corrupt removal and each delete guard.

**Slug:** `verification`

## Acceptance Criteria

### AC-001: Incident Recovers

Given a current record for `workspace-tests`, the live shared branch `litterbox/workspace-tests`, a live sandbox branch `refs/litterbox/workspaces/workspace-tests` holding different work, dead recorded pids and payload version 1, the next command tool call either repairs the sandbox and reports the recovery, or fails with a remedy that names a command that actually works. If it repairs, the workspace is preserved, the sandbox branch's work is what is materialised, and the command runs. The test uses the same shape as the recorded incident, with a controlled state directory and both branches.

**Slug:** `workspace-tests-recovers`

### AC-002: Paused Returns Immediately

`shell`, every MCP command tool and the `sandbox-reconcile` actions `begin`, `finish` and `abandon` addressed to a paused sandbox return without blocking and without writing a command into the sandbox or touching the resolution tree. The CLI prints the resume command and exits 4. MCP returns class `conflict` with the same reason and remedy. `list`, `sandbox-status` and `sandbox-ports` still answer for the paused sandbox.

**Slug:** `paused-returns-immediately`

### AC-003: List Shows Unpaired Entries

A record without a shared branch appears in `list` and can be addressed. A branch without a record still appears as an orphan. A record whose compute is missing appears as missing. A listing where the backend cannot be asked prints `unknown` with the reason and exits 1.

**Slug:** `list-shows-unpaired`

### AC-004: Matrix Is Enforced

Every cell of the CLI and MCP operation matrices has a test, including the source matrix's `diverged` and `resolving` columns, the `Pending` repair, the paused `sandbox-reconcile` refusal and each `delete` guard. A change to a cell without a matching test fails the suite.

**Slug:** `matrix-tests`

### AC-005: Class Parity

For every refusal that appears on both surfaces, the CLI exit code and the MCP class derive from the same contract error and agree. Paused is `conflict`, missing is `not-found` for commands that do not repair and a successful repair for those that do, corrupt delete is not a parse failure, and an unavailable backend is `unavailable`.

**Slug:** `cli-mcp-class-parity`

### AC-006: Corrupt Record Is Removable

A current record whose payload cannot be parsed is removed by `delete --ignore-invalid` without a parse error. The record file, the shared branch, the sandbox branch, the synced revision and the state directory are gone, and the output says compute could not be stopped when it could not be identified.

**Slug:** `corrupt-record-removable`

### AC-007: Delete Guards Are Independent

`delete` names every flag the record needs. An active sandbox needs `--ignore-running`; a record whose status cannot be read needs `--ignore-invalid`; a sandbox whose branch has work the shared branch does not, an open resolution, or a staging workspace needs `--ignore-unmerged`. A missing sandbox needs no compute flag. Passing one flag does not satisfy another. With every required flag, delete removes the record, state, staging workspace, shared branch, sandbox branch and synced revision and prints what it removed. A second delete succeeds.

**Slug:** `delete-missing-needs-force`

### AC-008: Create Reports Recovery

`sandbox-create` on a missing name returns a live inspection and a structured recovery field naming the shared branch, the materialise source and the prior state. `sandbox-create` on an active, paused, `diverged` or `resolving` name returns conflict. `sandbox-create` on a branch with no record returns conflict and does not adopt the branch.

**Slug:** `create-reports-recovery`

### AC-009: Concurrent Repair Spawns Once

Two concurrent operations on the same missing sandbox produce one repair. The second observes the first's result and does not start a second sandbox process or write a second payload. The test exercises the lock at the seam, not only the happy path.

**Slug:** `concurrent-repair-single-holder`

### AC-010: No New Surface

The MCP tool list is unchanged. No new command is added. `delete` is the only command whose arguments change: `--force` becomes `--ignore-running`, `--ignore-invalid` and `--ignore-unmerged`. A search of the lifecycle documentation finds no removed state or operation, and `held` no longer names the source state.

**Slug:** `no-new-tool-or-verb`

### AC-011: Build Is Clean

`cargo test -p agentkit-litterbox` passes, `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings` is clean, and `arch-lint check` and `bash hack/arch-lint-negative.sh` pass after the change.

**Slug:** `tests-and-clippy`

### AC-012: Docs Are Regenerated

`reference/cli.mdx` and `reference/mcp.mdx` contain the updated lifecycle matrices, and a test asserts the generated sections name the paused refusal including `sandbox-reconcile`, the sandbox-branch repair source and its fallback, `diverged` and `resolving`, the per-condition delete flags, corrupt removal, and records-without-branches.

**Slug:** `reference-regenerated`

### AC-013: Repair Uses The Sandbox Branch

A `Pending` or `diverged` sandbox whose workspace is gone repairs by materialising `refs/litterbox/workspaces/<name>`, not the shared tip. The recovery block names `sandbox` as the source. When the sandbox branch is absent the repair materialises the shared tip, the block names `shared` and a `fallback` naming the missing ref, and the command succeeds. When both refs are absent the repair returns the unavailable remedy.

**Slug:** `repair-source`

### AC-014: Open Resolution Survives Repair

A missing sandbox with an open resolution and a live sandbox branch repairs and keeps the resolution record unchanged; the workspace holds the revision under resolution. If the sandbox branch is absent, repair refuses rather than clearing the resolution, and the remedy names it. `finish` and `abandon` are the only operations that clear it.

**Slug:** `repair-keeps-resolution`

### AC-015: Absent Sandbox Revision Is Recoverable

With an open resolution and no sandbox ref or workspace, `sandbox-status` and `litterbox status` return `resolving`, `sandbox: null`, the recorded `synced` and `ours`, and a `next` naming the Git or jj recovery and the delete fallback. Repair refuses with the same continuation and writes no ref. The test exercises the Git and jj cases at the seam.

**Slug:** `absent-sandbox-revision`

## Edge Cases

### EC-001: Unreadable Payload With Workspace

The payload is version 1 or otherwise unreadable, but the state directory, workspace and recorded pids exist. The status check reads only the pids and reports missing when they are gone. Repair does not require the full payload, reuses the workspace, rebuilds the mounts and environment from resolved provision, starts the sandbox process and writes a current payload. The incident is this case.

**Slug:** `unreadable-payload-with-workspace`
### EC-002: Workspace Missing

The state directory exists but the workspace directory does not. Repair materialises the workspace from the sandbox branch `refs/litterbox/workspaces/<name>`, then starts the sandbox process. It does not use HEAD, and it uses the shared branch only when the sandbox branch is absent (EC-020).

**Slug:** `workspace-missing`
### EC-003: Branch Missing

The record exists. If the shared branch is gone but the sandbox branch is present, repair materialises the sandbox branch and succeeds, reporting `sandbox` as the source; the shared branch is not recreated. If both refs are gone, repair refuses as unavailable and names `litterbox delete <name>` with the flags the refusal names, followed by a create. The record remains listed and deletable.

**Slug:** `branch-missing`
### EC-004: State Directory Corrupt

The state directory exists but cannot be used as a state root, or the payload has no readable pid so absence cannot be proven. Repair refuses with a remedy that deletes the record and recreates it. It does not remove the directory and then fail, and it does not leave a partly written payload.

**Slug:** `state-directory-corrupt`
### EC-005: Backend Unavailable

The selected backend is not ready. Repair is not attempted. The operation returns `unavailable` with the readiness reason and remedy. Enumeration reports every record as `unknown` with that reason and exits 1.

**Slug:** `backend-unavailable`
### EC-006: Concurrent Calls

Two operations address the same missing sandbox at once. One takes the lock and repairs; the other waits, then observes active. A third call after both sees active and does not repair.

**Slug:** `concurrent-calls`
### EC-007: Repair Spawn Fails

The sandbox process starts but does not become ready, or the backend fails after spawning. The backend tears down what it started, leaves the workspace and record as they were, and returns a neutral failure. A retry can repair again.

**Slug:** `repair-spawn-fails`
### EC-008: Orphan Branch

A branch exists with no record. `list` reports it as an orphan. `sandbox-create` on that name returns conflict and does not adopt it. `delete` removes the branch as it does today.

**Slug:** `orphan-branch`
### EC-009: Record Without Branch

The record exists and the compute may even be active, but the shared branch is gone. `list` reports the record. If the sandbox branch is present, repair uses it; if both are gone, commands that need durable state to repair refuse. `delete` removes the record and compute once the guards' flags are passed.

**Slug:** `record-without-branch`
### EC-010: Paused Host-Side Operations

`list`, `sandbox-status`, `sandbox-ports`, `pause`, `resume` and `delete` continue to work on a paused sandbox. Upload, download and snapshot do not join and continue. None of them blocks.

**Slug:** `paused-host-side-operations`
### EC-011: Paused Delete

Deleting a paused sandbox needs no compute flag, because nothing is running. It still needs `--ignore-unmerged` when the sandbox branch holds work the shared branch does not, when a resolution is open, or when a staging workspace exists. It keeps its teardown behaviour.

**Slug:** `paused-delete`
### EC-012: Create Active Or Paused

`sandbox-create` on an active, paused, `diverged` or `resolving` name returns conflict. It does not repair, pause, resume or delete. The existing sandbox is untouched.

**Slug:** `create-active-or-paused`
### EC-013: Corrupt Delete Without Ignore Invalid

`delete` on a corrupt record without `--ignore-invalid` refuses and names what would be removed, because compute may be running and cannot be identified. With `--ignore-invalid` it proceeds without parsing the payload. `--ignore-unmerged` joins it when the addressable sandbox branch holds work.

**Slug:** `corrupt-delete-without-force`
### EC-014: Preparation Refuses During Repair

Host preparation refuses during repair because setup consent is missing or a devShell cannot be built. The structured provisioning failure is returned, no sandbox process starts and the workspace is untouched.

**Slug:** `preparation-refuses-during-repair`
### EC-015: List Without A Backend

`list` cannot construct the backend. It still prints every record from the metadata directory, each as `unknown` with the reason, and exits 1. It does not omit records or print a fabricated inspection.

**Slug:** `list-without-backend`
### EC-016: Repair While Paused

A paused sandbox is not missing. No repair is attempted. A command is refused per FR-008; pause, resume and delete behave as before.

**Slug:** `repair-while-paused`
### EC-017: Stale State Files

The state directory holds a status file, a diagnostic file and a payload from the dead compute. Repair replaces the status and diagnostic files when it starts and writes the new payload only after the sandbox process is ready. A failed attempt leaves the previous payload in place. No recorded pid, namespace inode or scope from the dead compute is reused.

**Slug:** `stale-state-files`
### EC-018: Setup After Workspace Rebuild

The workspace had to be materialised from the sandbox branch. The branch does not contain setup effects that were never snapshotted, so the sandbox provisioning steps run again after the new sandbox process is ready. If a step fails, the repair fails and the newly materialised workspace is removed, leaving the record in its pre-repair state.

**Slug:** `setup-after-workspace-rebuild`
### EC-019: Unreadable Payload Delete Needs Ignore Invalid

A current record whose payload is unreadable or from an unsupported version has an undetermined status. `delete` without `--ignore-invalid` refuses and says compute may still be running. With `--ignore-invalid` it removes the record, both branches, the synced revision and the state directory and reports compute it could not stop.

**Slug:** `unreadable-payload-delete-force`
### EC-020: Sandbox Branch Absent

The record exists and the shared branch is live, but `refs/litterbox/workspaces/<name>` is absent, as on a record written before the branch split. Repair materialises the shared tip, reports `source: shared` and a `fallback` naming the missing ref, and succeeds. It does not create the sandbox branch; the next plan initialises the bookkeeping refs from the shared tip.

**Slug:** `sandbox-branch-absent`
### EC-021: Repair Preserves Sandbox Work

The sandbox is `missing` and `diverged`: the shared branch is at the user tip, the sandbox branch holds the agent's snapshots, and the workspace is gone. Repair materialises the sandbox branch, so the agent's work survives. The record keeps its held merge, and the first command after repair snapshots on the sandbox branch without advancing the shared branch.

**Slug:** `repair-preserves-sandbox-work`
### EC-022: Paused Reconcile

A paused sandbox has a `diverged` source state. `sandbox-status` reports the divergence and the open resolution. `sandbox-reconcile` with `begin`, `finish` or `abandon` is refused with class `conflict` and the resume command before the workspace is touched. After `litterbox resume`, the same action proceeds.

**Slug:** `paused-reconcile`
### EC-023: Commands While Diverged Or Resolving

A `diverged` sandbox runs shell and every MCP command tool. Snapshots commit to the sandbox branch and do not advance the shared branch. A `resolving` sandbox does the same, and the edits are the resolution. Neither state freezes compute or refuses a tool.

**Slug:** `commands-while-diverged`
### EC-024: Open Resolution With No Sandbox Branch

The record has an open resolution and `refs/litterbox/workspaces/<name>` or the jj workspace is absent. This can follow a partly completed delete, a manual ref deletion or a rebuilt store. Repair refuses as unavailable rather than falling back to the shared branch, because the revision under resolution is not addressable; clearing the resolution silently is forbidden. `sandbox-status` and `litterbox status` still answer with `resolving`, `sandbox: null`, the recorded `synced` and `ours`, and the continuation from FR-018: Git `fsck --lost-found` and `update-ref`, jj `op log` and `workspace add`; or re-open from `ours`; or delete with `--ignore-unmerged` and create. Pushing the sandbox branch to the shared branch is not a recovery.

**Slug:** `resolution-without-sandbox-branch`
### EC-025: Several Delete Guards

An active, `diverged` sandbox with an open resolution and a staging workspace needs `--ignore-running` and `--ignore-unmerged`. A corrupt record with an addressable sandbox branch needs `--ignore-invalid` and `--ignore-unmerged`. The refusal names both flags; passing one leaves the other condition refused.

**Slug:** `several-delete-guards`
### EC-026: Delete Flag Without Its Condition

A missing sandbox with no sandbox-branch work, no resolution and no staging workspace receives `--ignore-running`. The flag is accepted, reported as unused, and the delete proceeds; it does not make a later condition pass.

**Slug:** `unused-delete-flag`
### EC-027: Sandbox Branch Equals The Shared Tip

The sandbox branch exists and names the same revision as the shared branch, as it does immediately after a create or an adoption. Repair materialises that revision and reports `source: sandbox`; there is no fallback because the ref exists.

**Slug:** `sandbox-branch-equals-tip`

