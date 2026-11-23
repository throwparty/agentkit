# Brief for agent-side conflict resolution

Input to `adrs/<date>-agent-side-conflict-resolution`. Written after T-023 and T-024 closed and the
frontier emptied. Each line is marked. Decided means you ruled on it in this conversation. Shown
means a probe in this map demonstrates it. Open means the ADR must rule on it. Nothing here is a
spec.

## Decided

1. Both conflicts are in scope. The divergence between the sandbox branch and the shared branch, and
   a shared-branch tip that is itself conflicted. A jj workspace sitting at `synced` with conflict
   markers present is still conflicted. The test suite has to prove that a fix reaches the commit
   that introduced the conflict.
2. One tree. Resolution changes what `/src` holds. It does not use a second tree, a second sandbox,
   or a mount swap. Disk cost and the one-sandbox-per-session expectation rule out a second
   sandbox.
3. MCP never writes a host tree. The MCP tools reach only the sandbox. If a host tree is ever
   needed, it is a worktree against Litterbox's own working copy, never the user's checkout. Host
   staging stays with the operator.
4. The working copy is ambient. Nothing holds it. Any tool may change it. Only the branch advance is
   held.
5. State changes reach the agent. Any operation that changes the tree reports state to the agent,
   and later calls repeat it.
6. Snapshot on every change to the tree.
7. The conflict report gives paths and counts. A tool lists conflicted files with a hunk count and a
   side count, never the hunks.
8. Resolved means the VCS reports no conflicts, through that same tool.
9. History inspection is out of scope. You supply whatever means the agent gets, expected for now to
   be read-only shell access on the host. A VCS MCP server is a later ADR.
10. Edits become snapshots. The agent needs no new write permission for its edits.
11. Both jj workflows were to be tested by experiment, not chosen in the abstract.

## Shown

- Git round-trips through a tree of files with no metadata. `merge-tree --write-tree --merge-base`
  writes a tree with markers. Materialised into a directory with no `.git` and no index, anything
  that edits files can edit it, and the host can turn the result into a merge commit with two
  parents and a clean tip. Git's only possible check for resolution is the marker scan from T-022.
- Both jj flows work. `jj new <theirs> <ours>` gives a conflicted commit that is both the working
  copy and the sandbox branch. Resolving it rewrites it clean and the bookmark follows. A conflicted
  tip taints its descendants. `jj new <tip>` then `jj squash --use-destination-message` rewrites the
  commit that introduced the conflict and leaves it clean.
- `jj resolve --list` exits 2 when the revision is clean and 1 on error.
- Resolving changes the commit id. `0e5d3952` became `97cbf119`, and the old id keeps reporting a
  conflict. Track the commit that introduced the conflict by position, not by id.
- jj writes its own marker format: `<<<<<<< conflict N of M`, `%%%%%%% diff from:`, `\\\\\\\ to:`
  and `+++++++`.
- Abandon is re-materialisation. Rebuilding the merge from its two recorded parents reproduces the
  conflict and needs no saved state. `jj op restore` is repository-wide and would rewind every other
  sandbox in the shared clone.
- T-024: agents resolve in place unprompted. Both runs fixed the conflict at the commit that
  introduced it, and neither added a child commit, so resolving in a child and squashing it adds
  work without adding capability.
- T-024: both runs produced trees that passed every automated check and were wrong. One deleted the
  user's whole side. The other left conflict-header text in the file as content. The conflict query,
  the marker scan, bookmark consistency and ancestry all reported success. Character-level editing
  of the markers failed twice. Only a diff against each side caught either.
- A three-way merge over the three versions resolves correctly. `git merge-file --union` produced 26
  lines with both sides and no markers. It failed in the sandbox only because the sandbox did not
  have the three versions.

## Open

The first three questions below were ruled on after this brief was written. They are kept here with
their answers so the brief and the ADR do not disagree.

1. ~~The check for resolution.~~ **Ruled: report it, do not enforce it.** `finish` refuses only when
   the VCS still reports conflicts. It returns a diff against base, ours and theirs for every
   conflicted path, and the agent has to justify the result. Enforcement has a trap: a deliberate
   union drops a side.
2. ~~Which host writes are permitted.~~ **Ruled: Litterbox performs every host write.** The agent's
   authority ends at `finish`. The commit, the ref move and any rewrite of a user-owned commit
   belong to Litterbox and use the same consent check staging already requires.
3. ~~How the three sides reach the agent.~~ **Ruled: an optional side on `read`.** `base`, `ours`
   and `theirs` are served from the backend. No path, mount or extra tool.
4. ~~The MCP tools.~~ **Ruled: `sandbox-reconcile` carries begin, finish and abandon; `sandbox-status`
   is added; `read` gains a side.** `sandbox-reconcile` stopped staging a host worktree, so no MCP
   call writes one.
5. ~~The `Vcs` contract.~~ **Ruled: `blob`, `resolve_begin` and `resolve_abandon` join the contract,
   and `Conflict` gains counts.** `stage`, `finish` and `discard_stage` keep meaning the host
   staging path the operator uses.
6. Landing a fix to a conflicted tip. A divergence is resolved by advancing the branch to a merge
   revision. A conflicted tip still needs the resolution squashed into the commit that introduced
   the conflict, and nothing has ruled whether rewriting a commit the user owns is allowed.

## Evidence

| What | Where |
|---|---|
| counts, exit codes, stale ids, both jj flows, Git round-trip | `evidence/T-023/README.md` |
| working copy state machine | `evidence/T-023/working-copy-states.mmd` |
| scripts and raw output | `evidence/T-023/*.sh`, `*.out` |
| agent comprehension, both framings | `evidence/T-024.md` |
| three-way merge over the sides | `evidence/T-024/merge-file/` |

## Out of scope

History inspection by the agent. A VCS MCP server. Native conflicts in the main workspace outside
resolution. Undo and history retention. The lifecycle delta: Pending, staging cleanup, the delete
warning.
