# Cartography map

**Destination:** Litterbox lets a user rewrite the content and history of a sandbox's branch. The sandbox adopts the rewrite when it has no unsaved work. A divergence is handed to the user on the host as a merge based on the revision both sides agreed on. Git and jj are supported to the limits of their capabilities.

Created: 2026-11-18

## Summary

| Metric | Value |
|--------|-------|
| Tickets | 23 |
| Answered | 22 |
| Open | 1 |
| Probing | 0 |
| Abandoned | 0 |
| Blocked | 0 |
| Frontier | T-023 |

## Tickets

### T-001: Can a jj repository be backed by a bare Git repository and host multiple jj workspaces, one per sandbox?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes: a non-colocated jj repo backed by a bare Git repo hosts one jj workspace per sandbox; the bridge needs an initial workspace, and each sandbox branch must be a bookmark exported with `jj git export`. |
| Evidence | [evidence/T-001.md](evidence/T-001.md) |

### T-002: Can a jj workspace be bind-mounted into a sandbox with `.jj` masked on bubblewrap, Docker and Seatbelt?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Bubblewrap and Docker hide the contents of `.jj` with a tmpfs or empty volume; the directory entry remains empty; Seatbelt is unprobed on Linux; a backend that cannot mask must refuse `Bound`. |
| Evidence | [evidence/T-002.md](evidence/T-002.md) |

### T-003: How does jj expose conflict paths and their base/ours/theirs sides, and does editing the materialised markers resolve the conflict or is `jj resolve` required?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-001 |
| Answer | Paths come from `jj resolve --list`; sides from `jj diff --from <base> --to <ours|theirs> --git -- <path>`; editing the materialised markers and snapshotting clears the working-copy conflict, so `jj resolve` is optional (merge-tool entry only). |
| Evidence | [evidence/T-003.md](evidence/T-003.md) |

### T-004: Does `git merge-tree --write-tree --merge-base=<tree>` produce a usable conflict tree and exit status, and can that tree be committed and annotated as a conflicted commit?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes: exit 1 on conflict, the tree OID is the first stdout line, the tree contains markers, and `commit-tree` yields an ordinary two-parent commit with no conflict metadata. |
| Evidence | [evidence/T-004.md](evidence/T-004.md) |

### T-005: Can a conflicted Git commit be marked with refs/notes so Litterbox can compute inherited conflict for descendants, and what is the ref/note layout?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-004 |
| Answer | Technically yes, but no tool ecosystem uses notes for conflict or divergence; git-appraise and Gerrit use notes for review metadata only. Reject the pattern; both VCS use host staging; native conflicts deferred. |
| Evidence | [evidence/T-005.md](evidence/T-005.md) |

### T-006: Can `rmcp` tool results carry a structured `workspace` block alongside the existing content, and what is the exact schema?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes: `CallToolResult.content` is a `Vec<ContentBlock>` and `structured_content` is optional; append `ContentBlock::json(workspace)` or use `structured()`. |
| Evidence | [evidence/T-006.md](evidence/T-006.md) |

### T-007: What is the exact `plan`/`decide` sequence for each action (tool, snapshot, rewrite, merge, resolve, abort, repair) and each conflict model?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-001, T-003, T-005 |
| Answer | One `plan` over Base/Workspace/Merge returns ordered steps; pre-tool and post-tool; no tool is blocked, but the hold model keeps the shared branch at the user tip while the sandbox snapshots its own branch; native conflicts deferred; every tool returns the post-plan `workspace` block. T-019 supersedes the Git merge branch: a dirty sandbox blocks the advance instead of merging, and the host resolves. |
| Evidence | [evidence/T-007.md](evidence/T-007.md) |

### T-008: How are the `workspaces`, `synced` and `conflicts` refs represented in jj and Git, and who updates them?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-001, T-007 |
| Answer | Git: branch plus `refs/litterbox/workspaces` and `synced`; history is a bridge reflog; `Blocking` merge sides in the record; no notes or conflict refs. jj: bookmark, synced bookmark or record field, jj workspace and op log, `jj git export`; abort is scoped, not a global op restore. T-019 fixes the `workspaces` ref as the sandbox branch and the `ours` side; T-020 fixes retention. |
| Evidence | [evidence/T-008.md](evidence/T-008.md) |

### T-009: How do pause, missing, repair and delete interact with the source state, especially a conflicted sandbox?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-007 |
| Answer | Lifecycle and source are orthogonal; pause/resume do not touch the source; repair materialises the branch and re-materialises a `Blocking` merge from record sides; `--force` only means stop it while running, and delete warns about unmerged work against the recorded base. |
| Evidence | [evidence/T-009.md](evidence/T-009.md) |

