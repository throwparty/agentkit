# Sandbox Provider Contract

## Tasks

### T-001: [x] Rename agentkit-path to agentkit-common

Rename the crate directory, package and lib path, and add an exit module holding the shared taxonomy (0 ok, 1 failed, 2 usage, 3 not found, 4 conflict, 5 unsupported, 6 denied, 7 unavailable) with a class-to-exit-code helper. Update the four dependants: switchboard, tackle, credentials and litterbox. Change no dependant's exit behaviour.


| Field | Value |
|-------|-------|
| Success Criteria | The workspace builds; no reference to agentkit-path remains; the crate declares no dependencies of its own; a unit test pins every taxonomy value. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | neutral-errors,no-new-dependency |

### T-002: [x] Contract trait and neutral types

Create src/sandbox/ with an object-safe Sandbox trait over BoxFuture, plus CreateRequest, Scope, Inspection, SandboxStatus and the neutral view of a sandbox. Leave the existing provider in place; the module compiles beside it.


| Field | Value |
|-------|-------|
| Success Criteria | An implementation coerces to Arc<dyn Sandbox> in a test; no contract type names a container, image, Docker, namespace or mechanism; no contract type is gated on a platform or OS name; Inspection carries only status, backend, egress, workspace, provenance and forwards. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On |  |
| References | backend-neutral-contract,sandbox-identity,sandbox-status,backends-are-mechanisms,primitive-gating |

### T-003: [x] Error taxonomy and surface mapping

Add src/sandbox/error.rs with NotFound, Conflict, Unsupported carrying operation, primitive, reason and remedy, Denied and Failed. Convert the existing SandboxError and ComputeError into it, and put the exit-code and MCP-class mapping in one place.


| Field | Value |
|-------|-------|
| Success Criteria | Each class has a test asserting its exit code and MCP class; unsupported carries all four fields; contract-level text is mechanism-free and the backend supplies its own words in the reason. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-001 |
| References | neutral-errors,legible-unavailability,neutral-errors-accepted |

### T-004: [x] Readiness and capability reports

Add report.rs and environment.rs: Readiness with per-primitive Check entries, Capability per operation, EgressPosture as denied, direct or proxied, Environment with Provenance by variable name and kind, and the workspace path type.


| Field | Value |
|-------|-------|
| Success Criteria | Report fields are typed, not formatted strings; a degraded check serialises its fallback and what it cannot bound; egress has three variants and no boolean exists; an environment that names a host path outside the view is representable as a report rather than as a grant. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-002 |
| References | backend-readiness,capability-report,egress-posture,environment-provenance,legible-unavailability,degradation-reported,delegation-absent,host-path-in-environment |

### T-005: [x] Facet traits and the compile-time gate

Add facets.rs with Lifecycle, Limits and Forwarding as supertraits of Sandbox, plus LimitsSpec and Forward. Include a compile_fail test showing a lifecycle call is rejected on a core-only type.


| Field | Value |
|-------|-------|
| Success Criteria | A compile_fail test fails for a lifecycle call on a type implementing only Sandbox; one Arc coerces into both Arc<dyn Sandbox> and Arc<dyn Lifecycle>; clippy is clean. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-002 |
| References | facet-families,facets-gate-compilation |

### T-006: Move Docker under backends/docker

Relocate compute/mod.rs to backends/docker/compute.rs, container naming and host port allocation to naming.rs, and the provider to backends/docker/mod.rs. Keep behaviour and tests; change no logic in this task.


| Field | Value |
|-------|-------|
| Success Criteria | The existing Docker tests pass unchanged after the move; nothing outside backends/docker refers to compute; the diff is a relocation. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On |  |
| References | docker-conformance,module-boundaries |

### T-007: Docker backend on the contract

Implement Sandbox for DockerBackend: create with setup and rollback, list for both scopes, exec, upload, download, inspect, delete, readiness and capabilities. Report direct egress, unchanged environment provenance, the workspace path and forwards with their bind address. Implement Lifecycle and Forwarding.


