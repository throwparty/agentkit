# DevShell Environment Prototype Tasks

## Tasks

### T-001: [x] Scaffold the harness crate

Create adrs/2026-11-07-devshell-environment/poc/prepare with its own Cargo.toml (serde, serde_json, toml) and a binary that takes a declared step. It is not a workspace member: the root Cargo.toml already excludes adrs, and nothing in crates/ may refer to it.

| Field | Value |
|-------|-------|
| Success Criteria | cargo build inside poc/prepare succeeds; the workspace's cargo metadata does not list the crate; no path dependency from any crate points at it. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | |
| References | prototype-not-shipped,no-new-dependency |

### T-002: [x] Model the step and its contributions

Step { identity, kind, settings } with the kind parsed as a string, Requirement { scope, name }, Contributions { environment, mounts }. Settings are typed per kind by the harness so an unknown key or a missing installable fails at parse rather than at launch.

| Field | Value |
|-------|-------|
| Success Criteria | A devshell step with an installable parses; an unknown key and a missing installable each fail naming the key; both are tests. The harness branches on the kind in exactly one place, and the number of branches is written down for the findings. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-001 |
| References | two-kinds,contributions,requirements,step-shape-refused |

### T-003: [x] Check the host before doing any work

nix on PATH with its version, flakes enabled, the installable evaluates, and path-info reports the devShell valid. Each check returns a structured result with a reason and a remedy, and a failure exits before print-dev-env runs.

| Field | Value |
|-------|-------|
| Success Criteria | With a PATH lacking nix the harness exits non-zero naming nix and a remedy; with an unbuilt derivation it reports not valid and writes only the .drv; a failed check adds no store path; each check has a test. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-002 |
| References | host-readiness,no-realisation,no-nix-refused,unrealised-refused,flakes-disabled,legible-unavailability |

### T-004: [x] Prepare the environment

print-dev-env --json for the baseline and the output path, then the devShell entered under env -i with only HOME and a minimal PATH, writing NUL-separated records to a file. Values come from the entered environment; provenance is the names that differ from the baseline. Shell state and backend-supplied values are dropped.

| Field | Value |
|-------|-------|
| Success Criteria | The variable count is within a small delta of T-011's 91; RUSTFLAGS carries no caller data; SSH_AUTH_SOCK, DOCKER_HOST, DBUS_SESSION_BUS_ADDRESS and WAYLAND_DISPLAY are absent; a banner-printing hook does not corrupt the records; a failing hook is reported with bounded output; a hook that execs another shell is recorded as the risk it is. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-003 |
| References | environment,environment-is-devshells,no-shell-state,no-values-on-surfaces,hook-fails,hook-prints,hook-execs-shell |

### T-005: [x] Turn the closure into mounts

path-info -r over the output path, de-duplicated and absolute, one read-only mount per path. A store-view switch selects the whole store instead. Record the count, the query and setup time, and the bwrap argv size.

| Field | Value |
|-------|-------|
| Success Criteria | The mount list equals the de-duplicated query output; the count matches T-022's 238 for this flake; the whole-store switch yields one mount; the argv size is recorded and the launch succeeds with it. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-004 |
| References | store-closure,closure-equals-query,backend-cannot-mount |

### T-006: [x] Launch the workload with the contributions

Feed lbx-bwrap the mounts as reads, the prepared environment and cargo test -p agentkit-path. The sandbox has no network and a read-only store.

| Field | Value |
|-------|-------|
| Success Criteria | cargo test -p agentkit-path passes inside the sandbox; the sandbox has no route to the host network; /nix/store is read-only; the launcher's environment is exactly the prepared one, with no caller variable added. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-004,T-005 |
| References | contributions,contributions-at-position,no-socket-or-network,no-nix-inside,host-placement |

### T-007: [x] Refuse a missing requirement

Declare a sandbox requirement the view cannot meet, such as a path that is not mounted or a shell the sandbox lacks, and show that the refusal is structured and happens before anything is launched.

| Field | Value |
|-------|-------|
| Success Criteria | The refusal names the requirement, its scope, the reason and the remedy; no sandbox process starts; the same step with the requirement satisfied proceeds. One test per scope. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-005 |
| References | requirements,requirement-refused,fail-closed,failure-classes |

### T-008: [x] Write the findings

Record the answers beside the crate: Q1 whether a kind the contract does not name survives a real launcher and what moving ProvisionKind and ProvisionStep to identity plus kind plus settings would cost; Q2 what a backend must report. Include the measurements and everything refuted, including the dirty-tree source write, the path-info refusal and the absence of shell state.

| Field | Value |
|-------|-------|
| Success Criteria | Each question has an answer with the command that produced it and an output excerpt; every measurement in the plan is present and reproducible from the file; the decisions left to the follow-up are listed; no claim appears without a citation. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-006,T-007 |
| References | boundary-proven,prototype-not-shipped,documentation |
