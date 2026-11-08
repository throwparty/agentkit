# Linux Bubblewrap Provider Tasks

## Tasks

### T-001: [x] Move the shared provisioning helpers

Move `run_steps`, the create budget, the transfer deadline and the output cap out of `backends/docker/backend.rs` into `backends/shared.rs` with `pub(crate)` visibility. Docker imports them and changes no behaviour. The provenance and failure types stay where they are.


| Field | Value |
|-------|-------|
| Success Criteria | Docker's existing tests pass untouched. A unit test in `shared.rs` covers a step that exits non-zero and a step that times out, using the same failure types Docker already reports. No constant is duplicated. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On |  |
| References | spawned-processes,docker-unchanged |

### T-002: [x] Add the backend module, its configuration and the registry entry

Create `src/backends/bubblewrap/` with `mod.rs` holding the `BACKEND` constant and `construct::build`, and `config.rs` typing `memory-mb` and `tasks` with `deny_unknown_fields`. Add the registry entry behind `cfg(target_os = 'linux')` and keep `DEFAULT` as docker. Rename the two registry tests that use bubblewrap as the name of an absent backend.


| Field | Value |
|-------|-------|
| Success Criteria | `litterbox backends` lists both backends and marks docker as selected. An unknown key in `[backends.bubblewrap]` fails as Misconfigured and names the key. A zero bound fails as Misconfigured. A test asserts the entry exists on Linux and is absent on a platform without it. The renamed registry tests pass. |
| Complexity | 🟢 Low |
| Effort | 3-4h |
| Depends On |  |
| References | backend-registration,platform-requirements,docker-unchanged,backends-listing |

### T-003: [x] Add the host seam

Define the `Host` trait covering the reads and writes the backend needs: a pid's existence, state, start time, uid, namespace inodes, environment and mount table; the scope's freeze, thaw, kill and limit files; binary resolution; and a clock. Implement `SystemHost` over `/proc`, cgroupfs and the file system, and `FakeHost` over a table for tests.


| Field | Value |
|-------|-------|
| Success Criteria | The backend is generic over `Host` and every `/proc`, cgroupfs and file-system read goes through it. `FakeHost` answers each method and a test asserts the backend reads nothing else. No path in the backend opens `/proc` directly. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-002 |
| References | test-coverage |

### T-004: [x] Render the view and its argv

Add `view.rs` with `Mount { sandbox_path, access }`, the ordered view, and the validation rules: absolute, not a protected root, the home, the metadata directory or the state directory, not a socket, device or FIFO, and no overlap with the workspace. Open each accepted path with `openat2` and `RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS`, and emit the descriptor as `/proc/self/fd/N`. Render the whole argv: seven namespaces, the nested-userns block, private root, proc, dev, tmpfs, home, and the mounts.


| Field | Value |
|-------|-------|
| Success Criteria | The golden test asserts the exact vector for the empty view, for an environment with a two-path closure and a workspace, and for the degraded form without a scope. A test proves that removing any invariant changes the vector. A refused path names the rule it broke. The argv emits no `--setenv`, no `--clearenv`, and no parent-death flag. |
| Complexity | 🔴 High |
| Effort | 8-12h |
| Depends On | T-003 |
| References | mounts-validation,mounts-render,golden-argv |

### T-005: [x] Resolve the holder command

Resolve the shell and the utility the holder runs from the host PATH, validate each the way bwrap is validated, and record the interpreter and libraries the host's loader reports for both. The sandbox's own userland is not the backend's to assume: a workload runs what its provisioning steps contribute.


| Field | Value |
|-------|-------|
| Success Criteria | The holder command names its shell, its utility and the paths they need bound. A missing or untrusted binary is reported as Unavailable with a remedy. A unit test covers the loader trace and the command's arguments. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-004 |
| References | mounts-render,holder-command |

### T-006: [x] Assemble the environment

Add `environment.rs` merging the resolved steps' contributions in order through `Contributions::environment_for`, applying the ungranted-path rule, then setting `HOME`, `TMPDIR` and `PWD`; `PATH` is whatever the steps contribute. Record withheld names in provenance as removed and the backend's own names as set.


| Field | Value |
|-------|-------|
| Success Criteria | A value naming an ungranted host path withholds its variable and the name is reported as removed. A credential-shaped name whose value names a granted path is carried. The merged environment applies later steps over earlier ones. `HOME`, `TMPDIR` and `PWD` are present and recorded as set, and a contributed `PATH` is carried unchanged. No value is written to a file, an argv or a log line. |
| Complexity | 🟡 Medium |
| Effort | 5-7h |
| Depends On | T-004,T-005 |
| References | environment-render,environment-ungranted-paths,secret-handling |

### T-007: Add the record payload

Add `payload.rs` with `BubblewrapData` version 1: the payload version, both pids with start times, the seven inode numbers, the uid, the root device and inode, the mounts from host paths with their access, the host and sandbox workspace paths, the scope unit, the paused flag and the provenance. Fail on a higher version as unreadable and on a lower version or missing field as Unavailable.


