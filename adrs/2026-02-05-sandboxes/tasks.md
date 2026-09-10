# Task List: Sandboxes Core Implementation

This task list defines the granular units of work for implementing the sandbox feature. All tasks reference `spec.md` and `plan.md`.

## Phase 1: Domain Models & Git Integration

### Task 1.1: Core Types & Error Mapping

- [x] Completed
- **Steps**:
  1. Define `SandboxConfig`, `ExecutionResult`, `SandboxStatus`, and `SandboxMetadata` structs in a dedicated `domain` module.
  1. Implement `Display` for these types to support CLI logging.
  1. Define a custom `Error` enum that maps `git2` and `bollard` errors to domain-specific failures.
- **Success Criteria**:
  - Code compiles with new types.
  - Error enum covers all failure modes identified in `spec.md` Section 6.
- **Test Requirements**: Unit tests for slugification and error string formatting.
- **Dependencies**: None.

### Task 1.2: Branch Management (Git Module)

- [x] Completed
- **Steps**:
  1. Initialize `git2::Repository`.
  1. Implement `create_branch(slug: &str) -> Result<String>`.
  1. Implement `delete_branch(name: &str) -> Result<()>`.
  1. Ensure existing branches are detected to prevent duplicates.
- **Success Criteria**:
  - Branches are created exactly as specified (slugified, descendants of `HEAD`).
  - `git2` errors are correctly wrapped in the domain `Error` type.
- **Test Requirements**: Integration tests creating and deleting branches in a temporary repo.
- **Dependencies**: Task 1.1.

### Task 1.3: Archive Generation (Git Module)

- [x] Completed
- **Steps**:
  1. Implement `get_archive(ref: &str) -> Result<Vec<u8>>`.
  1. Use `git2` to traverse the tree and package files into a tarball.
  1. Ensure `.gitignore` rules are respected during archiving.
- **Success Criteria**:
  - Generates a valid tarball of the current source code.
- **Test Requirements**: Verify tarball contents against expected repo state.
- **Dependencies**: Task 1.2.

## Phase 2: Docker Infrastructure

### Task 2.1: Bollard Client Setup & Image Management

- [x] Completed
- **Steps**:
  1. Initialize `bollard::Docker` client using Unix sockets.
  1. Implement an internal helper to pull `busybox:latest` if not present.
- **Success Criteria**:
  - Client successfully connects to local Docker daemon.
  - `busybox:latest` is available after initialization.
- **Test Requirements**: Connectivity test against Docker daemon.
- **Dependencies**: Task 1.1.

### Task 2.2: Container Provisioning (`create`)

- [x] Completed
- **Steps**:
  1. Implement `Sandbox::create`.
  1. Define container configuration (no bind mounts, specific root path).
  1. Use `bollard` to create and start the container.
- **Success Criteria**:
  - Container is created with the specified image and isolated settings.
  - Container ID is captured and stored in metadata.
- **Test Requirements**: Verify container exists and is running via `docker ps`.
- **Dependencies**: Task 1.2, Task 2.1.

### Task 2.3: State Management (`pause`, `resume`, `delete`)

- **Steps**:
  1. Implement `Sandbox::pause` using `bollard::container::PauseContainer`.
  1. Implement `Sandbox::resume` using `bollard::container::UnpauseContainer`.
  1. Implement `Sandbox::delete` using `bollard::container::RemoveContainer`.
  1. Ensure `delete` handles both running and paused containers.
- **Success Criteria**:
  - `pause`/`resume` toggle state without data loss.
  - `delete` removes all traces of the container.
- **Test Requirements**: Verify status transitions in integration tests.
- **Dependencies**: Task 2.2.

### Task 2.4: Execution Engine (`shell`)

- **Steps**:
  1. Implement `Sandbox::shell` using `bollard::exec::CreateExec` and `StartExec`.
  1. Capture stdout, stderr, and the exit code.
  1. Map output to `ExecutionResult`.
- **Success Criteria**:
  - Commands run isolated within the container.
  - Exit codes are correctly captured (e.g., `ls` in non-existent dir returns `2`).
- **Test Requirements**: Run `echo`, `ls`, and failing commands; assert results.
- **Dependencies**: Task 2.2.

### Task 2.5: File Streaming (`upload`, `download`)

- [x] Completed
- **Steps**:
  1. Implement `Sandbox::upload` using `bollard::container::UploadToContainer`.
  1. Implement `Sandbox::download` using `bollard::container::DownloadFromContainer`.
  1. Integrate the tarball generation from Task 1.3 for initial project copy.
- **Success Criteria**:
  - Files are correctly transferred between host and container.
- **Test Requirements**: Upload a file, modify it in container, download it, verify content.
- **Dependencies**: Task 1.3, Task 2.2.

## Phase 3: User Interfaces

### Task 3.1: CLI Command Handlers

- [x] Completed
- **Steps**:
  1. Add `pause`, `resume`, `delete`, and `shell` subcommands to the existing CLI.
  1. Bind subcommands to the Sandbox Interface methods.
- **Success Criteria**:
  - `litterbox pause <NAME>` triggers the domain `pause` logic.
  - `litterbox resume <NAME>` triggers the domain `resume` logic.
- **Test Requirements**: Mock CLI calls and verify interaction with `Sandbox` trait.
- **Dependencies**: Phase 2.

### Task 3.2: CLI Output Formatting & Error Handling

- [x] Completed
- **Steps**:
  1. Implement pretty-printing for `ExecutionResult` and `SandboxMetadata`.
  1. Ensure all errors are printed to stderr with clear context.
- **Success Criteria**:
  - Success messages are concise; error messages provide actionable info.
- **Test Requirements**: Capture stderr in E2E tests.
- **Dependencies**: Task 3.1.

### Task 3.3: MCP Tool Registration & Request Dispatch

- [x] Completed
- **Steps**:
  1. Register `sandbox-create` tool in the MCP server.
  1. Implement request handler that maps JSON parameters to `SandboxConfig`.
  1. Dispatch to `Sandbox::create`.
- **Success Criteria**:
  - MCP server exposes the tool to agents.
- **Test Requirements**: Use an MCP client to trigger creation.
- **Dependencies**: Phase 2.

### Task 3.4: MCP Result Mapping

- [x] Completed
- **Steps**:
  1. Map `SandboxMetadata` and `ExecutionResult` back to MCP-compatible JSON responses.
  1. Ensure domain errors are mapped to MCP protocol error codes.
- **Success Criteria**:
  - Agents receive structured success/error responses.
- **Test Requirements**: Verify JSON payloads in MCP tests.
- **Dependencies**: Task 3.3.