### T-010: What is the migration and configuration path for VCS selection and workspace binding on existing records?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | `[vcs] tool` with a per-VCS configuration section (`[vcs.git]`, `[vcs.jj]`) mirroring `[backends.*]`; the selected section supplies binding; required for create and stored per sandbox; legacy records are Git + Copied facts; incompatible combinations are refused at validation, never defaulted. The section is a workspace setting in the untracked `.agentkit/litterbox/config.local.toml`, not machine-wide user configuration: a machine hosts many workspaces, so one user file cannot carry their choices. The committed project file cannot set it. |
| Evidence | [evidence/T-010.md](evidence/T-010.md) |

### T-011: What is the operator CLI surface, and how does it share the `plan` logic with MCP?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-007 |
| Answer | `status`, `conflicts`, `resolve`, `abort`, `shell`; every command calls the same orchestrator `plan`; no `sync` command (reconciliation is lazy); `--json` mirrors the MCP `workspace` block. T-019 supersedes the Git commands: `status` and `reconcile` with `--finish` and `--abort`; the sandbox never resolves. |
| Evidence | [evidence/T-011.md](evidence/T-011.md) |

### T-012: How are `.jj`/`.git` masked per backend, and what does the backend do when it cannot mask a `Bound` workspace?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-002 |
| Answer | Bubblewrap and Docker mask `.jj` contents with a tmpfs or empty volume; the path remains as an empty mountpoint; a backend that cannot mask refuses `Bound`; Seatbelt is unprobed. |
| Evidence | [evidence/T-002.md](evidence/T-002.md) |

### T-013: What is the current module layout, where is it Git-coupled, and what has to move for one VCS-neutral trait and a `plan`-driven orchestrator?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Sandbox/backends are neutral; `scm` (1,777 lines) holds a Git-shaped trait and `GitScm`; `vcs_store` is a bare-Git store; `orchestrator/service.rs` (2,270) imports `ThreadSafeScm`/`VcsStore`/`ScmMode` and calls `git2::Repository::discover` directly; `domain::ScmError` wraps `git2::Error`; `mcp.rs` (2,276) and `cli.rs` (1,572) are monoliths. Define the VCS interface and `vcs::git_common` first, add arch-lint, adapt Git, build the plan/conflict surface, then add jj. |
| Evidence | [evidence/T-013.md](evidence/T-013.md) |

### T-014: Should Git emulate deferred conflict resolution with notes/refs, given the lack of tool adoption?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-005 |
| Answer | No. Notes are a metadata pattern for review and attribution, not conflict state; both VCS use host staging and native conflicts are deferred. `Deferred` is rejected. |
| Evidence | [evidence/T-005.md](evidence/T-005.md) |

### T-015: How should multiple steps of sandbox history be kept without putting bookkeeping refs in the user's repository?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Retention and restore semantics are user configuration with no default; Git uses a bridge reflog on `synced`, jj uses its op log; if `[history]` is absent, history/restore report unconfigured. The user's repository is not the history store. T-020: a dropped tip stays in the branch reflog; no new ref is minted. |
| Evidence | [evidence/T-015.md](evidence/T-015.md) |

### T-016: What gaps does a non-colocated jj bridge open, and are they acceptable?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | C is accepted: only the Git remote is external, and this model never creates a conflicted jj commit; native conflicts are deferred. Export the bookmark as-is, surface export failures, scope aborts, serialise bridge operations; no separate clean export. |
| Evidence | [evidence/T-016.md](evidence/T-016.md) |

### T-017: What is the generic VCS interface, what code is shared between backends, and what arch-lint rules keep VCS code out of the orchestration code?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Define `vcs` (trait, `Revision`, `TreeId`, `Capabilities`, `Merge`, `Conflict`, `ConflictSides`) and `vcs::git_common` (Git concepts: ignore, archive, object/ref primitives, path safety, naming) first; the contract must also express the operations orchestration uses today (branch create/delete, branch enumeration, archive, snapshot commit, snapshot filter, remote/direct fact); adapt Git; add jj later. Orchestrator imports `vcs` only (never `vcs::git_common`); `vcs::git` and `vcs::jj` may import `vcs`/`vcs::git_common` but never each other; arch-lint 0.11.0 scopes with `deny-scope-dep` and `independent` enforce the boundary. |
| Evidence | [evidence/T-017.md](evidence/T-017.md) |

