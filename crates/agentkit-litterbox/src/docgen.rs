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
        "\n### Commands\n\n\
         | Command | `active` | `paused` | `missing` |\n\
         | --- | --- | --- | --- |\n\
         | `list` | reported as `active` | reported as `paused` | reported as `missing`; the listing exits 0 |\n\
         | `shell` | runs the command | exits 4, `conflict`, and names the resume command | repairs the sandbox, warns on stderr, then runs the command |\n\
         | `pause` | freezes the sandbox | succeeds, changing nothing | exits 3, not found |\n\
         | `resume` | succeeds, changing nothing | thaws the sandbox | exits 3, not found |\n\
         | `delete` | refused without `--force` | removes the record and the branch | refused without `--force`; with it, removes the record, workspace and branch |\n\
         \n\
         Creating a sandbox is only available over MCP. There is no `create` command.\n\
         \n\
         A missing sandbox is repaired on the next command that needs live compute. The\n\
         repair reuses the workspace, keeps the branch, and is reported before the command's\n\
         output. A paused sandbox is refused before any join, so the caller returns\n\
         immediately instead of blocking.\n",
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
        "\n### Tools\n\n\
         | Tool | `active` | `paused` | `missing` |\n\
         | --- | --- | --- | --- |\n\
         | `bash`, `read`, `write`, `patch`, `ls`, `glob`, `grep` | runs | invalid params, `conflict`, naming the resume command | repairs the sandbox, appends a recovery block, then runs |\n\
         | `sandbox-ports` | the recorded ports | the recorded ports | the recorded ports |\n\
         | `sandbox-create` | conflict | conflict | repairs the sandbox and reports the recovery |\n\
         \n\
         `sandbox-create` on an active or paused name is a conflict. On a missing name it\n\
         repairs the compute and reports the recovery. `sandbox-ports` answers from the\n\
         record, so the same ports come back whether or not the compute is alive; a backend\n\
         that does not forward ports reports the operation as unsupported.\n\
         \n\
         A paused command is refused before any join, so the tool returns immediately. A\n\
         repaired command's result carries the normal content plus a structured recovery\n\
         block naming the sandbox, the state it came from and the branch.\n",
    );
    section.push_str(LINK);
    section
}

const HEADER: &str = "\n## Sandbox lifecycle\n\n";

const STATES: &str = "\
A sandbox is `active`, `paused` or `missing`. `active` and `paused` are derived from the\n\
sandbox's compute; `missing` means the record and the branch exist but the compute has\n\
exited. Every command that takes a sandbox expects `active`.\n";

const LINK: &str = "\nSee [Sandbox lifecycle](../lifecycle) for the state diagram, the failure classes and \
                    how to recover a missing sandbox.\n";

#[cfg(test)]
mod tests {
    use super::{cli_section, mcp_section};

    #[test]
    fn both_sections_name_the_three_states() {
        for section in [cli_section(), mcp_section()] {
            for state in ["`active`", "`paused`", "`missing`"] {
                assert!(section.contains(state), "missing {state} in {section}");
            }
            assert!(section.contains("../lifecycle"), "{section}");
        }
    }

    #[test]
    fn each_section_names_the_surface_it_documents() {
        assert!(cli_section().contains("| `shell` |"));
        assert!(!cli_section().contains("`sandbox-create`"));
        assert!(mcp_section().contains("`sandbox-create`"));
        assert!(!mcp_section().contains("| `shell` |"));
    }
}
