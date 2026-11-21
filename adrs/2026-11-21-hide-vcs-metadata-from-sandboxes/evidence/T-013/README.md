# T-013 — masking a directory and a file with bubblewrap

**Question:** T-002 proved masking when the metadata path is a directory, where `--tmpfs` hides the contents. Can bubblewrap mask a path that is a *file*, the way a linked worktree's `.git` is? And does the directory shape behave as T-002 recorded?

**Answer:** Both shapes can be masked, with different flags. A directory is hidden with `--tmpfs <workspace>/.git`: the contents are gone, the entry is empty, and a write lands in the tmpfs rather than the host. A file is hidden with `--ro-bind <empty file> <workspace>/.git`: the gitfile reads as empty and a write is refused at the read-only mount. Neither shape exposes the contents, and in both the host inode is untouched afterwards.

This settles the second assumption the plan carried. `masked_paths` has to say, per path, whether the path is a directory or a file, or the backend has to stat the host path to choose the flag. It cannot assume `--tmpfs` for everything.

The control case is included: without a mask the same workspace reads `SECRET` from `.git/HEAD`.

## Reproduction

The probe creates a scratch workspace, runs bubblewrap three times, and exits non-zero if any check fails. Run from the repository root:

```sh
bash adrs/2026-11-21-hide-vcs-metadata-from-sandboxes/evidence/T-013/probe.sh
```

Raw output: [`output.txt`](output.txt).

`bwrap` 0.11.2 from the devShell. `BWRAP=/path/to/bwrap` overrides the binary.

## Key observations

- Case A, `--bind <ws> /tmp/src --tmpfs /tmp/src/.git`: `readme=hello`, `head=denied`, `entries=0`, `write=tmpfs`, host `.git/HEAD` still `SECRET`, no `written` file on the host.
- Case B, `--bind <ws> /tmp/src --ro-bind <empty> /tmp/src/.git`: `readme=hello`, `bytes=0`, `write=denied` with `Read-only file system`, host gitfile still `gitdir: /tmp/elsewhere`.
- Case C, no mask: `SECRET`.

## Implication

Bubblewrap masking is two flags selected by the shape of the masked path, not one. The empty entry stays an empty entry: a masked `.git` is present and empty, never absent, which is what EC-001 of the spec states.

The workspace is mounted at `/tmp/src` rather than `/src` only because `--ro-bind / /` makes `/` read-only and bubblewrap cannot then create `/src`. The real backend builds its own root and mounts the workspace at `/src` (`backends/bubblewrap/view.rs:339-341`), so the flag choice is what transfers, not the path.
