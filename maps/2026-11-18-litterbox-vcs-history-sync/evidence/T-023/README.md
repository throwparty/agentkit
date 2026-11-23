# T-023: agent-side conflict resolution, probe notes

Scripts and raw output in this directory. Nothing here is production code.

| File | What it establishes |
|---|---|
| `hunks.sh` / `hunks.out` | each backend can report conflicted paths with a hunk count and a side count, without shipping the hunks |
| `git-workflow.sh` / `git-workflow.out` | a marker tree with no metadata, edited with no VCS, becomes a host-written merge commit with a clean tip |
| `jj-workflow.sh` / `jj-workflow.out` | both jj workflows: resolve in place, and resolve in a child then squash into the commit that introduced the conflict |
| `working-copy-states.mmd` | the working copy state machine, a draft for the ADR |

## Counting conflicts

- jj. `jj resolve --list -r <rev>` prints `path    N-sided conflict`, so the path and the side count
  come from one command. The hunk count comes from a marker scan of the materialised tree. `jj diff
  --git` writes no hunk headers for a conflicted merge, so it cannot supply the count.
- Git. `merge-tree --write-tree --merge-base` exits 1, and its path lines carry index stages 1, 2 and
  3, so the path and the side count come from one command. A real merge in a worktree gives the same
  three stages plus `MERGE_HEAD`. The hunk count comes from a marker scan.
- The backends count sides differently. jj says 2-sided. Git holds three stages because it
  materialises a base. The contract has to define `sides` as content sides and report `base`
  separately, or the same conflict reports 2 and 3.

## jj exit codes

`jj resolve --list` exits 2 and prints `Error: No conflicts found at this revision` when the
revision is clean. It exits 1 on a real failure, such as an unresolvable revset. A backend that
treats any non-zero exit as failure reads every clean revision as broken. One that treats any
non-zero exit as clean swallows errors.

## The stale commit id

Both jj workflows rewrite the working-copy commit when it is resolved, so its id changes.

```
captured before the edit: 0e5d3952
held after the edit:      97cbf119
```

The captured id keeps answering `file.txt    2-sided conflict`, because it is now a stale divergent
commit. Anything that holds a revision across a resolution holds the wrong one: a record, a report,
a test assertion. The live predicate is `conflicts()` on the graph, or a fresh read of the bookmark.

This matters for keeping fixes in the commit that introduced the conflict. That commit's identity has
to be tracked by position, through the bookmark or the parent chain, not by id.

## Flow 1: Litterbox introduces the merge

`jj new <theirs> <ours>` creates a conflicted merge commit that is both the working copy and the
sandbox branch. Editing the markers away and letting jj snapshot rewrites that commit clean, and the
bookmark follows the rewrite automatically. `conflicts()` goes from one entry to empty. The sandbox's
own commit survives. No user-owned history is touched.

## Flow 2: the user introduced the conflict

A conflicted shared-branch tip taints its descendants. A clean child still answers `file.txt
2-sided conflict` while not appearing in `conflicts()`. Resolving in the child and running `jj squash
--use-destination-message` rewrites the parent clean, moves the bookmark to the rewritten commit,
empties `conflicts()`, and leaves the sandbox's own commit in place. `jj squash` without
`--use-destination-message` opens an editor, so the backend has to pass the flag.

## Git: a tree of files round-trips

A marker tree materialised into a directory with no `.git` and no index can be edited by anything
that edits files. The host then adds that directory to a temporary index, writes a tree, and commits
it with both tips as parents. The result is `litterbox/probe` at a merge commit with two parents, a
clean status, no markers, and the sandbox tip as an ancestor of the new tip.

The check for resolution on Git is therefore the marker scan from T-022, applied to the tree that
would be committed. There is no index inside `/src` to ask. That is not a workaround. A Git conflict
lives in an index and never in a commit.
