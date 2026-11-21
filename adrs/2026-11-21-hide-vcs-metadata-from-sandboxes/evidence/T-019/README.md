# T-019 — gates and smoke

All four gates pass on a clean tree, and the smoke covers every acceptance criterion that needs a real backend.

## Gates

```text
cargo test -p agentkit-litterbox                                  559 passed, 14 ignored
cargo clippy -p agentkit-litterbox --all-targets -- -D warnings   clean
arch-lint check                                                   0 errors, 0 warnings in 425 files
bash hack/arch-lint-negative.sh                                   every planted import rejected
```

The negative fixture needed no new rule: this change adds no arch-lint scope.

Two of the 559 are live seam tests, added after the first pass because the probes alone left a gap. `a_bound_sandbox_hides_the_vcs_metadata_from_a_real_bubblewrap_sandbox` and `a_bound_sandbox_hides_the_vcs_metadata_inside_a_real_docker_container` each build a bound checkout holding `.git/HEAD`, create a real sandbox through `registry::build`, exec a shell script from inside it, and assert the workspace reads, the metadata does not, and the host file survives. Disabling the mask in either backend turns them red with `git-leaked=SECRET`, so they bite.

## Smoke

A scratch repository with a committed project file naming `bubblewrap`, a user configuration selecting it, and a `.agentkit/litterbox/config.local.toml` carrying `[vcs.git]`. Creation is MCP-only, so `sandbox-create` was driven over stdio.

1. **No `[vcs]` section.** `sandbox-create` fails: `configuration: no workspace binding is configured; set binding = 'copied' or binding = 'bound' under [vcs.git] in .agentkit/litterbox/config.local.toml`. AC-002.
2. **`binding = "bound"`.** Refused: `the configured workspace binding 'bound' is not one this VCS can honour; ... or select a VCS that binds`. AC-004 for the VCS side, since Git cannot bind.
3. **`binding = "copied"`.** Created, `status: Active`, `workspace: /src`. The record on disk carries `binding = "copied"`.
4. **No VCS data.** Inside the sandbox, `/src` lists `README.md` and no hidden entries, and `test ! -e /src/.git` succeeds. AC-001 for `Copied`.
5. **Legacy record.** Removing the `binding` line from `demo.toml` leaves a record that still loads, and `litterbox list` reports `demo active bubblewrap`. AC-003.
6. **Delete.** Removes the record, the state directory and the branch: `removed record ...; removed state ...; removed branch litterbox/demo`. Nothing is left in the records directory.
7. **Prune.** No bound sandbox can exist yet, because no VCS binds, so the orphan sweep has no case to exercise end to end. The call condition is covered by `delete_prunes_for_a_bound_record_even_when_it_is_not_remote` in the orchestrator tests.

`AC-001` for `Bound` is covered by the two live seam tests rather than by this smoke, because `create` refuses `Bound` for Git. Masking itself is also proven by T-013 and T-015 and by the backend rendering and bind tests.

## Notes

- `bubblewrap` exec initially appeared broken. The cause was a command path that was not among the sandbox's mounts; running the bound shell from the store path works and the command exits 0.
- Deleting the bubblewrap sandbox logs `the scope unit could not be retired` twice. The teardown still removes the record, the state directory and the branch, and this warning is not specific to the binding work.
