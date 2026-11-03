# Linux Bubblewrap Lifecycle and Limits

**Status:** implemented **Created:** 2026-10-31 **Author:** adrian

The isolation spike proved the bwrap view but left two gaps: the sandbox shares the caller's session unless --new-session is set, and it has no resource controls. Evidence on NixOS, kernel 7.2.12, systemd user manager, cgroup v2: systemd-run --user --scope starts the harness; systemctl --user freeze and thaw stop and resume the workload; MemoryMax=64M and TasksMax=16 bound a 256 MiB allocation and a 64-thread hog. The copied lifecycle.sh is the isolation ADR's tested process-group path. A host without a usable user manager selects the fallback by checking systemctl --user show-environment; with XDG_RUNTIME_DIR=/nonexistent the launcher exits 69 and lifecycle.sh freezes, resumes and kills. Launcher cost: direct 23.8ms median, scope 47.5ms median. CPUQuota was not applied or tested.

## Problem

The isolation harness cannot create a separate session without losing group-based lifecycle control, and it cannot bound memory or task count. We need a lifecycle design that uses cgroups where available and falls back explicitly where they are not.

## Goals

- Run the sandbox in its own session by default
- Freeze and resume the sandbox through cgroup v2
- Apply per-sandbox memory and task limits
- Fall back to process-group signalling without systemd
- Reuse the existing harness and argv builder unchanged

## Non-goals

- Nix devShell activation or the environment allowlist
- Egress proxy or port forwarding
- macOS confinement
- A long-lived supervisor or daemon

## Functional Requirements

### FR-001: Session By Default

The harness runs with --new-session and lifecycle control moves to the cgroup.

**Slug:** `session-by-default`

### FR-002: Cgroup Freeze

A transient systemd user scope freezes and thaws the sandbox through systemctl --user freeze and thaw.

**Slug:** `cgroup-freeze`

### FR-003: Resource Limits

The scope applies MemoryMax and TasksMax, and a workload exceeding either fails rather than consuming the host.

**Slug:** `resource-limits`

### FR-004: Process Group Fallback

Without a usable user manager the copied lifecycle.sh runs the sandbox and uses process-group signals.

**Slug:** `process-group-fallback`

### FR-005: Shared Code

The isolation harness and argv builder are copied unchanged; only launchers and scripts differ.

**Slug:** `shared-code`

### FR-006: Tests

A freeze test, a memory-limit test and a task-limit test run from one command; the fallback script is the isolation ADR's tested path.

**Slug:** `tests`

## Non-functional Requirements

### NFR-001: Systemd Optional

Systemd is detected at runtime through systemctl --user show-environment; its absence selects the fallback.

**Slug:** `systemd-optional`

### NFR-002: No Supervisor

The scope is transient and owned by systemd; the harness gains no daemon.

**Slug:** `no-supervisor`

### NFR-003: Cost Recorded

The systemd launcher's startup cost is measured and recorded beside the isolation cost.

**Slug:** `cost-recorded`

## Acceptance Criteria

### AC-001: New Session

The sandboxed command runs in its own session under the scope.

**Slug:** `new-session`

### AC-002: Freeze And Thaw

Freezing the scope stops the workload making progress; thawing resumes it.

**Slug:** `freeze-and-resume`

### AC-003: Memory Limit

A workload exceeding MemoryMax fails instead of consuming host memory.

**Slug:** `memory-limit`

### AC-004: Task Limit

A workload exceeding TasksMax fails instead of spawning without bound.

**Slug:** `task-limit`

### AC-005: Fallback Script

The copied lifecycle.sh performs freeze, resume and kill through process groups, as recorded in the isolation ADR.

**Slug:** `fallback`

### AC-006: Cost Recorded

The launcher's startup cost is measured and recorded.

**Slug:** `cost-recorded`

## Edge Cases

### EC-001: Systemd Absent

The launcher detects the missing user manager and selects the fallback; exercised with XDG_RUNTIME_DIR=/nonexistent.

**Slug:** `systemd-absent`

### EC-002: Delegation Absent

A user manager without cgroup delegation is not detected up front: the scope start fails and the probe exits non-zero, leaving the tested process-group fallback to be selected by the caller. Accepted as an unverified host state.

**Slug:** `delegation-absent`

### EC-003: Scope Name Collision

Transient scope names include the pid and are collected on exit.

**Slug:** `scope-name-collision`
