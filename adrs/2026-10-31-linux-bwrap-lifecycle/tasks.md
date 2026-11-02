# Linux Bubblewrap Lifecycle and Limits Tasks

## Tasks

### T-001: [x] Copy the baseline

Copy `adrs/2026-10-31-linux-bwrap-sandbox/poc` into this ADR unchanged and commit it before extending.


| Field | Value |
|-------|-------|
| Success Criteria | The copied tree is byte-identical to the isolation PoC by construction; the isolation ADR records its tests. |
| Complexity | 🟢 Low |
| Effort | 0.5h |
| Depends On |  |
| References | shared-code |

### T-002: [x] Cgroup launcher

Wrap the harness in `systemd-run --user --scope --unit=lbx-<pid>` with `--new-session`, and detect a missing user manager with `systemctl --user show-environment`.


| Field | Value |
|-------|-------|
| Success Criteria | The scope is active and the workload runs in its own session; the missing-manager path exits 69. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-001 |
| References | session-by-default,systemd-optional |

### T-003: [x] Freeze and thaw

Freeze the scope with `systemctl --user freeze` and resume with `systemctl --user thaw`; assert the loop file stops and restarts.


| Field | Value |
|-------|-------|
| Success Criteria | Frozen byte count is unchanged, thawed count grows, and the scope is stopped after thaw. `unfreeze` is not a systemd verb and is not used. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-002 |
| References | cgroup-freeze,freeze-and-resume |

### T-004: [x] Resource limits

Run a memory hog and a thread hog under `MemoryMax` and `TasksMax`; assert both fail. CPUQuota is not applied.


| Field | Value |
|-------|-------|
| Success Criteria | Both workloads exit non-zero and the host remains responsive. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-002 |
| References | resource-limits,memory-limit,task-limit |

### T-005: [x] Fallback script

Keep the copied `lifecycle.sh` as the process-group path. Selection was exercised with `XDG_RUNTIME_DIR=/nonexistent`: `lifecycle-cgroup.sh` exits 69 and `lifecycle.sh` freezes, resumes and kills.


| Field | Value |
|-------|-------|
| Success Criteria | The launcher selects the fallback and the script records freeze, resume and group kill. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-001 |
| References | process-group-fallback,fallback,systemd-absent |

### T-006: [x] Launcher cost

Measure the `systemd-run --user --scope` startup cost against the direct harness path.


| Field | Value |
|-------|-------|
| Success Criteria | Median and p95 recorded: direct 23.8/26.4ms, scope 47.5/56.2ms. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-002 |
| References | cost-recorded |

### T-007: [x] Findings

Record the cgroup properties, freeze results, limit results, costs and remaining risks.


| Field | Value |
|-------|-------|
| Success Criteria | Every verified acceptance criterion has an observation with the command and output. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-002,T-003,T-004,T-005 |
| References | tests |

