//! Prose the generated reference pages share.
//!
//! `docgen` rewrites `reference/cli.mdx` and `reference/mcp.mdx` from the code,
//! so a hand edit to either file is lost the next time they are generated. The
//! sandbox lifecycle is a property of the product rather than of the clap or
//! MCP schema, so it cannot be derived; it lives here once and is appended to
//! both pages.
//!
//! The prose mirrors `docs/docs/user/litterbox/lifecycle.mdx`, which carries the
//! state diagram. Changing one without the other makes the reference disagree
//! with the explanation, so the two are reviewed together.

/// The lifecycle section appended to the generated CLI reference.
pub fn cli_section() -> String {
    let mut section = String::new();
    section.push_str(HEADER);
    section.push_str(STATES);
    section.push_str(
        "\n### Commands by compute state\n\n\
         | Command | `active` | `paused` | `missing` |\n\
         | --- | --- | --- | --- |\n\
         | `list` | reported as `active` | reported as `paused` | reported as `missing`; the listing exits 0 |\n\
         | `shell` | runs the command | exits 4, `conflict`, and names the resume command | repairs the sandbox from the sandbox branch, warns on stderr, then runs the command |\n\
         | `pause` | freezes the sandbox | succeeds, changing nothing | exits 3, not found |\n\
         | `resume` | succeeds, changing nothing | thaws the sandbox | exits 3, not found |\n\
         | `status` | reports the source state | reports the source state | reports the source state; a repair is not attempted |\n\
         | `reconcile` | stages, finishes or aborts the host merge | unchanged; it does not join | unchanged |\n\
         | `delete` | refused without `--ignore-running` | removes the record and branches; `--ignore-unmerged` when work would be lost | removes the record and branches; `--ignore-unmerged` when work would be lost |\n\
         \n\
         ### Commands by source state\n\n\
         | Command | `clean` | `diverged` | `resolving` |\n\
         | --- | --- | --- | --- |\n\
         | `list` | `clean` | `diverged` | `resolving` |\n\
         | `shell` | runs; a snapshot advances the shared branch | runs; a snapshot stays on the sandbox branch | runs; edits are the resolution |\n\
         | `pause`, `resume` | compute only; the source state is unchanged | unchanged | unchanged |\n\
         | `status` | clean | diverged, with the reason | resolving, with the conflicts |\n\
         | `delete` | no unmerged flag | `--ignore-unmerged` | `--ignore-unmerged` |\n\
         \n\
         Creating a sandbox is only available over MCP. There is no `create` command.\n\
         \n\
         A missing sandbox is repaired on the next command that needs live compute. If the\n\
         workspace is gone, repair materialises the sandbox branch first and reports\n\
         `source: sandbox`. When that branch is absent it falls back to the shared branch and\n\
         reports `source: shared` with the missing ref and the Git `fsck`/`update-ref` or jj\n\
         op-log/workspace-add recovery. An open resolution whose sandbox\n\
         branch is gone is refused with the recorded `synced`/`ours` and the Git or jj\n\
         recovery; `status` prints the same continuation with `sandbox absent`.\n\
         \n\
         `delete` has one flag per guard: `--ignore-running` for live compute,\n\
         `--ignore-invalid` for a record or status that cannot be read, and\n\
         `--ignore-unmerged` for a sandbox branch, resolution or staging workspace that may\n\
         hold work the shared branch does not. A refusal names every flag the record needs;\n\
         a flag whose condition does not hold is reported as unused.\n",
    );
    section.push_str(LINK);
    section
}

