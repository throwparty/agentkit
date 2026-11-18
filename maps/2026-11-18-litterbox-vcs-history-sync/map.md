# Cartography map

**Destination:** Litterbox lets a user rewrite the content and history of a sandbox's branch and the sandbox adopts that rewrite, with Git and jj supported to the limits of their capabilities, conflicts surfaced to the agent through MCP, and conflicts resolvable without giving the sandbox VCS access.

Created: 2026-11-18

## Summary

| Metric | Value |
|--------|-------|
| Tickets | 17 |
| Answered | 17 |
| Open | 0 |
| Probing | 0 |
| Abandoned | 0 |
| Blocked | 0 |
| Frontier | empty |

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
| Answer | Technically yes, but no tool ecosystem uses notes for conflict or divergence; git-appraise and Gerrit use notes for review metadata only. Reject the pattern; Git uses `Blocking`, jj uses `Native`. |
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
| Answer | One `plan` over Base/Workspace/Merge returns ordered steps; pre-tool and post-tool; no tool is blocked, but `Blocking` holds the snapshot and branch while `Native` snapshots and allows descendants; every tool returns the post-plan `workspace` block. |
| Evidence | [evidence/T-007.md](evidence/T-007.md) |

### T-008: How are the `workspaces`, `synced` and `conflicts` refs represented in jj and Git, and who updates them?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-001, T-007 |
| Answer | Git: branch plus `refs/litterbox/workspaces` and `synced`; history is a bridge reflog; `Blocking` merge sides in the record; no notes or conflict refs. jj: bookmark, synced bookmark or record field, jj workspace and op log, `jj git export`; abort is scoped, not a global op restore. |
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
| Answer | Machine-owned `[vcs] tool` and `[workspace] binding`, required for create and stored per sandbox; legacy records are Git + Copied facts; incompatible requests are refused, never defaulted. |
| Evidence | [evidence/T-010.md](evidence/T-010.md) |

### T-011: What is the operator CLI surface, and how does it share the `plan` logic with MCP?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-007 |
| Answer | `status`, `conflicts`, `resolve`, `abort`, `shell`; every command calls the same orchestrator `plan`; no `sync` command (reconciliation is lazy); `--json` mirrors the MCP `workspace` block. |
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
| Answer | Sandbox/backends are neutral; `scm` (1,777 lines), `vcs_store` and `orchestrator/service.rs` (2,270) are Git-coupled; `mcp.rs` (2,276) and `cli.rs` (1,572) are monoliths. Define the VCS interface and `vcs::common` first, add arch-lint, adapt Git, build the plan/conflict surface, then add jj. |
| Evidence | [evidence/T-013.md](evidence/T-013.md) |

### T-014: Should Git emulate deferred conflict resolution with notes/refs, given the lack of tool adoption?

| Field | Value |
|-------|-------|
| State | answered |
| Depends on | T-005 |
| Answer | No. Notes are a metadata pattern for review and attribution, not conflict state; Git uses `Blocking` and jj uses `Native`. `Deferred` is rejected. |
| Evidence | [evidence/T-005.md](evidence/T-005.md) |

### T-015: How should multiple steps of sandbox history be kept without putting bookkeeping refs in the user's repository?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Retention and restore semantics are user configuration with no default; Git uses a bridge reflog on `synced`, jj uses its op log; if `[history]` is absent, history/restore report unconfigured. The user's repository is not the history store. |
| Evidence | [evidence/T-015.md](evidence/T-015.md) |

### T-016: What gaps does a non-colocated jj bridge open, and are they acceptable?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | C is accepted: only the Git remote is external, so a conflicted jj commit is not a supported Git consumer. Export the bookmark as-is, surface export failures, scope aborts, serialise bridge operations; no separate clean export. |
| Evidence | [evidence/T-016.md](evidence/T-016.md) |

### T-017: What is the generic VCS interface, what code is shared between backends, and what arch-lint rules keep VCS code out of the orchestration code?

| Field | Value |
|-------|-------|
| State | answered |
| Answer | Define `vcs` and `vcs::common` first; adapt Git; add jj later. Orchestrator imports `vcs` only; `vcs::git` and `vcs::jj` may import `vcs`/`vcs::common` but never each other; arch-lint enforces the boundary. |
| Evidence | [evidence/T-017.md](evidence/T-017.md) |

## Decision

Git and jj sit behind one VCS trait with opaque revisions and capabilities. Define `vcs` and `vcs::common` first, add arch-lint rules, adapt the existing Git implementation to the interface, build the plan/conflict/MCP/CLI surface on Git `Blocking`, and add jj later as a second implementation that uses `vcs::common` and never calls `vcs::git`. Git uses `Blocking` (`merge-tree` markers in the workspace, branch held at the user tip until resolve); jj uses `Native` (conflicts in commits, one jj workspace per sandbox, non-colocated bridge, `.jj` masked). `Deferred` and git-notes emulation are rejected. The source state is derived from Base/Workspace/Merge axes and one `plan` is the only consumer. No sandbox tool is blocked while conflicted; only the branch advance is held for `Blocking`. Every sandbox tool returns a proactive `workspace` block; `sandbox-status`, `sandbox-resolve`, `sandbox-conflict` are the model surface; `status`, `conflicts`, `resolve`, `abort`, `shell` are the operator surface. History retention and restore semantics are user configuration with no default; Git uses a bridge reflog on `synced`, jj uses its op log, and the user's repository is not the history store. Open risks: a `Blocking` merge is only durable through the record, so repair must re-materialise it; `.jj` is an empty mountpoint, not absent; Seatbelt masking is unprobed; the non-colocated jj bridge exposes only the Git remote, so a conflicted jj commit is not a supported Git consumer, export failures must surface, aborts must be scoped and bridge operations serialised; delete warns about unmerged work and `--force` only means stop it while running.

