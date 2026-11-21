# Verification

Date: 2026-11-19

## Gates

- `cargo test -p agentkit-litterbox`: 523 passed, 14 ignored.
- `cargo clippy -p agentkit-litterbox --all-targets -- -D warnings`: clean.
- `arch-lint check`: 0 errors, 424 files.
- `bash hack/arch-lint-negative.sh`: exit 0; every VCS plant rejected.
- The orchestrator, CLI and MCP contain no `git2`, `vcs::git` or `vcs::git_common`.

## Parity

`compose::tests::create_list_and_delete_through_the_git_backend` drives the real
`GitVcs` through the orchestrator with the recording compute backend: create
makes `litterbox/demo`, list reports it, delete removes it.

`vcs::git::vcs_tests` covers `prepare`, `materialise`, `snapshot`, `tip`,
`delete_branch`, `capabilities` and the unsupported surface. The moved Git tests
(snapshot commits, ignore rules, archive bytes, bare-store remote operations)
pass unchanged.

## Scope note

`config_loader.rs` still uses `git2` for repository discovery, workdir selection
and tracked/committed checks. That is configuration code, not VCS code: it
decides configuration scope from Git concepts and is outside this ADR's
boundary. The boundary this change enforces is orchestration and the surfaces:
neither imports `vcs::git`, `vcs::git_common` or `git2`.

A live sandbox create/exec/snapshot against Docker or bubblewrap was not run in
this environment; the mechanism is unchanged and the orchestrator-to-backend
path is covered by the existing `TestBackend` suite.
