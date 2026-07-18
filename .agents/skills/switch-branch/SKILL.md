---
name: switch-branch
description: Switching branches or bookmarks before making changes
---

## When to use me

When you need to switch to a different branch (e.g. a PR branch) before editing files.

## Detection

Check which VCS is active, in order:

- **jj** — if `.jj/` exists (project uses Jujutsu)
- **git** — if `.git/` exists

## Switching to a remote branch

### jj

Get the exact branch name from the PR's `head.ref` field via `github_pull_request_read`.

```bash
# Fetch latest
jj git fetch

# Create a new change on top of a remote branch
# If the branch is tracked: use <remote>/<branch-name>
# If not yet tracked: use <branch-name>@<remote>
jj new dependabot/cargo/html-to-markdown-rs-3.8.2@origin

# Or track it first, then use the short name:
jj bookmark track <branch-name>@<remote>
jj new <branch-name>
```

If jj hasn't fetched the ref, pull it via git first:

```bash
git fetch origin <branch-name>
jj bookmark track <branch-name>@origin
jj new <branch-name>
```

### git

```bash
# Fetch the remote branch
git fetch origin <branch-name>

# Check it out locally
git switch <branch-name>
# or: git checkout <branch-name>
```

## Checking current branch

```bash
# jj
jj log -r '@' --no-graph

# git
git branch --show-current
```
