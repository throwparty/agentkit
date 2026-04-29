# Roadmap

## Core Infrastructure
- Git branches for tracking agent changes (git, jj, etc. support planned)
- Host directories (`.litterbox/worktrees/{name}`) store committed state only
- Container-based sandboxes (Docker initially, with extensibility for other runtimes)
- Code copied into container; agent works on copy (never touches host filesystem directly)
- **No automatic sync**: container filesystem is throwaway; only changes visible in git diff are applied on merge
- Container image contains dependencies; tool versions managed via container image
- SSH agent forwarding with permission prompting
- Configuration via `.litterbox.toml` at repository root (no inheritance, keep simple)
  - `container.base-image`: Specifies Docker image for sandboxes
  - `container.setup-command`: List of command args to run once on container creation
  - `container.services`: Dictionary of named services → `{command = ["cmd", "arg1"], signals = {stop = "SIGTERM", restart = "SIGHUP"}}`
  - `container.ports`: Named ports for forwarding (random unused ports assigned on start, exposed as env vars)

## Security & Isolation
- Network isolation and policy controls (default: deny all, user can enable as needed)
- Filesystem isolation (read-only system mounts, tmpfs for ephemeral storage)
- Resource limits (CPU, memory, disk I/O, storage quotas)
- Secrets management (retrieve from system keychain, inject without exposing to model)

## MCP Tools for Agents
- `sandbox-create(name)`: Create a sandbox environment (git branch, host directory, container) based on HEAD. Name is slugified (non-alphanumeric → dashes, consecutive dashes → single dash). Errors if sandbox with that name already exists
- `sandbox-read(path)`: Read a file from the sandbox container filesystem. Hidden files excluded (security/privacy)
- `sandbox-write(path, content)`: Write a file to the sandbox container filesystem. Changes visible in git diff will be applied on merge
- `sandbox-exec(command, message)`: Execute arbitrary command in sandbox container. Changes to tracked files committed to git branch. Message as first line, command in body. Returns stdout, stderr, and exit code
- `sandbox-service(action, service)`: Control named services (start/stop/restart/status) via process supervisor
- `sandbox-milestone(message)`: Squash all commits since last milestone into one commit with provided message. Required parameter; error if message not provided
- Audit logging of all MCP tool invocations

**Note:** `sandbox-delete` is intentionally not exposed to agents to prevent destructive actions. Cleanup is managed by humans via CLI.

## CLI for Human Users
- `litterbox diff ENV`: Print current delta between base ref and environment state
- `litterbox list`: Enumerate all current environments
- `litterbox apply ENV [--args...]`: Merge changes from environment onto HEAD. Respects SCM configuration (git config merge strategy, user/machine settings). Accepts pass-through args for SCM-specific options. Sandbox persists after apply
- `litterbox merge ENV [--args...]`: Merge changes from environment onto HEAD and delete the sandbox
- `litterbox delete ENV`: Delete sandbox and reject changes
- `litterbox pause [--all-envs] [--all-repos]`: Gracefully pause containers (one env, all envs in repo, or all envs across all repos)
- `litterbox shell ENV`: Get interactive shell into environment

## Developer Experience
- Named port forwarding with dynamic allocation (random unused ports assigned on start, exposed as env vars to container)
- Each sandbox gets independent port mappings to support parallel agent work
- Shell access for manual inspection and debugging (via `litterbox shell`)
- Container lifecycle: setup command runs once on creation, services managed via lightweight process supervisor
- Graceful pause/resume of containers to conserve resources

## Observability
- Real-time monitoring of agent actions
- Cost tracking (token usage, compute time)
- Performance metrics per sandbox
- Event streaming for external integrations

## Cleanup & Maintenance
- Disk space management
- Configurable retention policies

## Workflow & Semantics

### Commit Granularity
- Every `sandbox-write` creates a commit to preserve context across interruptions (compaction, new messages)
- Every `sandbox-exec` creates a commit after execution (even if no changes)
- Commit format: agent-provided message as first line, executed command in body
- Agents call `sandbox-milestone(message)` when finishing a task to squash history since last milestone
- Agent must provide commit message; tool errors if message not provided
- This preserves linearity while avoiding excessive commit volume
- Users can rewrite commit messages after `litterbox apply` using their SCM tools

### Shell/Command Execution
- Agents have no access to host system (developer workstation)
- Agents have full execution access to their sandbox container via `sandbox-exec(command, message)`
- Agent can control named services via `sandbox-service(action, service)` (start/stop/restart/status)
- After command execution: commit tracked file changes to git branch, return stdout/stderr/exit code
- Non-zero exit codes don't prevent commits; agent receives error details and can respond

### State Visibility
- `sandbox-read` reads files from container filesystem (not host)
- Agent works entirely on container copy; never touches host filesystem directly
- **Container filesystem is throwaway**: only changes visible in git diff are applied on merge
- No automatic sync from container to host
- Hidden files excluded from agent visibility (security/privacy)
- No configuration for including hidden files

### Container Lifecycle
- Container created on `sandbox-create` and persists until `litterbox delete`
- Setup command runs once on container creation
- Services managed by lightweight process supervisor
  - **Recommended**: supervisord (simple INI config, `supervisorctl` interface for agents)
  - **Alternatives**: s6 (small supervision suite), custom wrapper
- Can be gracefully paused via `litterbox pause` to conserve resources
- Port mappings: named ports in config get random unused ports on start, exposed as environment variables to container

### Sandbox Naming & Conflicts
- `sandbox-create(name)` errors if sandbox with that name already exists
- Non-destructive: existing sandboxes are never automatically deleted/replaced
- Agent must choose a different name to create new sandbox

### Branch Strategy
- Each `sandbox-create(name)` creates a new branch based on HEAD with slugified name
- Slugification: non-alphanumeric characters → dashes, consecutive dashes → single dash
- Changes merge into development branch when human runs `litterbox apply` or `litterbox merge`
- Long-term future: Allow specifying custom base ref for `sandbox-create` (enables multiple worktrees from same branch/commit)

### Apply & Cleanup Semantics
- `litterbox apply ENV` merges sandbox branch onto HEAD via git diff (only tracked file changes), sandbox persists
- `litterbox merge ENV` merges via git diff and deletes the sandbox
- `litterbox delete ENV` deletes sandbox and rejects changes
- No automated garbage collection (too complex, risk of surprise destruction)
- Respects SCM configuration (git config merge strategy order: repo → user → machine)
- Accepts pass-through args for SCM-specific options (e.g., `--strategy=ours`)

### Cleanup Timing
- `sandbox-delete` not exposed to agents (too destructive)
- Humans manage cleanup via CLI
- No automated garbage collection
- Sandboxes persist until explicitly deleted by human

### Multi-Agent Collaboration
- Multiple agents can work on same repository (same host codebase)
- Each agent gets its own isolated sandbox environment
- One agent per sandbox (no concurrent agent access to same sandbox)
