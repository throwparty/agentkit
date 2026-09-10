---

## status: accepted

# ADR: MCP-based Sandbox Mutation Tools

## 1. Context and Problem Statement

With sandboxing now implemented in Litterbox, we need to expose tools that allow agents to mutate the contents of the compute sandbox. These mutations must be persisted to the compute environment, while changes to source files must be persisted to the SCM history.

The goal is to provide a set of Model Control Protocol (MCP) tools that enable agents to manipulate files and execute commands, while automating the SCM persistence to keep the developer in the loop without burdening the agent with SCM management.

## 2. Goals and User Journeys

### Goals

- Enable agents to perform file operations (`read`, `write`, `patch`) and command execution (`bash`).
- Ensure all changes are persisted to the sandboxed compute environment.
- Automatically capture changes to the file tree in SCM history (e.g., via auto-snapshots).

* Maintain a clean, developer-manageable SCM history by separating agent-driven snapshots from meaningful developer commits.

### User Journey

1. **Agent starts work:** A developer assigns a task to an agent within a Litterbox sandbox.
1. **Agent modifies files:** The agent uses `read` to explore and `patch` to apply a bug fix.
1. **Automatic Persistence:** Litterbox detects the file changes and automatically creates a snapshot commit in the SCM history.
1. **Agent runs tests:** The agent executes `bash` to run the project's test suite.
1. **Developer reviews:** The developer inspects the auto-generated snapshots and stages the final changes into a meaningful commit.

## 3. Research into Existing Agent Harnesses

This research analyzes the callable tools provided to models by leading agent harnesses, specifically focusing on those that operate in local or sandboxed terminal environments.

### 3.1 Claude Code (Anthropic)

Claude Code [claude-code] provides a set of tools specifically designed for local development:

- **`bash`**: Runs shell commands in the local environment.
- **`read_file`**: Reads the content of a specific file.
- **`write_to_file`**: Overwrites a file with new content.
- **`edit_file`**: Applies changes using `old_str` and `new_str` parameters for exact string replacement.
- **`grep_file`**: Searches for patterns within files.
- **`glob_file`**: Lists files matching a glob pattern.
- **`ls`**: Lists directory contents.
- **`undo`**: Reverts the last file modification.

### 3.2 OpenAI Codex

OpenAI Codex [openai-codex] (specifically the `codex-cli` agent) utilizes a lightweight toolset for local terminal interaction:

- **`shell`**: Executes shell commands on the host system.
- **`read`**: Reads the content of files.
- **`write`**: Writes content to files, supporting both full overwrites and targeted edits.

### 3.3 OpenCode

OpenCode [opencode-tools] uses MCP natively to expose a variety of local and remote tools, emphasizing precise file manipulation:

- **`read` / `write`**: Standard file I/O for full content replacement.
- **`patch`**: Applies unified diffs to files for incremental changes.
- **`bash`**: Executes shell commands.
- **`grep` / `glob`**: File system search and navigation.

## 4. Proposed Toolset Comparison

This table focuses on the writing and mutation tools intended for implementation in `mcp-tools`.

| Category       | Proposed Tools | Claude Code     | OpenAI Codex | OpenCode |
| :------------- | :------------- | :-------------- | :----------- | :------- |
| **Execution**  | `bash`         | `bash`          | `shell`      | `bash`   |
| **Full Write** | `write`        | `write_to_file` | `write`      | `write`  |
| **Patching**   | `patch`        | `edit_file`     | `write`      | `patch`  |

## 5. Functional Requirements

### 5.1 Core Mutation Tools

- **`write(sandbox, path, content)`**: Replaces the entire content of a file inside the sandbox container. Suitable for new files or major overwrites. Paths may be absolute within the container or relative to the sandbox workdir.
- **`patch(sandbox, path, diff)`**: Applies a unified diff for the target path inside the sandbox container. The diff may create, modify, or delete the file at `path`. Reject invalid or non-applicable diffs with a clear error.
- **`bash(sandbox, command, workdir?, timeout?)`**: Executes an arbitrary shell command inside the sandbox container. `workdir` may be an absolute container path or relative to the sandbox workdir. Returns `stdout`, `stderr`, and `exitCode`.

### 5.2 Navigation and Search

- **`read(sandbox, path)`**: Returns file content from inside the sandbox container.
- **`read(sandbox, path, offset, limit)`**: Returns a line-based slice, where `offset` is the starting line index (0-based) and `limit` is the maximum number of lines.
- **`ls(sandbox, path, recursive?)`**, **`glob(sandbox, pattern, path?)`**: For directory exploration inside the sandbox. Paths may be absolute within the container or relative to the sandbox workdir.
- **`grep(sandbox, pattern, path, include?)`**: For content search inside the sandbox. Paths may be absolute within the container or relative to the sandbox workdir.

### 5.3 Sandbox Targeting and Path Resolution

- All tools MUST operate on files and processes inside a sandbox container (no host filesystem access).
- Each tool call MUST include a `sandbox` name that is resolved using the same slugification rules as `sandbox-create`.
- Relative paths are resolved against the sandbox workdir (`/src`); absolute paths are interpreted within the container filesystem.

### 5.4 Auto-Snapshotting (SCM Persistence)

- Litterbox MUST monitor the file system for changes after the execution of any mutation tool (`write`, `patch`) or `bash` command.
- If `git status` indicates changes, Litterbox MUST automatically create an SCM snapshot (git commit). It MUST NOT create empty commits (no `--allow-empty`).
- Snapshot commit messages MUST encode the trigger, e.g. `bash: cargo test`, `write: path`, `patch: path`.
- Snapshot commits MUST be written to a dedicated branch (e.g. `litterbox-snapshots`) to avoid polluting the main development branch.
- Agents MUST NOT have explicit SCM tools (e.g., `git commit`); SCM management is handled entirely by the host system.

## 6. Non-Functional Requirements

- **Idempotency:** Re-running a `patch` on the same state should be handled gracefully.
- **Security:** `bash` commands must be restricted by the sandbox's security policy.
- **Performance:** Auto-snapshots must be lightweight to avoid blocking agent execution.

## 7. Acceptance Criteria

- Agent can successfully fix a bug using `read` and `patch`.

* `bash` commands can execute build/test scripts within the sandbox.

- Every file modification by the agent is visible in the SCM history as a separate snapshot.

* The agent is unable to manually manipulate SCM history.

## 8. Edge Cases and Error Handling

- **Large Diffs:** `patch` must handle large or complex diffs without corruption.
- **Concurrent Access:** The system must prevent race conditions if multiple tools attempt to modify the same file.
- **Invalid Commands:** `bash` should return non-zero exit codes and `stderr` to the agent.
- **Missing Sandbox:** If the specified sandbox does not exist, tool calls should return a clear "sandbox not found" error.

## 9. Tool Interface Alignment

- `read` uses line-based `offset`/`limit` to match common agent tool conventions (OpenCode).
- `patch` accepts unified diffs and may create/delete files, matching typical agent patch tools.
- `bash` follows the OpenCode-style signature (`command`, optional `workdir`, optional `timeout`); Codex/Claude-style agents that omit `timeout` are supported via defaults.

[claude-code]: https://github.com/anthropics/claude-code
[openai-codex]: https://github.com/openai/codex
[opencode-tools]: https://opencode.ai/docs/tools
