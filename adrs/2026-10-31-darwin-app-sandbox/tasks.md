# Darwin App Sandbox Spike Tasks

## Tasks

### T-001: [x] Scratch layout

Create `adrs/2026-10-31-darwin-app-sandbox/poc` with `custom`, `nono` and `fixture` subdirectories. Each Cargo.toml carries an empty `[workspace]` table so neither crate joins the agentkit workspace, and no agentkit crate references them.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo metadata` at the repo root does not list either prototype package; removing the ADR directory leaves the workspace buildable. |
| Complexity | 🟢 Low |
| Effort | 0.5h |
| Depends On |  |
| References | spike-only,shared-independent-prototypes |

### T-002: [x] Custom policy model

Represent the policy as types: workspace path, read grants, write grants, connect grants, and a baseline. Canonicalise every path with `std::fs::canonicalize` before rendering, reject relative paths, and render SBPL from the model rather than by string concatenation at the call site.


| Field | Value |
|-------|-------|
| Success Criteria | A unit test asserts that a model with a symlinked path renders the resolved path; a relative path is rejected before rendering. |
| Complexity | 🟡 Medium |
| Effort | 3h |
| Depends On | T-001 |
| References | policy-schema,policy-surface,path-canonicalisation |

### T-003: [x] Custom binary and exec

Parse `--workspace`, repeatable `--read`, `--write`, `--connect`, then `--` and a command. Apply the profile and replace the process with the command; never return to the caller.


| Field | Value |
|-------|-------|
| Success Criteria | Running with a grant for one path writes there and is refused outside it; an unknown argument exits non-zero without execing. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-002 |
| References | confined-helper-binary,policy-transport,new-process-per-command,fail-closed |

### T-004: [x] Baseline and invariants

Add the read-only baseline for the dynamic linker and system paths, the deny-default stance, and the invariants the caller cannot override: writes only inside the workspace and explicit write grants, reads denied for credential directories, network denied unless granted.


| Field | Value |
|-------|-------|
| Success Criteria | Removing the baseline causes exec to abort, which is recorded; adding a `--write` grant for `$HOME` still leaves credential reads denied. |
| Complexity | 🟡 Medium |
| Effort | 3h |
| Depends On | T-003 |
| References | deny-by-default-baseline,invariant-safety-rules,mechanism-limits |

### T-005: [x] Apply and survive

Call `sandbox_init`, print the returned message verbatim on failure and exit non-zero without execing. Record whether the binary is signed and what macOS reports for it.


| Field | Value |
|-------|-------|
| Success Criteria | An unsigned local build confines a child process; a deliberately malformed profile produces the library's message rather than a silent abort. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-003 |
| References | profile-applies-unsigned,sandbox-init-returns-an-error,deprecation-resilience |

### T-006: [x] Golden profiles

Store the rendered profile for a small set of inputs as fixtures and compare byte for byte, so a change to the emitter shows up as a fixture diff.


| Field | Value |
|-------|-------|
| Success Criteria | Two fixtures cover an empty network grant set and a single connect grant; editing the emitter fails the test until the fixture is updated. |
| Complexity | 🟢 Low |
| Effort | 1.5h |
| Depends On | T-002 |
| References | golden-profiles,testability |

### T-007: [x] Fixture crate

A small Rust crate that builds and tests, materialised into the workspace before each run. It is the workload both prototypes confine.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo build` and `cargo test` both succeed inside the custom sandbox with the toolchain readable and the workspace writable. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-001 |
| References | shared-command-app,research-scenarios,toolchain-not-installed |

### T-008: [x] Hostile checks

Add the adversarial half of the fixture: a write outside the workspace, a write inside `$HOME`, a read of `~/.ssh`, an outbound connection, and a child process that repeats the write. Each check reports whether the write landed rather than whether an error was printed.


| Field | Value |
|-------|-------|
| Success Criteria | Every hostile check reports the operation denied and leaves no artefact on the host; a deliberately over-broad profile makes at least one report a leak. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-007 |
| References | write-outside-workspace-denied,credential-read-denied,descendants-confined,network-denied-by-default,symlinked-paths,workspace-inside-home,home-directory-paths-in-the-baseline |

### T-009: [x] Custom end-to-end run

Run the fixture under the custom prototype and record the raw output, the profile, and anything that had to be added to make a real workload run.


| Field | Value |
|-------|-------|
| Success Criteria | The fixture crate builds and tests, all hostile checks are denied, and every profile addition is attributed to the observation that required it. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-004,T-005,T-007,T-008 |
| References | shared-command-app |

### T-010: [x] Nono prototype

Link nono at an exactly pinned version, map workspace and grants onto `CapabilitySet`, apply, and exec. Filter the baseline for paths that do not exist, because nono rejects them.