| Field | Value |
|-------|-------|
| Success Criteria | A round trip through `into_value` and `from_record` preserves every field. A higher version fails as unreadable, a lower version fails as Unavailable with the delete-and-recreate remedy, and a payload missing a field fails the same way. The metadata store's 0600 file and 0700 directory are asserted. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-003 |
| References | record-payload,record-edition-mismatch |

### T-008: Validate the record per operation

Add the single validation function that returns Absent, Forbidden, Editions or Live. Check the pid exists, is not a zombie, its uid and start time match, all seven inodes match through the descriptors that setns will receive, the root device and inode match, and every recorded host path appears with its recorded access while no host path outside the record is reachable. Map the outcome per operation: NotFound for a command, Missing for inspection and listing, already torn down for delete, Denied for a live mismatch, Unavailable for an edition mismatch.


| Field | Value |
|-------|-------|
| Success Criteria | A fake host mutates one field at a time and each mutation produces the documented outcome. A dead pid produces Missing for inspect and success for delete. A record with a foreign uid produces Denied. The kernel mounts for proc, dev and tmpfs are not treated as host paths. |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On | T-007 |
| References | record-validation,fail-closed,stale-holder,foreign-holder,foreign-record |

### T-009: Materialise the workspace and transfer into it

Copy the staged temporary tree into `data_dir/litterbox/bubblewrap/<project-slug>/<sandbox-name>/workspace` without following a link. Implement upload and download through an `O_PATH` descriptor with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`, copying a directory recursively, preserving modes and keeping symlinks as symlinks.


| Field | Value |
|-------|-------|
| Success Criteria | A tree with a symlink, a device and a FIFO is refused and the offending path is named. A copy from a source path containing spaces is byte-faithful. Upload writes only inside the workspace and download reads only from it. A round trip through download preserves modes and symlink entries. |
| Complexity | 🟡 Medium |
| Effort | 5-6h |
| Depends On | T-007 |
| References | workspace-materialisation,awkward-source-path |

### T-010: Start the holder and wait for the view

Add `holder.rs` building the spawn: the rendered argv, all three standard streams null, a cleared environment carrying only the assembled sandbox environment, and `systemd-run --user --scope --unit` when a scope is available. Read the status document with a timeout, then poll the mount table until every recorded host path is present with its recorded access. Tear down by killing the sandbox pid, writing `cgroup.kill` when a scope exists, waiting for the namespace to empty and removing the state directory.


| Field | Value |
|-------|-------|
| Success Criteria | A create against a fake host returns no sandbox when the status document is absent or malformed and reports the stage. A view that never completes fails within the budget and leaves no state directory. The large-closure host test creates a 1779-path view and reports how long the view took. The spawn is asserted to set all three streams to null. |
| Complexity | 🔴 High |
| Effort | 8-10h |
| Depends On | T-004,T-006,T-007 |
| References | holder-create,setup-diagnostics,cost,large-closure-create,large-closure |

### T-011: Implement create in the backend

Assemble create: refuse ports with Unsupported naming forwarding, refuse a duplicate name with Conflict before any process starts, run the sandbox steps through the shared runner under the shared budget, and write the payload once the view is ready. Record no sandbox on any refusal.


| Field | Value |
|-------|-------|
| Success Criteria | A project declaring a port is refused and leaves no branch, state directory or record. A second create of the same name is Conflict and the first sandbox is untouched. A failing step kills the sandbox and removes the state directory. `contribution_support` reports both sorts as Available. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-008,T-009,T-010 |
| References | holder-create,ports-refused,fail-closed,missing-contributed-path,ports-refused-create |

### T-012: Implement the join

Add `join.rs`: open the seven namespace descriptors and the root descriptor before joining, join the user namespace that owns the others through `NS_GET_USERNS`, then ipc, net, pid, mnt, uts and cgroup, then the sandbox's own user namespace, fork again, close descriptors above 2, chroot through the pinned root, chdir, set no-new-privileges, set uid and gid, clear capabilities, verify `/proc/self/ns` against the record, and exec with the environment read from the sandbox. Never call setgroups. Map the reserved statuses to named errors.


| Field | Value |
|-------|-------|
| Success Criteria | The host suite runs a command whose own `/proc/self/ns/pid` equals the sandbox's and differs from the host's. A listener started with its streams redirected is reachable from a second command and not from the host. A join that cannot enter a namespace returns the reserved status as a named error, never as the command's exit status. getgroups is never called and setgroups is never written. |
| Complexity | 🔴 High |
| Effort | 10-14h |
| Depends On | T-008,T-010 |
| References | exec-join,joined-process-identity,shared-context,killed-client,concurrent-commands |

### T-013: Implement pause, resume and the scope

Add the lifecycle facet: freeze through the scope and confirm by sampling cumulative CPU time until it settles, then thaw and report a failure if it does not. Record the paused state so inspection and a later process can report it. Without a scope, and only after acceptance, pause and resume use the stop-and-rescan loop over the pid namespace.


| Field | Value |
|-------|-------|
| Success Criteria | A command writing every 50ms stops making progress after pause and continues after resume, and inspection reports paused then running. A confirmation that never settles reports a failure and leaves the sandbox thawed. A pause that cannot be confirmed does not leave the sandbox frozen. A paused sandbox can be deleted. |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On | T-010,T-012 |
| References | lifecycle-facet,lifecycle-works,paused-then-deleted |

### T-014: Implement limits and the degraded mode

Write `memory.max` and `pids.max` on the scope at create and in `set_limits`, reporting a failed write as a failure. Report the cgroup check as Degraded with a `Fallback` when there is no scope, and replace the facet-derived pause and limits capability entries with that state so the listing cannot offer a bound the host cannot apply.


| Field | Value |
|-------|-------|
| Success Criteria | On a host without a scope, create is refused until `accept-degraded = true`, then succeeds. `set_limits` refuses at the facet level with Unsupported naming the cgroup requirement. `litterbox backends` reports pause and limits as degraded with their fallback. A failed write is reported, not swallowed. |
| Complexity | 🟡 Medium |
| Effort | 4-6h |
| Depends On | T-010,T-013 |
| References | resource-limits,degraded-without-cgroups,degraded-accepted,no-cgroup-delegation |

### T-015: Implement readiness

Report one check per requirement: the effective uid is not root, bwrap is present, canonical, root-owned, not group or world writable and at least 0.8.0, user namespaces are permitted, a scope is usable or Degraded with the fallback, a holder command resolves, and the shell resolves with its utilities beside it. Refuse create on any check that is not available except the cgroup one, which is gated behind acceptance.


| Field | Value |
|-------|-------|
| Success Criteria | `litterbox backends` prints every check with a state, a reason and a remedy. `--json` emits the same report. An untrusted bwrap is skipped in favour of a later trusted candidate, and a path with only untrusted candidates is Unavailable. A version below the floor is Unavailable and names the floor. Running as root is Misconfigured. |
| Complexity | 🟡 Medium |
| Effort | 5-7h |
| Depends On | T-005,T-010,T-014 |
| References | readiness-checks,user-namespaces-blocked,running-as-root,untrusted-bwrap,version-floor |

### T-016: Implement listing, inspection and delete

Rebuild each payload from the state directory for both scopes, report a corrupt entry rather than dropping it, and report another backend's records without failing. Inspect reports egress Denied, the workspace path, the recorded provenance and no forwards. Delete kills the sandbox pid, waits for the namespace to empty, removes the state directory, and succeeds when the sandbox is already gone or the record cannot be read.


| Field | Value |
|-------|-------|
| Success Criteria | Machine-wide pause sees this backend's sandboxes. A sandbox whose holder died is still listed and reports Missing. A corrupt payload is reported. A machine holding records from both backends does not fail. Delete after an out-of-band kill succeeds and removes the state directory. A record refused by validation is still deletable. |
| Complexity | 🟡 Medium |
| Effort | 5-7h |
| Depends On | T-008,T-010,T-012 |
| References | machine-list,egress-denied,delete-teardown,holder-crash |

### T-017: Cover the suite: unit, host and conformance

Write the unit tests over `FakeHost` and the renderer, the ignored host suite and the crate's conformance run. The host suite creates a sandbox, runs two commands, proves the shared pid and loopback context, kills a client and finds no defunct process, deletes the sandbox, and keeps the probes from T-033 to T-041 as regression checks. It also covers a recycled pid, a foreign holder with a writable root, a symlink planted in the workspace, and the isolation cases: a store write fails read-only, an ungranted host path is absent, an outbound connection has no route, and a granted tree holding a unix socket is refused at create.


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests cover the golden argv, the payload round trip and both version directions, every validation outcome, each credential family, the ungranted-path rule, the holder command, the readiness states and the reserved-status mapping. The host suite skips with the missing requirement named where bwrap is absent. The conformance run covers duplicate create, transfers, delete idempotency, inspection after delete, step order and lifecycle visibility. No contributed value appears in the record, the state directory or the log. |
| Complexity | 🔴 High |
| Effort | 10-14h |
| Depends On | T-011,T-012,T-013,T-014,T-015,T-016 |
| References | test-coverage,host-join,isolation-holds,prepared-environment,no-values-on-disk,create-and-shell |

### T-018: Declare the architecture rules and document the backend

Add an arch-lint scope for `src/backends/bubblewrap/**` with deny edges to the orchestrator, the command line and MCP code, persistence, SCM and provision preparation, and a negative fixture for each edge. Document bubblewrap on the backends page and in the user configuration reference: its keys and units, its readiness output, what the degraded mode does not bound, and the ports refusal.


| Field | Value |
|-------|-------|
| Success Criteria | `arch-lint check` passes and each new deny edge has a negative fixture that fails when the edge is removed. The backends page no longer says Docker is the only mechanism. The configuration reference names `memory-mb` and `tasks` with their units. The documented reach matches what inspection reports. |
| Complexity | 🟢 Low |
| Effort | 3-4h |
| Depends On | T-002 |
| References | backend-documentation,fail-closed |

