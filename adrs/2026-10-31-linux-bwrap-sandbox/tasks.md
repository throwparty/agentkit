# Linux Bubblewrap Sandbox Spike Tasks

## Tasks

### T-001: [x] Scratch layout

Create `adrs/2026-10-31-linux-bwrap-sandbox/poc` with `bwrap` and `fixture` subdirectories. Each Cargo.toml carries an empty `[workspace]` table so neither crate joins the agentkit workspace, and no agentkit crate references them.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo metadata` at the repo root does not list either package; removing the ADR directory leaves the workspace buildable. |
| Complexity | 🟢 Low |
| Effort | 0.5h |
| Depends On |  |
| References | spike-only |

### T-002: [x] Typed view and argv builder

Represent the view as types: workspace, read-only paths, writable paths, environment, and a fixed namespace set. Render bwrap argv from the model, and keep the builder free of shell interpolation.


| Field | Value |
|-------|-------|
| Success Criteria | A unit test asserts that a view renders a stable argv and that no argument is produced by joining caller-supplied strings. |
| Complexity | 🟡 Medium |
| Effort | 3h |
| Depends On | T-001 |
| References | argument-construction,namespace-set,filesystem-view |

### T-003: [x] Canonicalisation and invariants

Canonicalise every path with `std::fs::canonicalize` before rendering, reject relative paths and paths escaping the workspace, and apply invariants the caller cannot widen: no host home, store read-only, workspace the only writable tree, network namespace always unshared.


| Field | Value |
|-------|-------|
| Success Criteria | A symlinked path renders resolved; a path escaping the workspace is rejected; a request to mount the host home or the store read-write is refused. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-002 |
| References | path-canonicalisation,invariant-rules |

### T-004: Version floor and fail closed

Parse `bwrap --version`, refuse to run below the floor required by the emitted flags, and exit non-zero without execing when bwrap is missing, too old, or a namespace cannot be created.


| Field | Value |
|-------|-------|
| Success Criteria | The exact exit code is asserted for a missing binary, an old version and a blocked user namespace; the command sentinel is absent in each case. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-002 |
| References | version-floor,fail-closed,bwrap-missing-or-old |

### T-005: [x] Fixture crate

A small Rust crate that builds and tests, materialised into the workspace before each run.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo build` and `cargo test` both succeed inside the sandbox with the store readable and the workspace writable. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-001 |
| References | shared-command-app,research-scenarios |

### T-006: [x] Hostile checks

Add the adversarial half of the fixture: a write outside the workspace, a write inside the host home, a read of a credential file, an outbound connection, and a child process that repeats the write. Each check reports whether the write landed.


| Field | Value |
|-------|-------|
| Success Criteria | Every hostile check reports the operation blocked and leaves no artefact on the host; a deliberately over-broad view makes at least one report a leak. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-005 |
| References | write-outside-workspace-denied,credential-read-denied,descendants-confined,network-denied-by-default |

### T-007: [x] First end-to-end run

Run the fixture under the harness with the initial bind set and record the raw output, the argv, and every failure.


| Field | Value |
|-------|-------|
| Success Criteria | The fixture crate builds and tests; all hostile checks are blocked; every failure is listed with the missing path or namespace that caused it. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-003,T-004,T-005,T-006 |
| References | shared-command-app |

### T-008: Baseline iteration

Add the paths a Nix-built toolchain needs, discovered by failure: the store, the system profile on NixOS, the dynamic linker, and the certificate, hosts and resolver files. Attribute each addition to the observation that required it.


| Field | Value |
|-------|-------|
| Success Criteria | A table in the findings maps each added path to the failure it fixed; removing it reproduces the failure. |
| Complexity | 🟡 Medium |
| Effort | 3h |
| Depends On | T-007 |
| References | toolchain-baseline,nixos-system-profile |

### T-009: [x] Isolation assertions

Prove the store is read-only, the host home is absent from the view, and host processes are invisible and unsignalable.


| Field | Value |
|-------|-------|
| Success Criteria | Writing to the store fails while executing from it succeeds; the host home path does not resolve; a kill against a host pid fails. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-007 |
| References | store-read-only,host-home-absent,host-pids-hidden |

### T-010: [x] Lifecycle

Make each sandbox its own session and process group and exercise pause, resume and delete against a command that keeps working. Count survivors after the kill.


| Field | Value |
|-------|-------|
| Success Criteria | The written byte count is unchanged while paused, increases after resume, and no child survives the group kill; the behaviour of a `setsid` escape is recorded. |
| Complexity | 🔴 High |
| Effort | 3h |
| Depends On | T-007 |
| References | process-group-lifecycle,pause-and-resume,delete-kills-and-cleans,process-group-escape |

### T-011: [x] Denial diagnostics

Establish what an operator sees when a command is blocked: errno in the child, whether the path is absent or permission-denied, and what bwrap or the kernel logged.


| Field | Value |
|-------|-------|
| Success Criteria | A written note states what is observable, what is not, and how a blocked command is distinguished from one that failed for its own reasons. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-007 |
| References | denial-diagnostics,explainable-failures |

### T-012: Portability matrix

Run the user namespace probe on NixOS, Fedora and Ubuntu, including Ubuntu's default AppArmor restriction, and record the exact command and result for each.


| Field | Value |
|-------|-------|
| Success Criteria | Each distribution has a row with kernel version, the probe command and its output; Ubuntu's restricted case records the remediation. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-004 |
| References | portability-matrix,userns-restricted |

### T-013: [x] Cost measurement

Time argv construction plus bwrap plus exec against a bare exec baseline and record median and p95.


| Field | Value |
|-------|-------|
| Success Criteria | A table reports both figures with the baseline column and the number of runs. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-007 |
| References | startup-cost |

### T-014: [x] Golden arguments

Store the rendered argv for a small set of views as fixtures and compare byte for byte.


| Field | Value |
|-------|-------|
| Success Criteria | Two fixtures cover a minimal view and a view with additional read paths; editing the builder fails the test until the fixture is updated. |
| Complexity | 🟢 Low |
| Effort | 1.5h |
| Depends On | T-002 |
| References | golden-arguments,testability |

### T-015: Edge checks

Exercise the edge cases that are cheap to confirm: a setuid binary, a device node outside the view, a nested bwrap, an unavailable cgroup namespace, and a command that exits leaving grandchildren.


| Field | Value |
|-------|-------|
| Success Criteria | Each case has a recorded observation and, where the behaviour is a limitation, a line in the findings. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-007 |
| References | setuid-binaries,device-nodes,nested-bwrap,cgroup-namespace-unavailable,zombie-reaping,proc-mount-failure |

### T-016: [x] Findings and assessment

Write `findings.md`: per goal, the observation with the raw output, the gap, and the open risk. Add the qualitative judgement of the argv builder, configuration effort and maintainability.


| Field | Value |
|-------|-------|
| Success Criteria | Every goal in the spec has an answer with evidence; the recommendation states whether a production implementation is worth building and what it must own. |
| Complexity | 🟡 Medium |
| Effort | 3h |
| Depends On | T-008,T-009,T-010,T-011,T-012,T-013,T-015 |
| References | findings-document,qualitative-assessment,findings-answer-questions,judgement-over-metrics |

### T-017: Teardown check

Confirm the harness is unreachable from the rest of the suite, then record what would have to be removed to discard the spike.


| Field | Value |
|-------|-------|
| Success Criteria | No agentkit crate references the prototype, the Docker path still builds and its tests pass, and the removal steps are written down. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-016 |
| References | no-behaviour-change-elsewhere,docker-path-unaffected,spike-only,primitive-longevity |