| Field | Value |
|-------|-------|
| Success Criteria | The same arguments produce the same confinement through nono; the pinned version appears in Cargo.toml and Cargo.lock. |
| Complexity | 🟡 Medium |
| Effort | 3h |
| Depends On | T-001 |
| References | nono-capability-mapping,nono-breaking-releases,nono-fixed-policy |

### T-011: [x] Nono gap inventory

Record every requirement the capability model cannot express, with the reason, and note each place a raw platform rule was needed to get further. Attempt one per-host network grant and record whether the escape hatch closes the gap.


| Field | Value |
|-------|-------|
| Success Criteria | The inventory names at least the `/dev/null` write grant and the macOS network limitation; each entry states whether it was worked around and how. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-010 |
| References | nono-gap-inventory,nono-platform-reach,network-grant-per-host |

### T-012: [x] Nono end-to-end run

Run the identical fixture under the nono prototype and record the results beside the custom ones.


| Field | Value |
|-------|-------|
| Success Criteria | The fixture crate builds and tests under nono and the hostile checks match the custom results, or every difference is explained. |
| Complexity | 🟡 Medium |
| Effort | 1.5h |
| Depends On | T-008,T-010 |
| References | nono-conformance |

### T-013: [x] Lifecycle

Make each sandbox its own process group and exercise pause, resume and delete against a command that keeps working. Count surviving children after the kill.


| Field | Value |
|-------|-------|
| Success Criteria | The written byte count is unchanged while paused, increases after resume, and no child survives the group kill; the same is recorded for any process that leaves the group. |
| Complexity | 🔴 High |
| Effort | 4h |
| Depends On | T-003,T-010 |
| References | process-group-lifecycle,pause-and-resume,delete-kills-and-cleans,process-group-escape |

### T-014: [x] Egress grants

Start a local server, grant exactly its address and port, and check that the granted connection succeeds while an ungranted port and an external host fail. No external network dependency.


| Field | Value |
|-------|-------|
| Success Criteria | Granted connection succeeds; ungranted and external connections fail; the recording notes whether the external failure was a refusal or a name-resolution failure. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-003,T-013 |
| References | network-denied-by-default,network-grant-per-host,research-scenarios |

### T-015: [x] Denial diagnostics

Establish what an operator can see when a command is blocked: errno in the child, any system log message, and whether the denial can be attributed to a specific command.


| Field | Value |
|-------|-------|
| Success Criteria | A written note states what is observable, what is not, and which of the two prototypes does better, with the commands used to check. |
| Complexity | 🟡 Medium |
| Effort | 2h |
| Depends On | T-009 |
| References | denial-diagnostics,explainable-failures |

### T-016: [x] Findings and assessment

Write `findings.md`: per goal, the observation with the raw output, the gap, and the open risk. Add the qualitative comparison of interface shape, configuration effort, and maintainability.


| Field | Value |
|-------|-------|
| Success Criteria | Every goal in the spec has an answer with evidence; both prototypes are compared in prose rather than a score. |
| Complexity | 🟡 Medium |
| Effort | 3h |
| Depends On | T-009,T-012,T-013,T-014,T-015 |
| References | findings-document,qualitative-assessment,findings-answer-questions,judgement-over-metrics |

### T-018: [x] Fail-closed harness

Runs the helper with seven invalid inputs, a symlink escape and a dump into the workspace, asserting exact exit codes and that the command never ran.


| Field | Value |
|-------|-------|
| Success Criteria | Every case exits 64 and the sentinel inside the workspace is absent. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-003,T-005 |
| References | fail-closed |

### T-019: [x] Credential harness

Grants the whole home, places the workspace inside it, and asserts credential denial with a positive control.


| Field | Value |
|-------|-------|
| Success Criteria | Key, keychain and traversal denied; an ordinary home file readable. |
| Complexity | 🟡 Medium |
| Effort | 1h |
| Depends On | T-004,T-008 |
| References | invariant-safety-rules,credential-read-denied |

### T-020: [x] Process-table check

Runs a resident command and asserts the profile text is absent from the process table while the command is visible.


| Field | Value |
|-------|-------|
| Success Criteria | Profile absent, command visible. |
| Complexity | 🟢 Low |
| Effort | 0.5h |
| Depends On | T-003 |
| References | policy-off-the-command-line |

### T-021: [x] Cost measurement

Times apply-plus-exec against a bare exec baseline and records median and p95.


| Field | Value |
|-------|-------|
| Success Criteria | Both prototypes measured with a baseline column. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-003 |
| References | startup-cost |

### T-017: [x] Teardown check

Confirm the prototypes are unreachable from the rest of the suite, then record what would have to be removed to discard the spike.


| Field | Value |
|-------|-------|
| Success Criteria | No agentkit crate references the prototypes, the Docker path still builds and its tests pass, and the removal steps are written down. |
| Complexity | 🟢 Low |
| Effort | 1h |
| Depends On | T-016 |
| References | no-behaviour-change-elsewhere,docker-path-unaffected,spike-only |