| Field | Value |
|-------|-------|
| Success Criteria | Every existing provider test is ported with unchanged assertions and passes; inspect reports direct egress; a failed setup leaves no container and no branch; a forwarded port reports the configured bind address; a confinement or setup failure is distinguishable from a command that exited non-zero; a workspace bound elsewhere translates only workspace references. |
| Complexity | 🔴 High |
| Effort | 6-8h |
| Depends On | T-006 |
| References | docker-conformance,backend-neutral-contract,egress-posture,port-forwarding,create-atomicity,workspace-transfer,environment-provenance,workspace-path-mapping,fail-closed,provenance-visible,egress-typed,ports-honest,create-leaves-nothing,workspace-relocated,direct-egress-reported |

### T-008: Docker configuration and readiness

Add backends/docker/config.rs with DockerConfig typed there and deny_unknown_fields, parsed from its own section. Move image validation out of config_loader and answer readiness with the daemon and image state, each with a reason and a remedy.


| Field | Value |
|-------|-------|
| Success Criteria | An unknown key under the Docker section fails naming that key; a missing image reports readiness with a remedy rather than a generic config error; the core config types name no Docker key. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-006 |
| References | per-backend-config,backend-readiness,primitive-missing-or-old |

### T-009: Configuration scopes and merge

Add src/config/ with the user file under agentkit-common's config directory, the project file and the local override, merged defaults then user then project then local. Backend selection and the forwarding bind are user-only; the backends map holds toml::Value per backend; setup-command moves under [project].


| Field | Value |
|-------|-------|
| Success Criteria | A project file naming a backend is reported and has no effect; a project-set bind interface is reported and ignored; setup-command under [docker] fails naming the new key; merge order is covered by tests. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-001 |
| References | backend-selection,per-backend-config,port-forwarding,project-asks-for-backend |

### T-010: Backend registry and construction point

Add backends/mod.rs listing the compiled-in set, resolving selection from configuration or the per-invocation flag, constructing one backend, and returning Selected with the core trait object and whichever facet objects the mechanism implements.


| Field | Value |
|-------|-------|
| Success Criteria | Exactly one constructor exists; an unknown backend fails listing what the binary contains; an unready backend fails with its readiness reason and remedy and never falls back; two backends on one host are listed and selected independently; a test observes the Docker facet objects through Selected. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-005, T-008, T-009 |
| References | single-construction-point,backend-selection,one-construction-point,backend-readiness,readiness-refuses,mechanism-requirement-unmet,two-backends-one-host |

### T-011: Record store with backend field

Replace SandboxMetadata with SandboxRecord carrying name, backend, project, branch, mode and a backend-owned payload. Detect legacy records through toml::Value and report them; delete metadata_for_slug and the resolve fallback so nothing is fabricated from a name.


| Field | Value |
|-------|-------|
| Success Criteria | A round-trip test; the pre-change fixture yields an unknown record addressing no sandbox; no path synthesises a record; a record naming another backend is refused with both names, and a sandbox remains addressable only by the backend that created it after the selection changes. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-002 |
| References | sandbox-record,sandbox-identity,legacy-record-not-adopted,backend-mismatch-refused,sandbox-missing,backend-switched |

### T-012: CLI on the contract

Route list, pause, resume, delete and shell through the registry and the record store; address sandboxes by name; derive status from the backend; remove container vocabulary from help, output and errors while keeping command and flag names.


| Field | Value |
|-------|-------|
| Success Criteria | Generic command help and output contain no container or Docker term; pause without the lifecycle facet reports unsupported with reason and remedy; status uses the contract vocabulary; exit codes come from the shared taxonomy. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-003, T-010, T-011 |
| References | cli-surface,sandbox-status,backend-neutral-contract,neutral-errors |

### T-013: Bulk pause and resume through the contract

