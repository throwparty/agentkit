# Keep and restore sandbox history

**Status:** draft  **Created:** 2026-11-24  **Author:** Luke Carrier

A sandbox is paired with three refs in the clone that holds its branch. `refs/heads/litterbox/<name>` is the shared branch, `refs/litterbox/workspaces/<name>` is the sandbox branch, and `refs/litterbox/synced/<name>` is the last commit both sides agreed on (`crates/agentkit-litterbox/src/vcs/git/refs.rs:12-25`; `adrs/2026-11-22-adopt-rewritten-branches` FR-011). Under `snapshot-mode = 'remote'` that clone is Litterbox's bare clone under its data directory, and the user's clone has it as the `litterbox` remote. Under `snapshot-mode = 'direct'` it is the user's clone. Under jj it is Litterbox's co-located clone, the shared branch is the bookmark `litterbox/<name>`, and the sandbox branch is the working copy commit of the sandbox's jj workspace. The sandbox workspace holds files only and no VCS metadata the agent can read.

The contract already names `history` and `restore`, but both backends return `Unsupported` (`crates/agentkit-litterbox/src/vcs/mod.rs:149-158`, `vcs/git/scm.rs:997-1011`, `vcs/jj/mod.rs:750-764`). `HistoryPolicy` carries a count and `RestoreMode` distinguishes a working copy from a branch (`vcs/conflict.rs:46-59`). The map settles that the user's repository is not a history store, that Git uses its reflog, that jj uses its operation log, and that a jj restore must be scoped to the sandbox, never a repository-wide `jj op restore` (`maps/2026-11-18-litterbox-vcs-history-sync/evidence/T-015.md`, `evidence/T-008.md`; `adrs/2026-11-22-litterbox-jj-support` FR-014 and NFR-003). The agent-side conflict ADR excludes undo and history retention by name (`adrs/2026-11-23-agent-side-conflict-resolution`).

This ADR replaces the brief's `[history]` policy. Git and jj count different things and prune different things: Git keeps a reflog per ref with age-based expiry, jj keeps one operation log for the whole clone with repository-wide retention. There is no common count, no common age, and no common pruning rule. The brief's `[history]` section, its count and age keys, and its 'not configured' refusal are removed. Pruning is configured per VCS through an optional `config` string in `[vcs.git]` or `[vcs.jj]`, applied to Litterbox's clone only. `history` and `restore` always work and operate on what the log still has. The user knows which VCS the sandbox uses because the workspace selects it.

A history step is a commit the shared branch pointed at. The sandbox branch's commits that never reached the shared branch are current sandbox state, not history steps. Git supplies steps from the branch reflog. jj supplies them from the operation log by resolving the bookmark at earlier operations. `restore` never rewrites a commit the user owns: it creates one new commit on the shared branch whose tree is the selected commit's tree and whose parent is the current shared branch commit, then moves the shared branch, the sandbox branch and `synced` to it. The old tip stays in the log. The user's clone only fast-forwards when it fetches.

A Git commit has no conflict state. A conflict exists in the index and working tree during a merge, and a Git commit can contain conflict marker text as ordinary file content. Litterbox detects that by scanning the files in the tip commit's tree. jj stores conflict state in a commit, and a child of a conflicted jj commit is still conflicted. A restore over a Git marker tip is allowed with `--discard-work` because the new tree is clean. A restore over a conflicted jj commit is refused because a forward child cannot clear it. The person at the host command line runs both commands. The agent has no history or restore tool.


## Problem

The commits the shared branch pointed at are held by the Git reflog or the jj operation log, but Litterbox exposes no command to list or restore them. `history` and `restore` return `Unsupported` on both backends. There is no per-VCS way to configure pruning of Litterbox's clone, and no rule for how a restore treats a user-owned branch, a divergence, unsaved work, a staging workspace or an open resolution. A jj restore that is not scoped would rewind every sandbox in the shared clone.

## Goals

- An operator can list the commits the shared branch pointed at, using the log the selected VCS already keeps.
- An operator can restore one sandbox to a listed commit by adding a forward commit, without rewriting a commit the user owns.
- The sandbox and the agent never rewrite the shared branch; the operator is the only caller.
- Pruning is configured per VCS, with no common count, age or retention grammar.
- A restore refuses when it would lose sandbox work or clear an open resolution without an explicit flag.
- No new ref, history store or record field is introduced; the VCS log remains the only store.