### T-018: Can Seatbelt deny a path inside a workspace it already allows, so a bound workspace can hide `.git` or `.jj`?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes: SBPL resolves rules last-match-wins, so `(deny file-read* (subpath <ws>/.jj))` and `(deny file-write* (subpath <ws>/.jj))` emitted after the workspace allow rules override them; the `.jj` entry stays visible in a listing but every read, write and stat under it returns EPERM, and descendants inherit it. |
| Evidence | [evidence/T-018.md](evidence/T-018.md) |

### T-019: Where does a rewritten branch get reconciled, and what does the sandbox do?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | The sandbox never merges. It adopts when its workspace matches its own branch and holds the shared branch at the user tip when it does not. The host resolves in a staging workspace under the data directory (a linked worktree for Git, a jj workspace for jj). The merge uses the synced revision as its base because Git's inferred base is wrong after a rebase. Staging is a command the user runs or an MCP tool the harness may allow; it never writes the user's checkout. Finish fast-forwards when the branch is free and prints the command when the checkout holds it. |
| Evidence | [evidence/T-019.md](evidence/T-019.md) |

### T-020: How are sandbox revisions that a rewrite drops retained?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | The Git branch reflog; jj uses its operation log. Adoption reports the old tip once and names the reflog; the object stays reachable for the reflog lifetime. No new ref and no record field. The bare store logs every ref update; under direct the user's repository logs branches by default. |
| Evidence | [evidence/T-020.md](evidence/T-020.md) |

### T-021: When the bridge tip is a conflicted revision, does the sandbox fall back to the last non-conflicted history, and at which points does it check?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Yes: on every operation that reads source state, check the bridge tip; if conflicted, hold at synced and the sandbox branch and report; adopt only a non-conflicted tip. The main workspace stays buildable. |
| Evidence | [evidence/T-021.md](evidence/T-021.md) |

### T-022: How does each VCS report a conflicted tip (jj commit state, Git marker tree), and what does the MCP workspace block say?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-021 |
| Answer | jj: `jj resolve --list -r <rev>` and the `conflict` template; Git: a marker scan of the tip tree, as a safety net behind the host staging. The workspace block reports `held`, `reason: bridge tip is conflicted`, synced, bridge_tip, paths and the next step. |
| Evidence | [evidence/T-022.md](evidence/T-022.md) |

### T-023: Can an agent resolve a deferred conflict, and if so what resolution context and knowledge does it need?

| Field | Value |
|-------|-------|
| State | open |
| Depends on | T-021 |

## Decision

Git and jj sit behind one VCS trait with opaque revisions and capabilities. Define `vcs` and `vcs::git_common` first, add arch-lint rules, adapt the existing Git implementation to the interface, build the plan surface on Git `Blocking`, and add jj later as a second implementation that uses `vcs::git_common` and never calls `vcs::git`. Git uses `Blocking` as a hold: the shared branch stays at the user tip while the sandbox snapshots its own branch, and the host resolves the difference. jj uses the same hold and host-staging model (one jj workspace per sandbox, non-colocated bridge, `.jj` masked); native conflicted commits and agent-side resolution are part of the goal; T-023 is open, and the fallback is T-021/T-022. `Deferred` and git-notes emulation are rejected. The source state is derived from Base/Workspace/Merge axes over four trees, and `Workspace` compares the workspace with the sandbox branch. One `plan` is the only consumer. No sandbox tool is blocked while diverged; only the branch advance is held. Every sandbox tool returns a proactive `workspace` block; `sandbox-status` and `sandbox-reconcile` are the model surface, and `sandbox-reconcile` stages only. `status` and `reconcile` with `--finish` and `--abort` are the operator surface. Git staging is a linked worktree under the data directory on a temporary branch and jj staging is a jj workspace; the merge uses `synced` as its base because Git's inferred base is wrong after a rebase. The user's checkout is never written. Dropped tips stay in the branch reflog; no new ref is minted. History retention and restore semantics are user configuration with no default; Git uses the reflog, jj uses its op log, and the user's repository is not the history store. Open risks: the masked `.jj` is never absent — an empty mountpoint under bubblewrap/Docker, present-but-inaccessible under Seatbelt's deny rule (T-018); the non-colocated jj bridge exposes only the Git remote; the main workspace never holds a conflicted tree, so the Git consumer gap depends on the conflict living on the bridge (T-021), but export failures must surface, aborts must be scoped and bridge operations serialised; delete warns about unmerged work and `--force` only means stop it while running.

ADR: `adrs/2026-11-19-decouple-vcs-from-orchestration`

