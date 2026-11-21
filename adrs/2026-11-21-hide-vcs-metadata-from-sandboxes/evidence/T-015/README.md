# T-015 — masking with Docker, and the daemon filesystem question

**Question:** T-002 recorded that an empty volume at `/workspace/.jj` hides the contents. Two things were still unknown. Does Docker create the mountpoint when the bound directory holds no `.git` at all, so a path can be masked before it exists? And how does the backend establish that the daemon shares the host filesystem, which `Bound` needs because the bind source is resolved on the daemon's side?

**Answer:** Both are settled.

An empty host directory bound over `/workspace/.git` hides the contents, and a write from inside the container lands in the mask rather than in the host `.git`. A named volume behaves the same way.

When `.git` does not exist, Docker creates the mountpoint *through the workspace bind*, so `<workspace>/.git` appears on the host as an empty directory and the mask is mounted over it. Masking an absent path therefore works, at the cost of leaving an empty directory in the VCS-owned workspace. That is acceptable: the directory is the metadata path Litterbox already hides, and creating it does not expose any VCS data.

The daemon filesystem check is a sentinel round trip. A file written on the host and bound read-only into a container is readable only when the daemon shares the host filesystem. This daemon does. The backend can run the same check once per create, or once per process, and refuse `Bound` when the sentinel is not readable.

## Reproduction

```sh
bash adrs/2026-11-21-hide-vcs-metadata-from-sandboxes/evidence/T-015/probe.sh
```

Raw output: [`output.txt`](output.txt).

Docker 29.9.3, root `/home/lukecarrier/.local/share/docker`, `busybox:latest`. `IMAGE=...` overrides the image.

## Key observations

- Case A, `-v <ws>:/workspace -v <empty>:/workspace/.git` with `.git` present: `readme=hello`, `entries=0`, `write=volume`, host `.git/HEAD` still `SECRET`, no `written` in the host `.git`.
- Case B, the same with `.git` absent: `entry=present`, `entries=0`, and the host workspace gains an empty `.git` directory.
- Case B2, `-v litterbox-probe-mask:/workspace/.git` with `.git` absent: identical to Case B, so a named volume buys nothing over a host directory.
- Case C: the sentinel reads `host-side`, so this daemon shares the host filesystem.
- Case D, no mask: `SECRET`.

## Implication

The Docker implementation binds an empty directory it owns, not a named volume, because the two behave the same and a host directory leaves no named volume to clean up over time. The mask can be bound read-only so the sandbox cannot even write to the mask, though Case A shows writes reaching only the mask either way.

The daemon check belongs in create, before the container is made, and its failure is a refusal of `Bound` naming the daemon rather than a silent copy.
