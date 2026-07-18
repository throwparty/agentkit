---
name: pr-check-failure
description: Diagnosing and fixing failing GitHub Actions CI checks and broken builds
---

## When to use me

When a PR's GitHub Actions checks are failing and you need to determine the cause and fix it.

## Diagnosis workflow

1. **Fetch and switch to the PR branch** — get the exact branch name from the PR's `head.ref` field (via `github_pull_request_read` with `get` method), then use the `fetch-remote` skill to fetch, then use the `switch-branch` skill to check out that ref.

2. **Check if the failure is transient or real**
   - If the failure is `dorny/paths-filter` with a GitHub API 5xx error → transient, re-run the workflow.
   - If the failure mentions Nix hash mismatch → real, needs a hash update.
   - If the failure is a Nix daemon connection error → likely transient, re-run.

3. **For transient failures**: re-run via `gh run rerun <run-id>`.

4. **For Nix hash mismatches**: the error output shows `got: sha256-<hash>` which is the correct hash. Extract it and update `cargoHash` in `nix/flake.nix`.

5. **For other failures**: inspect the full job log for the actual error message.

## Transient failures to ignore

- `dorny/paths-filter` → GitHub API 5xx, re-run
- Nix connection to daemon socket → re-run
- Any step that succeeds on re-run without code changes
