# ADR 2026-02-07-mcp-tools: Task Breakdown

This document outlines the implementable tasks for the `mcp-tools` feature, derived from the `spec.md` and `plan.md` documents. Each task is designed to be independently implementable, with clear acceptance criteria, test requirements, and references to the relevant specification sections.

## Phase 1: Navigation & Search Tools

### [x] Task 1.1: Implement `read` Tool Handler

- **Description:** Develop the MCP tool handler for `read`, allowing agents to read file content from the sandbox.
- **Acceptance Criteria:**
- `read(sandbox: string, path: string)` returns the full content of a file inside the sandbox container.
- `read(sandbox: string, path: string, offset: number, limit: number)` returns the specified portion of a file.
- Relative paths are resolved against `/src` inside the container.
- Handles "file not found" errors gracefully.
- **Test Requirements:**
  - Unit tests for reading existing files (full and partial).
- Unit tests for error cases (non-existent file, invalid path, missing sandbox).
- **References:** `spec.md` Section 5.2, `plan.md` Section 3.2 (MCP Tool Handlers).

### [x] Task 1.2: Implement `ls` Tool Handler

- **Description:** Develop the MCP tool handler for `ls`, allowing agents to list directory contents in the sandbox.
- **Acceptance Criteria:**
- `ls(sandbox: string, path: string)` returns a list of files and directories in the specified path inside the sandbox.
- `ls(sandbox: string, path: string, recursive: true)` returns a recursive listing.
- Relative paths are resolved against `/src` inside the container.
- Handles "directory not found" errors gracefully.
- **Test Requirements:**
  - Unit tests for listing various directories (empty, with files, with subdirectories).
  - Unit tests for recursive listing.
- Unit tests for error cases (non-existent directory, missing sandbox).
- **References:** `spec.md` Section 5.2, `plan.md` Section 3.2 (MCP Tool Handlers).

### [x] Task 1.3: Implement `glob` Tool Handler

- **Description:** Develop the MCP tool handler for `glob`, allowing agents to find files matching a pattern in the sandbox.
- **Acceptance Criteria:**
- `glob(sandbox: string, pattern: string)` returns a list of file paths matching the pattern inside the sandbox.
- Relative paths are resolved against `/src` inside the container.
  - Handles various glob patterns (wildcards, character sets).
- **Test Requirements:**
  - Unit tests for various glob patterns and matching files.
- Unit tests for patterns with no matches.
- Unit tests for missing sandbox.
- **References:** `spec.md` Section 5.2, `plan.md` Section 3.2 (MCP Tool Handlers).

### [x] Task 1.4: Implement `grep` Tool Handler

- **Description:** Develop the MCP tool handler for `grep`, allowing agents to search for patterns within file contents in the sandbox.
- **Acceptance Criteria:**
- `grep(sandbox: string, pattern: string, path: string)` returns lines matching the pattern in files within the specified path inside the sandbox.
- `grep(sandbox: string, pattern: string, path: string, include: string)` filters files by the include pattern.
- Relative paths are resolved against `/src` inside the container.
- Handles "pattern not found" and "path not found" errors gracefully.
- **Test Requirements:**
  - Unit tests for searching various patterns in files.
  - Unit tests for `include` filtering.
- Unit tests for error cases (non-existent path, no matches, missing sandbox).
- **References:** `spec.md` Section 5.2, `plan.md` Section 3.2 (MCP Tool Handlers).

## Phase 2: Core File System Mutation Tools

### [x] Task 2.1: Implement `write` Tool Handler

- **Description:** Develop the MCP tool handler for `write`, allowing agents to write content to files in the sandbox.
- **Acceptance Criteria:**
- `write(sandbox: string, path: string, content: string)` creates a new file or overwrites an existing one inside the sandbox.
- Relative paths are resolved against `/src` inside the container.
  - The written content matches the provided content exactly.
  - Handles permission errors gracefully.
- **Test Requirements:**
  - Unit tests for creating new files.
  - Unit tests for overwriting existing files.
- Unit tests for permission denied errors and missing sandbox.
- **References:** `spec.md` Section 5.1, `plan.md` Section 3.2 (MCP Tool Handlers).

### [x] Task 2.2: Implement `patch` Tool Handler

- **Description:** Develop the MCP tool handler for `patch`, allowing agents to apply diffs to files in the sandbox.
- **Acceptance Criteria:**
- `patch(sandbox: string, path: string, diff: string)` successfully applies a valid unified diff to the specified file inside the sandbox.
- Relative paths are resolved against `/src` inside the container.
  - The file content reflects the changes described by the diff.
  - Handles "file not found" and "invalid diff" errors gracefully.
- **Test Requirements:**
  - Unit tests for applying valid diffs (additions, deletions, modifications).
- Unit tests for error cases (non-existent file, malformed diff, missing sandbox).
- **References:** `spec.md` Section 5.1, `plan.md` Section 3.2 (MCP Tool Handlers).

## Phase 3: Command Execution Tool

### [x] Task 3.1: Implement `bash` Tool Handler

