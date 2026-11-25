# T-017 — Seatbelt masks a bound workspace by deny rule

**Question:** FR-013 says the Seatbelt renderer emits a deny layer after the allow rules, so a `Bound` workspace keeps the metadata entry visible but refuses every access under it, and descendants inherit the refusal. Does that hold on a real macOS host? Does the renderer put the deny after the allow? Is `Copied` unchanged?

**Answer:** Yes on macOS 27.2.1 (arm64). A `Bound` sandbox created through `SeatbeltBackend` with `.jj` masked reads and writes the visible workspace normally. `.jj` appears in `ls -a`, but `cat`, `ls`, `stat` and a redirect into it each exit `1` with `Operation not permitted` (`EPERM`), including from a descendant shell. The host `.jj/secret.txt` is untouched and no write reaches the host. The deny blocks follow every allow block in the rendered profile, and a `Copied` create still works with no deny layer.

The implementation landed in `b05a0e69` (backend and policy) and `4117e487` (spec). This evidence closes the host check the task asked for.

## Reproduction

The probe builds the helper, then runs four focused tests on this host. Run from anywhere:

```sh
bash adrs/2026-11-21-hide-vcs-metadata-from-sandboxes/evidence/T-017/probe.sh
```

Raw output: [`output.txt`](output.txt).

The probe exits non-zero if a test fails or a filter matches anything other than exactly one test. It uses the same helper the backend uses, resolved from `target/debug/litterbox-seatbelt-helper`.

## Key observations

From inside the `Bound` sandbox, with `.jj` as the masked path and `.jj/secret.txt` containing `top secret`:

| Operation | Exit | Result |
|-----------|------|--------|
| `cat visible.txt` | 0 | `hello` |
| `ls -a` | 0 | `.jj` is present in the listing |
| `cat .jj/secret.txt` | 1 | `Operation not permitted`, empty stdout |
| `ls .jj` | 1 | `Operation not permitted` |
| `stat .jj` | 1 | `Operation not permitted` |
| `stat .jj/secret.txt` | 1 | `Operation not permitted` |
| `echo evil > .jj/evil.txt` | 1 | `Operation not permitted`, no host file |
| `/bin/sh -c 'cat .jj/secret.txt'` | 1 | `Operation not permitted` |

The renderer test `a_masked_path_is_denied_after_the_allows` asserts both deny blocks are positioned after `(allow file-write*`, and that the masked path reaches the parameter table rather than the profile text.

## Implication

Seatbelt masking is deny-access masking, not the empty mountpoint bubblewrap and Docker leave: the entry remains and is visible, but it cannot be statted or traversed. This matches EC-001 of the spec and AC-006.

`Copied` is unchanged: `a_copied_workspace_renders_no_deny_layer` confirms a `.git` present under the source does not cause a deny layer unless the request carries a mask, and `create_exec_and_cleanup` confirms the copied create, exec and delete path still behaves as before.

The `tmp` entry in the listing is the sandbox's `TMPDIR` created inside the workspace by the environment merge; it is unrelated to masking.