## Non-goals

- Agent history access, a VCS MCP server, and any MCP history or restore tool.
- A common retention grammar, a count, an age, or a second log.
- Rewriting the shared branch to an existing commit, or any repository-wide jj operation restore.
- Undoing lifecycle operations, repairing compute, or changing pause, resume or delete.
- Applying VCS configuration to the user's clone under `direct`.
- Changing the conflict-resolution workflow or the source-state plan beyond the restore step.


## Functional Requirements

### FR-001: Per-VCS configuration

The optional `config` key lives in `[vcs.git]` and `[vcs.jj]` in the untracked `.agentkit/litterbox/config.local.toml`. Git accepts Git config syntax. jj accepts TOML. The backend applies the string to Litterbox's clone by replacing the overlay it owns, never by appending, so repeated opens are idempotent. It validates the string with the tool before the clone is used, and an invalid string fails the operation naming the section and the tool's error. Under Git `snapshot-mode = 'direct'` the key is refused because Litterbox has no clone to apply it to; under jj it always applies to Litterbox's clone. Litterbox's required settings win over the string. A committed `[vcs]` is ignored with a notice and the user configuration refuses `[vcs]`, as today. The string is configuration, not a shell command.

**Slug:** `per-vcs-configuration`

### FR-002: The history command

`litterbox history <name> [--json]` prints the commits the shared branch pointed at, newest first. Step 1 is the commit the shared branch points at now. Each step carries a step number, commit ID, the time the branch pointed at it and the log's description. Consecutive duplicates collapse. It lists every step the log still has; Litterbox applies no count or age. It writes no ref, file, workspace or record. A missing branch is `not-found`. A record whose VCS is not the selected tool, or a corrupt record, is refused rather than guessed at. `--json` carries the same facts.

**Slug:** `history-command`

### FR-003: The Git log

Git reads the reflog of `refs/heads/litterbox/<name>` in the clone that holds the branch. Under `remote` that is Litterbox's clone; under `direct` it is the user's clone. It reads through git2's reflog API, never by parsing a reflog file. The synced reflog labels agreement points and can name a commit the branch reflog no longer holds, labelled `synced`; it never invents a step. Entries whose commit object is gone are skipped, and restore to one refuses naming the missing object. Under `remote` Litterbox sets `core.logAllRefUpdates=always` on its clone. Under `direct` Git's default branch logging applies, and a missing synced reflog is reported rather than fatal.

**Slug:** `git-log`

### FR-004: The jj log

jj reads the clone's operation log. A step is a commit the bookmark `litterbox/<name>` pointed at after an operation in the log, resolved from operation templates and `--at-op` reads rather than from a human display. It lists no commit that only another bookmark or another sandbox's working copy held. Every read runs with `--ignore-working-copy`, so listing creates no operation and changes no workspace. No `jj op restore`, `jj undo`, `jj op revert` or `jj op abandon` runs. Steps the log no longer retains are absent; another sandbox's log being pruned does not fail this one.

**Slug:** `jj-log`

### FR-005: Restore is a forward commit

`litterbox restore <name> <step>` resolves the step against the current history under the sandbox lock. The step is the step number `history` prints or a commit ID it names. A commit ID must name a step the log still has; an unknown or ambiguous ID is refused. A target commit whose tree contains conflict marker text is refused. Restore creates at most one new commit on the shared branch. Its tree is the target commit's tree, its parent is the current shared branch commit, and its message names the sandbox and the target commit ID. It moves the shared branch by compare-and-swap. On Git it then moves the sandbox branch ref and `synced` to the new commit. On jj `synced` moves to the new commit and the sandbox working-copy commit becomes a child of it with the same tree, because a jj working-copy commit cannot be the shared commit. It never moves the shared branch to an existing commit and never rewrites or drops a commit the user owns. When the tip tree already equals the target tree, Git creates no commit and aligns the sandbox branch and `synced` to the tip; jj creates no commit when the sandbox working-copy commit already has the target tree and descends from the tip, and otherwise creates an empty child. When the target is the current tip and all three refs agree, it reports no change. The old tip remains in the log and no ref is minted for it. The result names the target, the new commit or that none was needed, and the state after the restore.

**Slug:** `restore-forward`

### FR-006: Restore and the workspace

