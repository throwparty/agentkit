---
name: fetch-remote
description: Fetching the latest remote branches before switching or inspecting
---

## When to use me

When you need to update local VCS state from a remote (e.g. after looking up a PR's head ref) before switching branches or making changes.

## Detection

Check which VCS is active, in order:

- **jj** — if `.jj/` exists
- **git** — if `.git/` exists

## Fetching

### jj

```bash
jj git fetch
```

Fetches all remotes. Does not auto-track new bookmarks — use `jj bookmark track <name>@<remote>` after fetching if needed.

### git

```bash
git fetch --all
```

Or for a single remote and branch:

```bash
git fetch origin <branch-name>
```