/// The lifecycle section appended to the generated MCP reference.
pub fn mcp_section() -> String {
    let mut section = String::new();
    section.push_str(HEADER);
    section.push_str(STATES);
    section.push_str(
        "\n### Tools by compute state\n\n\
         | Tool | `active` | `paused` | `missing` |\n\
         | --- | --- | --- | --- |\n\
         | `bash`, `read`, `write`, `patch`, `ls`, `glob`, `grep` | runs | invalid params, `conflict`, naming the resume command | repairs the sandbox, appends a recovery block, then runs |\n\
         | `sandbox-ports` | the recorded ports | the recorded ports | the recorded ports |\n\
         | `sandbox-create` | conflict | conflict | repairs the sandbox and reports the recovery |\n\
         | `sandbox-status` | reports the source state | reports the source state | reports the source state |\n\
         | `sandbox-reconcile` | begins, finishes or abandons a resolution | invalid params, `conflict`, naming the resume command | repairs, then begins, finishes or abandons |\n\
         \n\
         ### Tools by source state\n\n\
         | Tool | `clean` | `diverged` | `resolving` |\n\
         | --- | --- | --- | --- |\n\
         | command tools | runs; a snapshot advances the shared branch | runs; a snapshot stays on the sandbox branch | runs; edits are the resolution |\n\
         | `sandbox-status` | clean | diverged, with the reason | resolving, with the conflicts |\n\
         | `sandbox-reconcile` | begin fails; there is nothing to resolve | begins when the divergence conflicts and the shared tip is clean; fails otherwise | begin fails; finish and abandon close it |\n\
         | `sandbox-create` on a missing name | repair lands clean | repair keeps the divergence | repair keeps the resolution |\n\
         \n\
         `sandbox-create` on an active, paused, diverged or resolving name is a conflict. On a\n\
         missing name it repairs the compute and reports the recovery. `sandbox-ports` answers\n\
         from the record, so the same ports come back whether or not the compute is alive.\n\
         \n\
         A paused command or `sandbox-reconcile` action is refused before any join, so the\n\
         tool returns immediately. `sandbox-status` does not join and still answers.\n\
         \n\
         When only the sandbox revision is absent and a resolution is open, `sandbox-status`\n\
         returns `resolving`, `sandbox: null`, the recorded `synced`/`ours` and a `next` that\n\
         names the Git reflog/fsck or jj op-log recovery and the delete fallback.\n\
         \n\
         When repair falls back to the shared branch, its recovery block's `fallback` names the\n\
         missing ref and the Git `fsck`/`update-ref` or jj op-log/workspace-add recovery.\n",
    );
    section.push_str(LINK);
    section
}

const HEADER: &str = "\n## Sandbox lifecycle\n\n";

const STATES: &str = "\
A sandbox has a compute state and a source state. The compute state is `active`,\n\
`paused` or `missing`; it describes the process. The source state is `clean`,\n\
`diverged` or `resolving`; it describes the branches. `diverged` means the shared\n\
branch advance is held because the branch moved under unsaved work or the shared tip\n\
carries a conflict. `resolving` means a `sandbox-reconcile` resolution is open.\n\
Neither source state refuses a tool: only the branch advance waits.\n";

const LINK: &str = "\nSee [Sandbox lifecycle](../lifecycle) for the state diagram, the failure classes and \
                    how to recover a missing sandbox.\n";

#[cfg(test)]
mod tests {
    use super::{cli_section, mcp_section};

    #[test]
    fn both_sections_name_every_state() {
        for section in [cli_section(), mcp_section()] {
            for state in [
                "`active`",
                "`paused`",
                "`missing`",
                "`clean`",
                "`diverged`",
                "`resolving`",
            ] {
                assert!(section.contains(state), "missing {state} in {section}");
            }
            assert!(section.contains("../lifecycle"), "{section}");
        }
    }

    #[test]
    fn the_cli_section_names_the_delete_flags() {
        let section = cli_section();
        for flag in [
            "`--ignore-running`",
            "`--ignore-invalid`",
            "`--ignore-unmerged`",
        ] {
            assert!(section.contains(flag), "missing {flag} in {section}");
        }
    }

    #[test]
    fn each_section_names_the_surface_it_documents() {
        assert!(cli_section().contains("| `shell` |"));
        assert!(!cli_section().contains("`sandbox-create`"));
        assert!(mcp_section().contains("`sandbox-create`"));
        assert!(!mcp_section().contains("| `shell` |"));
        assert!(mcp_section().contains("`sandbox-status`"));
        assert!(mcp_section().contains("`sandbox-reconcile`"));
    }
}