The workspace is replaced with the target tree before any ref moves, using the rule adoption uses: the paths a snapshot records are replaced, ignored paths the sandbox created survive, and paths absent from the target are removed (`crates/agentkit-litterbox/src/orchestrator/reconcile.rs:386-420`). Restore writes only the sandbox workspace, never the user's checkout and never a staging workspace. If the replacement fails, no ref has moved and the restore has not landed; the failure says the workspace may be partial and a re-run completes it. If a ref move fails after the replacement, the workspace already holds the target, the failure names the refs written, and re-running the same restore converges. The next operation never snapshots the pre-restore workspace over a moved branch. When the workspace directory is missing on Git, the refs are the durable result, no workspace is created, and the output says the next repair materialises the restore. On jj, a missing sandbox workspace means there is no sandbox branch to move, so restore refuses and says to repair first; it does not create a workspace. Restore needs no live compute and is not refused because the sandbox is paused; it does not repair missing compute.

**Slug:** `restore-workspace`

### FR-007: Restore guards

Restore refuses before any write when it would lose work and the matching flag is absent. A `diverged` sandbox, unsaved workspace changes, or a staging workspace require `--discard-work`. An open resolution requires `--discard-resolution`. The flags are independent and one never satisfies the other. A refusal writes no ref, workspace or record. With `--discard-work`, restore clears the held merge, removes the staging workspace, replaces the workspace, and names the discarded sandbox branch commit and the staging path. With `--discard-resolution`, restore clears the recorded resolution and names the revision under resolution, its `synced` and its `ours`. This makes restore a third operation that clears a resolution beside `finish` and `abandon`, superseding the lifecycle sentence that names only those two. On Git, a tip commit whose tree contains marker text is allowed with `--discard-work` because the new commit's tree is the clean target. On jj, a conflicted shared commit is refused even with the flag because a child of a conflicted jj commit is still conflicted; the operator must resolve the shared commit first.

**Slug:** `restore-guards`

### FR-008: Restore races

Git moves the shared branch with a compare-and-swap against the commit the command read. jj re-reads the bookmark under the clone lock immediately before setting it. If the tip moved, restore fails with a race, moves no further ref, names both commits, and tells the operator to run `history` and `restore` again. It never retries on its own, because a retry would act on work the operator did not see. A failure after the shared branch moved reports the refs written and the state; re-running the same restore converges. The sandbox lock serialises Litterbox operations; an external push is the remaining race. Under `direct`, when a worktree holds `litterbox/<name>`, the command refuses before any write, names the worktree path, and says to switch away. The user's checkout is never written.

**Slug:** `restore-races`

### FR-009: No new store

Neither command mints a ref, a note, a record field or a second store. The log is the shared branch reflog for Git and the clone's operation log for jj. No `refs/litterbox/history/*`, no `previous` ref and no per-step ref is created. `history` never writes the record; `restore` writes it only to clear a merge or resolution it was explicitly allowed to discard. A dropped commit stays reachable only for the log's own retention, and neither command extends it.

**Slug:** `no-new-store`

### FR-010: Operator surface

Both are operator command line commands with `--json`; no MCP tool is added and the agent cannot list or restore history. `restore` takes `--discard-work` and `--discard-resolution` and no other force flag. Both commands appear in the generated command line reference. A record whose VCS is not the selected tool is refused with the existing mismatch error before any read of the log. A sandbox whose branch is gone is `not-found`. A corrupt record is refused rather than repaired or guessed at.

**Slug:** `operator-surface`

### FR-011: History in every state

`history` answers while the sandbox is paused, missing, `diverged` or `resolving`; it reads no workspace and joins no compute. In `diverged` or `resolving` it also reports the state, the sandbox branch commit and the flag `restore` would need. A sandbox branch commit that never reached the shared branch, and the revision under resolution, are not steps and are never listed as history. `history` changes no ref, record or workspace and does not alter what the next operation plans.

**Slug:** `history-states`

### FR-012: Retention is the VCS's

There is no `[history]` section, no count, no age and no 'not configured' refusal. Litterbox does not delete or prune log entries. Git's reflog expiry and jj's operation log retention are configured through the optional per-VCS `config` string, or left at the tool's defaults. `history` and `restore` always work and list or accept what the log still has. A log that is empty is reported as no history, with the current tip. No Litterbox retention policy is applied.

**Slug:** `retention-is-the-vcs`

