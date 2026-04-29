# Ideas & Design Decisions

This document captures design ideas, alternative approaches considered, and rationale for decisions made during roadmap development.

## Process Supervision for Container Services

**Problem**: Agents need to control long-running services (web servers, databases, etc.) inside their sandboxes. Need a lightweight init system that allows start/stop/restart/status operations.

**Options Considered**:
1. **tini** - Minimal init that reaps zombies, widely used in containers. Too minimal for our needs (doesn't provide service management).
2. **s6** - Small supervision suite with powerful features. Potentially overkill but very robust.
3. **supervisord** - Python-based process manager with simple INI config. Easy to use, `supervisorctl` CLI for control.
4. **Custom wrapper** - Simple shell script managing named processes and signal forwarding. Minimal dependencies but more work to maintain.

**Decision**: Recommended **supervisord** as primary option
- Simple INI configuration format
- `supervisorctl` provides CLI interface that agents can use via `sandbox-service` tool
- Well-understood, widely deployed, good documentation
- Alternatives (s6, custom) noted for users with specific needs

## Container Filesystem is Throwaway

**Key Insight**: We don't trust agents to work unsupervised, but we also don't have time to review every action. Hence sandboxing.

**Architecture**:
- Agent works entirely inside container on a copy of the code
- Changes tracked via git commits (triggered by `sandbox-write` and `sandbox-exec`)
- On `litterbox apply`/`merge`, only changes visible in git diff are applied to host
- **No automatic sync** from container to host filesystem
- Container filesystem is disposable; only the git history matters

**Rationale**:
- Security: prevents agents from sneaking in files outside tracked changes
- Clarity: human reviews exactly what they're accepting (via `litterbox diff`)
- "Litter tray" metaphor: sometimes agent output is cat shit, so we review before accepting

## Named Ports with Dynamic Allocation

**Problem**: Multiple sandboxes running in parallel need port forwarding, but hardcoded ports would conflict.

**Solution**:
- Configuration specifies **named** ports (e.g., `backend`, `frontend`)
- On `sandbox-create`, litterbox assigns random unused ports
- Port numbers exposed to container as environment variables (e.g., `PORT_BACKEND=54321`)
- Each sandbox gets independent port mappings

**Benefits**:
- Parallel agent work without port conflicts
- Configuration stays simple (no need to manually assign port ranges)
- Services read port from env var, remain portable
- Future: allow configuring the host port range instead of the default

## Commit on Every Write/Exec

**Problem**: Need to preserve agent context across interruptions (model compaction, new messages), but don't want thousands of commits.

**Solution**:
- Every `sandbox-write` and `sandbox-exec` creates a commit
- Agents call `sandbox-milestone(message)` to squash history since last milestone
- Preserves linearity while avoiding commit explosion
- Humans can rebase/squash/rewrite after applying

**Rationale**:
- Can't use transactions (would require state in model across interruptions)
- Fine-grained commits enable recovery if agent session dies
- Milestone tool gives agents control over commit granularity
- Humans have final say via SCM tools after apply

## No Hidden Files for Agents

**Decision**: Hidden files (`.env`, `.git/*`, etc.) are excluded from `sandbox-read` with no option to include them.

**Rationale**:
- **Security**: Prevents leaking secrets from files like `.env`
- **Privacy**: Keeps internal tooling/config away from model training data
- **Performance**: Avoids massive reads (e.g., all of `node_modules`)
- Secrets are injected separately via system keychain integration

## Non-Destructive by Default

**Design Principle**: Never automatically delete user work.

**Implementations**:
- `sandbox-create` errors if name conflicts (doesn't replace existing sandbox)
- `sandbox-delete` not exposed to agents (humans only)
- No automated garbage collection
- `litterbox apply` keeps sandbox; use `litterbox merge` to apply+delete
- Explicit human action required for all destructive operations

**Rationale**: "Surprise destruction" is unacceptable. Disk is cheap, recovering lost work is expensive.

## Service Configuration Format

**Format**:
```toml
[[services]]
name = "backend"
command = ["npm", "run", "dev"]
signals = { stop = "SIGTERM", restart = "SIGHUP" }

[[services]]
name = "db"
command = ["postgres", "-D", "/data"]
signals = { stop = "SIGTERM" }
```

**Rationale**:
- Command as list of args (not string) prevents shell injection issues
- Named signals allow flexibility (some services need SIGHUP to reload, others use SIGUSR1)
- Dictionary structure maps cleanly to supervisord config

## Future: Custom Base Ref for Sandbox Creation

**Current**: `sandbox-create` always branches from HEAD

**Future**: Allow `sandbox-create(name, base_ref="some-branch")`

**Use Cases**:
- Experimenting with variations of the same change
- Multiple agents working from same starting point
- Branching from specific commit/tag for hotfixes

**Why Not Now**: Start simple, add complexity when needed

## Git Integration: Patch Shuttling

**Problem**: Agent makes changes inside container. How do we get those into the git branch on the host?

**Solution**: Use patch format (like `git format-patch`) over text streams
- Git runs **only on the host**, not in containers
- After `sandbox-write` or `sandbox-exec`, diff the container filesystem to see what changed
- Generate patch from that diff
- Stream patch text to host process
- Apply patch to host git repo and commit
- Container has no git, just files
- No bind mounts needed (security win)

**Similar to**: `git request-pull` workflow - serialize changes as patches, transport as text, apply on other side

**Benefits**:
- Clean separation between container and host
- No filesystem sharing reduces attack surface
- Patches are human-readable (easier debugging)
- Standard git format enables compatibility with existing tools
- Container stays simple (no git installation needed)

## MCP Transport

**Decision**: stdio only

**Rationale**: MCP server must run on host (to access host git repo, spawn containers, etc.), so stdio transport is natural fit. Agent runs litterbox MCP server as subprocess.

## Multi-Repo Support

**Feature**: `litterbox pause --all-repos` stops all sandboxes across all repositories

**Use Case**: Free up resources on laptop/workstation when stepping away. Multiple projects might have sandboxes running; this stops everything at once.

**Implementation Notes**: Requires tracking which repos have active sandboxes, likely via global state in `~/.config/litterbox/` or similar.
