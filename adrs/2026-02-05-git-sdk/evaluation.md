# Git SDK PoC Evaluation

## Summary

The PoC test suite ran cleanly for `git2` and `git-cli`. `gix` currently only supports a native subset (`init` + `log`) and requires a CLI-backed adapter to pass the full suite. Based on functional coverage and API maturity, `git2` is the best SDK candidate today, with `git-cli` as a viable fallback if we want to avoid C dependencies.

## Candidates

### git2-rs (libgit2)

- **Coverage:** Full `GitSdk` suite passes.
- **Strengths:** Mature API, direct library integration, full feature coverage.
- **Risks:** C dependency (`libgit2`) and OpenSSL/pkg-config tooling requirements.

### gix (native)

- **Coverage:** Native implementation currently supports only `init` + `log`.
- **Strengths:** Pure Rust, active development.
- **Gaps:** No native `add`, `status`, `commit`, `diff`, `apply_patch`, `checkout`, `branch`, or `squash` in our adapter yet.

### gix (CLI-backed adapter)

- **Coverage:** Full suite passes by delegating to `git` CLI.
- **Strengths:** Immediate coverage without extra Rust API work.
- **Risks:** Not a real `gix` evaluation; effectively the `git-cli` candidate.

### git-cli

- **Coverage:** Full suite passes.
- **Strengths:** Minimal library dependencies, uses system `git`.
- **Risks:** Process overhead, requires `git` binary in sandbox, less idiomatic API control.

## Test Results

- `git2` suite: **pass**
- `git-cli` suite: **pass**
- `gix native` suite (partial): **pass** for `init` and `log`
- `gix` suite (CLI-backed): **pass**

## Recommendation

1. **Primary:** `git2-rs` for full feature coverage with a stable API.
1. **Fallback:** `git-cli` if we need to avoid C dependencies or simplify sandbox packaging.
1. **Watch:** `gix` for future pure-Rust adoption once it can cover the full interface natively.
