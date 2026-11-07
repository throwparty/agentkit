# DevShell Environment Implementation Tasks

## Tasks

### T-001: [x] Contract value types

In `sandbox/contract.rs`, remove `ProvisionKind` and the `ProvisionStep` enum. Add `ProvisionStep { identity, kind, settings }` with a custom deserializer that removes `identity` and `kind` and keeps the remaining keys as a `BTreeMap<String, toml::Value>`. Add `Placement { Host, Sandbox }`, `ContributionSort { Environment, Mount }`, `Contributions { variables, provenance, mounts, workspace_root, workspace_variables }`, `ContributionSupport { sort, state, reason, remedy, fallback }`, `SandboxAction { run }` and `ResolvedStep { identity, kind, placement, sandbox, contributions }`. Replace `CreateRequest.provision` with `Vec<ResolvedStep>`.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo check -p agentkit-litterbox` fails only at the consumers handled in T-002; a `[[provision]]` table round-trips into `ProvisionStep`; `identity` and `kind` do not appear in `settings`; no `ProvisionKind` remains in the contract; the contract has no kind-specific variant. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On |  |
| References | kind-registry,contributions,contribution-support,request-contributions,no-new-dependency |

### T-002: [x] Backend contract and capabilities

Replace `Backend::provision_support` with `contribution_support`. Update `Selected::merge_capabilities` to emit `contribution:<sort>` entries. Update `sandbox/facets.rs`, `orchestrator/bulk.rs`, `testing.rs` and the Docker backend with stubs that report no contribution sorts. Update every site that constructs `ProvisionSupport`.


| Field | Value |
|-------|-------|
| Success Criteria | `cargo check -p agentkit-litterbox` passes; `Selected` emits `contribution:environment` only for a backend that reports it; Docker and TestBackend compile with empty support; the contract conformance suite passes. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-001 |
| References | contribution-support,contribution-refused,boundaries |

### T-003: [x] Provenance and environment rebasing

Change `StepProvenance.kind` to `String`. Add `Contributions::environment_for(&self, workspace: &Path) -> Environment`, which builds the runtime environment and rewrites only the variables named in `workspace_variables`, replacing the recorded `workspace_root` prefix with the sandbox workspace. Leave store paths untouched. Keep host paths in `Provenance.host_paths`.


| Field | Value |
|-------|-------|
| Success Criteria | A unit test builds an `Environment` from contributions with a different workspace path and asserts only `CARGO_HOME`, `RUSTUP_HOME` and the PATH entry change; a store path in `NIX_LDFLAGS` is unchanged; `StepProvenance` serialises `kind` as a string; a fixture secret is absent from the serialised provenance. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-001 |
| References | provenance,workspace-mapping,no-values-on-surfaces,workspace-elsewhere,provenance-no-value |

### T-004: [x] Kind registry and settings

Add `provision/kind` with a `Kind` trait, `KindDescription`, `SettingsError`, an internal `Parsed` enum, `command.rs`, `devshell.rs` and `registry.rs`. Each kind reports its name, placement, contribution sorts and consent requirement, validates its settings, and exposes `check` and `prepare` hooks. `command` accepts only `run`; `devshell` accepts only `installable`; an unknown kind or key fails with the identity and kind. Register a test-only third kind behind `cfg(test)`.


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests parse a command step and a devshell step; an unknown kind, an unknown key and a missing installable each fail naming the identity; two devshell steps in one sequence fail; the test-only kind is registered only in tests and has no contract variant. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-001 |
| References | kind-registry,devshell-step,step-shape-refused,two-devshell-steps,registry-proven |

### T-005: [x] Config generic step and store view

Update `config.rs` so `ProjectFile.provision` and `Config.provision_definitions` hold the generic step. Keep the generic validation in `check_provisioning` and call the registry from `config_loader::validate_provisioning` for kind-specific settings. Add `StoreView { Closure, Whole }` to `Config`, read `store-view` from the local override only, and ignore a project value with a notice. Default to `Closure`.


| Field | Value |
|-------|-------|
| Success Criteria | A project with a devshell step loads; an unknown kind, unknown key or missing installable fails at load with the identity; a project `store-view` produces a notice and does not take effect; a local `store-view = 'whole'` reaches `Config`; existing provisioning config tests pass. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-004 |
| References | kind-registry,devshell-step,step-shape-refused,config-compatibility,store-view-setting |

### T-006: [x] Host checks and closure

Add `provision/prepare/runner.rs` with a `CommandRunner` trait, a system runner and a recorded runner. Add `checks.rs` for nix version, flakes, installable evaluation and store validity, each with a reason and remedy. Add `closure.rs` for `path-info -r` or the store root for the whole view. Add `check` and `prepare` entry points in `mod.rs`. All process execution goes through the runner.


| Field | Value |
|-------|-------|
| Success Criteria | With a recorded runner, a missing nix, disabled flakes, a failed evaluation and an invalid devShell each produce the structured reason and remedy; `closure` de-duplicates and matches a fixture query; a dirty tree is reported as a possible source write; no module outside `prepare` spawns a process. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-001 |
| References | host-requirements,no-realisation,store-closure,no-nix-refused,unrealised-refused,flakes-disabled,nix-absent,dirty-tree,no-new-supervisor,legible-unavailability |

### T-007: [x] Environment capture

Add `provision/prepare/environment.rs`. Run `print-dev-env --json` for the baseline and output path, then evaluate the `print-dev-env` script once in a bash child. Start the child from a caller with HOME, PATH, PWD, OLDPWD, SHLVL and `_`; dump `env -0` before and after; compute the diff; restore `NIX_BUILD_TOP`, `TMP`, `TMPDIR`, `TEMP`, `TEMPDIR` and `terminfo`. Keep exported-but-empty variables. Record `workspace_root` and the variables whose values contain it. Record a host path for a value that is a single absolute path outside the store and the workspace. Redirect hook output to a bounded file; a non-zero hook is Failed with the output.


| Field | Value |
|-------|-------|
| Success Criteria | Against recorded script output, the contribution contains the 18 empty build-phase variables and no HOME, PWD, OLDPWD, SHLVL or `_`; PATH leads with CARGO_HOME/bin; a caller RUSTFLAGS is absent; a banner-printing hook does not corrupt the records; a failing hook is Failed with bounded output; a fixture secret is absent from provenance. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-006 |
| References | environment-capture,environment-is-direnv,empty-variables-kept,no-shell-state,no-shell-state-accepted,hook-fails,hook-prints,hook-execs-shell,command-needs-shell-state,no-values-on-surfaces |

### T-008: [x] Devshell kind prepare and contributions

Implement `kind/devshell.rs` and the devshell prepare entry point. `check` runs the host checks and the closure query without evaluating the hook. `prepare` runs the checks, the capture and the closure and returns `Contributions` with the environment, provenance, mounts, workspace root and workspace variables. Reject a second devshell step in one backend sequence. Keep nix out of the contributions.


| Field | Value |
|-------|-------|
| Success Criteria | A prepared devshell yields a non-empty environment and a closure of store paths; the whole view yields the store root; a backend limit is reported by the backend, not the kind; the contributions contain no nix binary, socket path or network setting; a command step yields a `SandboxAction` and no contributions. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-004,T-006,T-007 |
| References | devshell-step,contributions,store-closure,closure-equals-query,backend-cannot-mount,closure-too-large,no-nix-inside |

### T-009: [x] Resolver

Rewrite `provision/resolve.rs` to use the registry. For each definition, look up the kind, validate its settings, compare the kind's contribution sorts with the backend's `contribution_support`, and return planned steps carrying the generic step and the kind description. Remove every match on `ProvisionKind`. A missing sort is Unsupported; a missing kind or setting is Misconfigured. Keep `resolve` and `preflight_steps` pure.


| Field | Value |
|-------|-------|
| Success Criteria | A devshell step against an empty support list is Unsupported naming the step, backend, sort, reason and remedy; a command step against an empty list resolves; an unknown kind and a missing installable are Misconfigured; no kind enum remains in the resolver; unit tests cover each class. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-002,T-004 |
| References | contribution-support,contribution-refused,failure-classes,not-a-devshell,backend-cannot-mount |

### T-010: [x] Orchestrator consent and preparation order

Update `orchestrator/service.rs`. After resolution, refuse a step whose kind requires consent unless the project is in `allowed-projects` or `allow-project-setup`. Prepare every planned step on a blocking thread before creating the branch. A command step yields a `SandboxAction`; a devshell step yields contributions. Build `CreateRequest` with `ResolvedStep` values in sequence order, then create the branch, stage and call the backend. A preparation failure leaves no branch.


| Field | Value |
|-------|-------|
| Success Criteria | A devshell step outside allowed-projects is Denied and no nix process starts; a hook failure leaves no branch or record; a prepared request reaches the backend with contributions at the devshell step's position; a command step before the devshell sees the default environment and one after sees the prepared environment. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-008,T-009 |
| References | host-placement,consent,consent-gate,fail-closed,hook-fails,no-realisation |

### T-011: [x] Docker backend

Update `backends/docker/backend.rs` for the new contract. `contribution_support` returns an empty list. `create` runs only `ResolvedStep` values with a `SandboxAction`, in order, and records the command steps in the provenance. Keep `DockerData` as container, forwards and `Provenance`. `exec`, `inspect` and `delete` are unchanged. Do not add host-step application.


| Field | Value |
|-------|-------|
| Success Criteria | Docker reports no contribution sorts; a project with a devshell step is refused as Unsupported before the container is created; a command-only project creates and runs as before; the Docker tests and the contract conformance suite pass. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-002,T-009 |
| References | docker-compatibility,docker-rejects,docker-selected,config-compatibility,no-socket-or-network |

### T-012: [x] Doctor

Update `cli.rs` and `orchestrator::preflight`. `DoctorStep.kind` becomes `Option<String>`. Add the contribution support entries, the store view and, per host step, the requirement checks, the installable, whether it is valid and the closure count. A failed requirement is Unavailable with its reason and remedy; a missing consent is Denied. The doctor calls `check`, never `prepare`, and creates no sandbox, workspace or branch. Keep `--json` and the exit codes.


| Field | Value |
|-------|-------|
| Success Criteria | The doctor on a devshell project reports the host checks, installable, validity, closure count and store view; a missing nix, disabled flakes, an unrealised devShell and a denied project each report the right state; a store listing before and after differs only by a dirty-tree source path; the JSON shape and exit codes are tested. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-006,T-008,T-009 |
| References | doctor,doctor-no-mutation,host-requirements,store-view-setting,legible-unavailability,dirty-tree |

### T-013: [x] Test backend, conformance and cross-process

Make `TestBackend`'s contribution support configurable, record the `ResolvedStep` values in the record, apply the recorded environment on `exec`, and return the provenance from `inspect`. Register the test-only third kind. Add the position test and the cross-process test: create a record through `metadata_store`, spawn the test binary with an environment variable, and assert the child reads the same environment and provenance. Add a second-create test for a changed flake.


| Field | Value |
|-------|-------|
| Success Criteria | A recording backend sees contributions at the devshell step's position; a second process reads the same environment and provenance from the record; delete removes the contributions; the test-only kind resolves without a contract variant; a second create re-prepares and may differ. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-010,T-011 |
| References | registry-proven,contributions-at-position,persistence,persistence-across-processes,no-values-on-surfaces,test-selection,devshell-changed |

### T-014: [x] Arch-lint

Update `arch-lint.toml`. Split the provision scope into `litterbox-kinds` at `src/provision/kind/**`, `litterbox-prepare` at `src/provision/prepare/**` and `litterbox-provision` for the resolver. Allow config to reach kinds and prepare to reach kinds. Deny every backend the kinds, prepare and resolver scopes. Deny prepare the backends, orchestrator, surfaces, persistence, SCM and configuration scopes. Add a negative fixture for each new deny edge. Verify no production source refers to the prototype.


| Field | Value |
|-------|-------|
| Success Criteria | The architecture lint passes on the tree; each new deny edge has a negative fixture that fails; a grep of production sources finds no reference to `adrs/2026-11-07-devshell-environment/poc`. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-009,T-010 |
| References | boundaries,kind-registry,contribution-support,prototype-not-shipped |

### T-015: [x] Documentation

Update the configuration reference with the `devshell` step, `installable`, the `store-view` setting and the consent rule. Document the doctor's host report with the existing preflight output. Add kind, contribution, host step and store view to `crates/agentkit-litterbox/CONTEXT.md`. Extend `.agentkit/tackle/prompts/nix/PROMPT.md` with the `print-dev-env` evaluation and the six restores.


| Field | Value |
|-------|-------|
| Success Criteria | The docs build; the configuration reference shows the new step and setting; the doctor section names every reported field; CONTEXT.md defines the new terms; the nix prompt records the capture rule; `cargo metadata` shows no new crate. |
| Complexity | 🟢 Low |
| Effort | 2-3h |
| Depends On | T-008,T-012 |
| References | documentation,config-compatibility,no-new-dependency |

### T-016: Host tests

Add a host test target that requires nix and a valid devShell. Cover the capture against this repository's flake, the closure count and equality with `path-info -r`, the unrealised refusal (only the derivation is written), the missing-nix refusal, and the dirty-tree source write. The target must never build a derivation and must be selected separately from the unit tests.


| Field | Value |
|-------|-------|
| Success Criteria | The host target passes on a host with nix and a valid devShell; the environment matches `environment-is-direnv`; the mounts equal the query; an unbuilt devShell fails as Unavailable and writes no output path; the target is absent from the default `cargo test` selection. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-007,T-008 |
| References | environment-is-direnv,closure-equals-query,unrealised-refused,no-nix-refused,test-selection,dirty-tree |

