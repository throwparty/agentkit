---
name: github-actions
description: GitHub Actions CI/CD workflows, composite actions, and workflow linting (actionlint, zizmor)
---

## When to use me

- Creating or modifying `.github/workflows/*.yaml` files
- Creating or modifying `.github/actions/*/action.yaml` composite actions
- Creating or modifying scripts under `scripts/` that workflows call
- Writing or running bats tests for workflow scripts

## Project conventions

### Workflow files

Naming: `<scope>.<action>.yaml` (e.g. `nix.lint.yaml`, `nix.update-hashes.yaml`, `agentkit.build-and-test.yaml`).

Workflow structure:
- `name:` with quoted string, e.g. `"Nix: lint"`
- `on:` events
- `concurrency:` group and cancellation policy
- `permissions: {}` at top level (empty, then per-job)
- Jobs with `name:`, `runs-on: ubuntu-24.04`, scoped `permissions:`, optional `if:` for conditionals
- Steps use `name:` field for readability
- Action versions pinned by commit SHA with `# ratchet:owner/repo@tag` annotation

### Composite actions

Stored in `.github/actions/<name>/action.yaml`. Used with `uses: ./.github/actions/<name>`.

### Script extraction

No inline bash in workflows. All bash scripts live in `scripts/` as standalone `.sh` files that:
- Start with `#!/usr/bin/env bash`
- Use `set -euo pipefail`
- Accept arguments and environment variables for configuration
- Are tested with bats

Workflow `run:` steps call scripts directly, e.g. `run: scripts/check-and-update-cargo-hash.sh`.

### Bats tests

Tests live in `tests/` as `.bats` files, one per script. Test helper functions and fixtures go in `tests/test_helper/`. Run with `bats tests/`.

### Key patterns

#### dorny/paths-filter
Used in "Detect changes" jobs. Defines path filters per scope. The GitHub API can transiently 5xx — re-run the workflow if that happens.

#### Nix and git config
Nix flake fetching uses SSH URLs internally. These are rewritten to HTTPS via `git config --global --add url."https://github.com/".insteadOf` for each SSH format. This allows Nix to fetch public repos without SSH keys.

#### Checkout with persist-credentials
When a workflow needs to `git push`, checkout with `persist-credentials: false` then inject credentials before push:
```yaml
- uses: actions/checkout@<sha>
  with:
    persist-credentials: false
- run: scripts/push-with-token.sh
```
The script configures the origin remote URL with `GITHUB_TOKEN` embedded: `git remote set-url origin "https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPOSITORY}"`. This only changes the local repo's `origin` remote, so Nix's git operations for other repos are unaffected.

#### zizmor linting
`zizmor` checks for security issues in workflows. The `artipacked` finding flags credential persistence. Fixed with `persist-credentials: false` + explicit token injection before push (not with inline suppression comments).

#### Actionlint
Run actionlint to validate workflow syntax. Ignore `unexpected key "queue" for "concurrency" section` since it's a GitHub partner preview feature.

### bash features to use

- `set -euo pipefail`
- `[[ ]]` instead of `[ ]` for conditionals
- `$(...)` instead of backticks
- `local` for function-scoped variables
- `readonly` for constants
- `${var:-default}` for defaults, `${var:?error}` for required vars
- `printf` over `echo` for portability
- `#!/usr/bin/env bash` for portability
- `>` redirect for write, `>>` for append
