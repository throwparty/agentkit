# Linux Bubblewrap Lifecycle and Limits Implementation Plan

## Approach

Five milestones. M1 copies the bwrap spike's PoC unchanged and checks it in, so the extension has a clean diff. M2 runs the harness under a transient systemd user scope with --new-session. M3 freezes and thaws the scope. M4 applies memory and task limits and exercises a hog. M5 runs the process-group fallback and writes findings.

## Architecture

The isolation harness and argv builder stay unchanged. `lifecycle-cgroup.sh` starts `systemd-run --user --scope --unit=lbx-<pid>` around the harness and uses `systemctl --user freeze`, `thaw` and `stop` for lifecycle. `limits.sh` adds `MemoryMax`, `MemorySwapMax` and `TasksMax` properties. `lifecycle.sh`, copied from the spike, remains the fallback when the user manager is unavailable.

## Technologies

| Technology | Role |
|------------|------|
| systemd user manager | Transient scopes, cgroup freeze and resource properties |
| bubblewrap (bwrap) | Unchanged sandbox construction |
| systemctl --user | Freeze, thaw and stop the scope |

## Components

### poc/lifecycle-cgroup.sh

Cgroup lifecycle

Starts the harness in a named scope with --new-session, freezes it, thaws it and stops the scope. Covers FR-001, FR-002 and AC-001, AC-002.


### poc/limits.sh

Resource limits

Runs a memory hog and a thread hog under MemoryMax and TasksMax, asserting both fail. Covers FR-003 and AC-003, AC-004.


### poc/lifecycle.sh

Fallback lifecycle

The spike's process-group freeze, kept for hosts without a usable user manager. Covers FR-004, AC-005 and EC-001.


### findings.md

The deliverable

Records which path ran, the cgroup properties, the failure modes, the costs and the remaining risks. Covers FR-006.


## Data Flow

The launcher creates a transient scope, systemd places the harness and its descendants in the scope cgroup, and the harness builds the unchanged bwrap argv. Freeze and limits act on the cgroup; the process-group fallback signals the harness's group. Observations are the loop file size while frozen, the exit status of the hog, the scope's cgroup properties and the launcher cost.


## Deployment

None. The launchers are throwaway scripts under this ADR. They are removed with the ADR once the findings are accepted.