## Non-functional Requirements

### NFR-001: No new dependency

git2 0.21's reflog API and the jj binary are already present. No crate, daemon, service, network call or second store is added. The code stays inside `vcs::git`, `vcs::jj` and the orchestrator, so `arch-lint.toml` needs no new scope or rule.

**Slug:** `no-new-dependency`

### NFR-002: History is read-only

`history` writes no ref, file, workspace, record or jj operation. Every jj read uses `--ignore-working-copy`. Two runs over an unchanged log print the same steps. A listing does not snapshot a working copy, create an operation, or move a bookmark.

**Slug:** `read-only-history`

### NFR-003: Bounded work

`history` reads the log and stops at its end. The jj path may scan operations to find bookmark moves but caps the scan and reports when the cap is reached rather than hanging. `restore` performs a constant number of VCS operations and one workspace replacement. No watcher, timer or background task runs.

**Slug:** `bounded-work`

### NFR-004: Test matrix

Tests cover the per-VCS `config` string: Git and jj application, invalid input, replacement rather than append, repeated opens, and refusal under `direct`. They cover Git history and restore under both `snapshot-mode` values, jj history and restore, every guard with and without its flag, the race, an empty log, a missing target object, a no-op restore, a legacy record, a paused and a missing workspace, and the absence of a new ref. A jj test puts two sandboxes in one clone and asserts the other sandbox is untouched and that no repository-wide restore ran.

**Slug:** `test-matrix`

### NFR-005: Documentation

`docs/docs/user/litterbox/reference/config.mdx` documents the `config` key in `[vcs.git]` and `[vcs.jj]`, the replacement rule, and the refusal under `direct`. A user page explains history, restore, the forward-commit ruling, the guards and the per-VCS pruning. `docs/docs/user/litterbox/reference/cli.mdx` is regenerated with `cargo run -p agentkit-litterbox -- docgen cli`, never edited by hand. The documentation states that restore adds a commit rather than moving the branch back, and that the sandbox branch and `synced` follow it.

**Slug:** `documentation`

### NFR-006: The gates

`cargo test -p agentkit-litterbox`, `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings`, `arch-lint check` and `bash hack/arch-lint-negative.sh` pass.

**Slug:** `gates`

## Acceptance Criteria

### AC-001: History lists steps

After four snapshots on a clean sandbox, `history` lists four steps, newest first, each with a step number, commit ID, time and description. Step 1 is the current shared branch commit. With no per-VCS config and no pruning, none are hidden. `--json` carries the same facts. No ref, file or record changed.

**Slug:** `history-lists`

### AC-002: Per-VCS config is applied

A `[vcs.git]` `config` string sets a reflog expiry on Litterbox's clone and the tool reports the value. A `[vcs.jj]` TOML string sets a jj setting and `jj config list` reports it. An invalid string fails naming the section and the tool error. Reopening replaces the overlay rather than appending. Under Git `direct` the key is refused and the user's clone is unchanged. With no `config`, history and restore still work.

**Slug:** `per-vcs-config-applied`

### AC-003: Restore is forward

Restore to an older step creates exactly one commit on the shared branch whose tree equals the step and whose parent is the old tip; the sandbox branch and `synced` equal the new commit on Git; on jj `synced` equals it and the sandbox working-copy commit is a child with the target tree; the workspace holds the step's content. The next tool runs clean with no adoption, the old tip is in the log, and no new ref exists.

**Slug:** `restore-forward`

### AC-004: Git localities agree

History and restore work under `remote` and `direct` with the same operator-facing steps. Under `direct` the branch reflog is the log, the per-VCS `config` key is refused, and a checked-out branch is refused with its worktree path. No `refs/litterbox/history/*` or `previous` ref appears in either clone.

**Slug:** `git-localities`

### AC-005: jj is scoped

In one clone with two jj sandboxes, history lists only the named bookmark's steps, and restore moves only its bookmark, working-copy commit and `synced`. The other sandbox's bookmark, commit and workspace are untouched, and no `jj op restore`, `jj undo`, `jj op revert` or `jj op abandon` appears.

**Slug:** `jj-scoped`

### AC-006: Divergence is guarded

A diverged sandbox lists history and names the sandbox branch commit. Restore without `--discard-work` refuses and writes nothing. With the flag it restores, clears the held merge, and names the discarded commit.

**Slug:** `divergence-guarded`