Drive pause --all-envs from SCM branches and pause --all-repos from each backend's list, both through the lifecycle facet, skipping sandboxes that vanish mid-batch.


| Field | Value |
|-------|-------|
| Success Criteria | Both flags behave as before under Docker; a sandbox deleted mid-batch does not abort the run; no bulk path holds a backend directly; an absent facet is reported once rather than per sandbox. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-012 |
| References | bulk-operations-via-contract,bulk-pause-via-contract,bulk-and-delete-race |

### T-014: Backend introspection command

Add a backends command listing every compiled-in backend, marking the selected one, and printing readiness and capability entries, with --json emitting the same structure. Expose nothing equivalent over MCP.


| Field | Value |
|-------|-------|
| Success Criteria | Table and JSON output are tested; an unavailable backend shows its primitive, reason and remedy; no MCP tool returns the report. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-010, T-012 |
| References | backend-introspection,capability-report,introspection-works |

### T-015: MCP on the contract

Update response shapes to name, backend, status, egress and provenance by variable name; make the ports query return forwards with bind or a structured unsupported; map errors from the shared taxonomy; accept no backend settings as tool arguments.


| Field | Value |
|-------|-------|
| Success Criteria | A test enumerates the tool and argument names and finds them unchanged; no response carries a container id or an environment value; the ports query on a backend without forwarding fails unsupported. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-003, T-010, T-011 |
| References | mcp-surface,mcp-tool-names-stable,ports-honest,neutral-errors |

### T-016: Conformance suite

Add a test-only conformance module with a TestBackend implementing the contract and one facet, and a generic exercise over create, enumerate, exec, upload, download, inspect, delete, the unsupported path and create rollback. Run it for TestBackend always and for Docker under the existing opt-in.


| Field | Value |
|-------|-------|
| Success Criteria | The suite contains no backend-specific branch; Docker passes it with LITTERBOX_DOCKER_TESTS set; TestBackend exercises the unsupported path; each backend's tests remain selectable by module. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-005, T-007, T-010 |
| References | conformance-suite,docker-suite-unchanged,test-selection,docker-conformance |

### T-017: Architecture lint rules and negative fixture

Add scopes and deny rules to arch-lint.toml for sandbox, backends, backends::docker, config, persistence, cli and mcp: the contract imports nothing internal, a backend imports the contract only, the CLI and MCP import no backend module, persistence imports the contract only. Add hack/arch-lint-negative.sh that introduces a violating import, expects the check to fail, and restores the tree.


| Field | Value |
|-------|-------|
| Success Criteria | The clean tree passes arch-lint; the negative fixture fails as expected and the script succeeds only when it does; just lint runs it; the existing workflow step enforces the rules. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-012, T-015 |
| References | module-boundaries,architecture-lint,boundaries-fail-the-build,one-construction-point |

### T-018: Docs and repository configuration

Migrate this repository's .litterbox.toml to the new shape, update the configuration reference for the user file, the per-backend section and the setup-command location, add a page on choosing a backend and reading a readiness report, and regenerate the CLI and MCP references.


| Field | Value |
|-------|-------|
| Success Criteria | The repository config validates under the new binary; the docs show the user file and per-backend sections; no page states a Docker-only concept as general; regenerated references produce no diff. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-009, T-014, T-015 |
| References | documentation-parity,vocabulary-ownership,cli-surface,mcp-surface |

### T-019: Parity sweep and dead code removal

Remove what the contract supersedes: the old provider trait, SandboxMetadata, the compute accessor and the naming helpers on the public surface. Run the full suite, clippy and the lint, and confirm the generic surfaces carry no mechanism vocabulary.


| Field | Value |
|-------|-------|
| Success Criteria | Clippy with -D warnings is clean; the full suite passes; no dead code allows bypassing the registry; a grep of generic surfaces finds no container, image or Docker term outside the Docker backend. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-016, T-017, T-018 |
| References | docker-parity,vocabulary-ownership,module-boundaries,no-new-dependency |