- **Description:** Develop the MCP tool handler for `bash`, allowing agents to execute shell commands within the sandbox.
- **Acceptance Criteria:**
- `bash(sandbox: string, command: string)` executes the command inside the sandbox and returns `stdout`, `stderr`, and `exitCode`.
- `bash(sandbox: string, command: string, workdir: string)` executes the command in the specified working directory inside the sandbox.
- `bash(sandbox: string, command: string, timeout: number)` terminates the command if it exceeds the timeout.
  - Handles non-zero exit codes and command execution errors gracefully.
- **Test Requirements:**
  - Unit tests for successful command execution (various commands).
  - Unit tests for commands with non-zero exit codes.
  - Unit tests for `workdir` functionality.
- Unit tests for `timeout` functionality and missing sandbox.
- **References:** `spec.md` Section 5.1, `plan.md` Section 3.2 (MCP Tool Handlers).

## Phase 4: Auto-Snapshotting Mechanism

### [x] Task 4.1: Implement SCM Integration Module (Staging & Committing)

- **Description:** Develop the core logic within the SCM Integration Module for staging changes and creating Git commits using `git2`.
- **Acceptance Criteria:**
- The module can detect all changes (added, modified, deleted) in the mounted SCM repository.
- The module can stage all detected changes.
- The module can create a Git commit with staged changes when `git status` reports changes.
- The module does not create empty commits (no `--allow-empty`).
- **Test Requirements:**
  - Unit tests for detecting changes in a mock repository.
  - Unit tests for staging changes.
  - Unit tests for creating commits.
- **References:** `plan.md` Section 3.2 (SCM Integration Module), `plan.md` Section 2 (Technology Stack Justification - `git2`).

### [x] Task 4.2: Integrate Auto-Snapshot Triggering with Mutation Tools

- **Description:** Modify the `write`, `patch`, and `bash` tool handlers to trigger the SCM Integration Module for a snapshot after successful execution.
- **Acceptance Criteria:**
- After a successful `write` operation, an SCM snapshot is automatically created.
- After a successful `patch` operation, an SCM snapshot is automatically created.
- After a successful `bash` command that modifies the filesystem, an SCM snapshot is automatically created.
- Snapshots are only created when `git status` reports changes.
- The MCP Server does not proceed to execute another tool until the snapshot has completed.
- **Test Requirements:**
  - Integration tests: Execute `write`, `patch`, `bash` and verify the presence of a new SCM commit.
- **References:** `spec.md` Section 5.3, `plan.md` Section 3.2 (SCM Integration Module - Snapshot Creation).

### [x] Task 4.3: Implement Commit Message Generation

- **Description:** Develop the logic for generating standardized commit messages for auto-snapshots.
- **Acceptance Criteria:**
- Commit messages encode the trigger (e.g., `bash: cargo test`, `write: path`, `patch: path`).
- **Test Requirements:**
- Unit tests for commit message generation.
- **References:** `plan.md` Section 3.2 (SCM Integration Module - Commit Message Generation).

### [x] Task 4.4: Implement Branch Management for Snapshots

- **Description:** Implement the strategy for managing agent-generated snapshots on a dedicated branch or similar mechanism to avoid polluting the main development branch.
- **Acceptance Criteria:**
  - Snapshots are committed to a dedicated branch (e.g., `litterbox-snapshots`).
  - The main development branch remains unaffected by agent snapshots.
- **Test Requirements:**
  - Integration tests: Verify commits appear on the correct branch.
- **References:** `plan.md` Section 3.2 (SCM Integration Module - Branch Management).

## Phase 5: Testing & Error Handling

### [x] Task 5.1: Implement Comprehensive Unit Tests

- **Description:** Write thorough unit tests for all individual components and tool handlers.
- **Acceptance Criteria:**
  - Each function/method has adequate test coverage.
  - All unit tests pass.
- **Test Requirements:** N/A (this task is about writing tests).
- **References:** `plan.md` Section 5 (Unit Tests).

### [x] Task 5.2: Implement Integration Tests

- **Description:** Develop integration tests to verify the interactions between different components (Agent-MCP, MCP-Sandbox, Sandbox-SCM).
- **Acceptance Criteria:**
  - Key integration points are tested.
  - All integration tests pass.
- **Test Requirements:** N/A (this task is about writing tests).
- **References:** `plan.md` Section 5 (Integration Tests).

### [x] Task 5.3: Implement End-to-End Tests for Auto-Snapshotting

- **Description:** Create end-to-end tests that simulate an agent's full workflow, including file modifications and `bash` commands, and verify the resulting SCM snapshots.
- **Acceptance Criteria:**
  - A simulated agent workflow successfully triggers and verifies SCM snapshots.
  - All end-to-end tests pass.
- **Test Requirements:** N/A (this task is about writing tests).
- **References:** `plan.md` Section 5 (End-to-End Tests).

### [x] Task 5.4: Implement Robust Error Handling

- **Description:** Implement comprehensive error handling mechanisms across all MCP tools and internal components, ensuring clear error messages and graceful failure.
- **Acceptance Criteria:**
  - All defined edge cases (e.g., file not found, invalid diff, command timeout) are handled.
  - Clear error messages are returned to the agent.
  - The MCP Server remains stable under error conditions.
- **Test Requirements:**
  - Unit and integration tests specifically targeting error conditions and expected error responses.
- **References:** `spec.md` Section 8, `plan.md` Section 6 (Error Reporting).