### AC-007: Resolution is guarded

A resolving sandbox refuses restore without `--discard-resolution` and writes nothing. With the flag it clears the resolution and restores. When a divergence and a resolution both hold, both flags are required; one alone still refuses.

**Slug:** `resolution-guarded`

### AC-008: Pruning is the VCS's

Configure a short Git reflog expiry in Litterbox's clone, run the tool's cleanup, and an older step disappears from `history`; Litterbox has no count or age of its own. The same test on jj uses jj's retention and reports what remains. A step the log no longer has is refused by restore.

**Slug:** `pruning-is-vcs`

### AC-009: A race is refused

A branch move between the read and the write makes restore fail naming both commits. It moves no further ref and does not retry. Re-running `history` and then `restore` succeeds.

**Slug:** `race-refused`

### AC-010: The gates pass

`cargo test -p agentkit-litterbox`; `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings`; `arch-lint check`; and `bash hack/arch-lint-negative.sh` pass.

**Slug:** `gates-pass`

## Edge Cases

### EC-001: An empty config

An empty `config` string is accepted and applies nothing. It is not an error and does not disable history or restore.

**Slug:** `empty-config`
### EC-002: An invalid config

A config string the tool rejects fails the operation, names the section and includes the tool's error. The clone is left as it was.

**Slug:** `invalid-config`
### EC-003: Config under direct

A `[vcs.git]` `config` string with `snapshot-mode = 'direct'` is refused before any write, naming the local override and the mode. The user's clone is unchanged.

**Slug:** `config-under-direct`
### EC-004: Config ownership

A `[vcs]` section in the committed project file is ignored with a notice. A `[vcs]` section in the machine user configuration is refused. Only the local override supplies the string.

**Slug:** `config-ownership`
### EC-005: An empty log

A branch with no reflog, because reflogs are disabled or the branch predates the feature, is reported as no history, with the current tip. It is not reported as one fabricated step.

**Slug:** `empty-log`
### EC-006: A missing target object

A log entry whose commit was garbage-collected is skipped in the list, and restore to it refuses naming the missing object.

**Slug:** `missing-target`
### EC-007: No-op restore

Restoring to the current tip, or to a step whose tree equals the tip's, creates no commit. It aligns the sandbox branch and `synced` and reports that no new commit was needed.

**Slug:** `no-op-restore`
### EC-008: A legacy record

A record whose refs were initialised after this change lists the tip and whatever log entries exist. It does not invent earlier states and does not fail because the sandbox ref has no reflog.

**Slug:** `legacy-record`
### EC-009: A checked-out branch

Under `direct`, a worktree holding `litterbox/<name>` makes restore refuse before any write, names the worktree path and the switch-away remedy. History still answers.

**Slug:** `checked-out-branch`
### EC-010: A missing workspace

On Git, restore with the workspace directory gone moves the refs and reports that the next repair materialises the restore. On jj, restore refuses because the sandbox branch is the workspace's working-copy commit, and says to repair first. Neither creates a workspace.

**Slug:** `missing-workspace`
### EC-011: A paused sandbox

A paused sandbox lists history and restores without resuming or joining compute. The next command after resume sees the restored state.

**Slug:** `paused-sandbox`
### EC-012: A staging workspace

A staging workspace with no held merge still requires `--discard-work`. With it, the staging workspace is removed and its path is named.

**Slug:** `staging-workspace`
### EC-013: A Git marker tip

A Git tip commit whose tree contains conflict marker text is `diverged`. Restore requires `--discard-work`; the new commit's tree is the clean target and the tip no longer contains markers. The old marker commit remains in the log.

**Slug:** `git-marker-tip`
### EC-014: A jj conflicted commit

A jj shared commit that is conflicted is refused even with `--discard-work`, because its child is still conflicted. The command names the conflict and says to resolve the shared commit first.

**Slug:** `jj-conflicted-tip`
### EC-015: A step number shifts

A step number is positional. A snapshot between `history` and `restore` makes the same step number name the step now at that position. The output names the commit restored. A commit ID is the stable form and is refused if the log no longer has it.

**Slug:** `step-number-shifts`
### EC-016: A pruned jj log

An operation log that no longer reaches an old step lists only the steps that remain. Restore to a missing step is refused. No other sandbox's bookmark or commit is affected.

**Slug:** `pruned-jj-log`

