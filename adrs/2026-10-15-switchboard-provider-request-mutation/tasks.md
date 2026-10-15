# Switchboard: Provider Request Mutations

## Tasks

### T-001: [x] Create agentkit-script crate and Limits

Add crates/agentkit-script to the workspace with Cargo.toml (rhai 1.26 with serde, sync, no_module, no_closure, no_time; serde_json; thiserror) and src/lib.rs, src/engine.rs, src/limits.rs, src/script.rs, src/error.rs. Define the Limits struct with the documented defaults and a Default impl.


| Field | Value |
|-------|-------|
| Success Criteria | cargo build -p agentkit-script succeeds; Limits::default() matches the documented values; the crate has no dependency on agentkit-switchboard. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On |  |
| References | engine-library, resource-limits |

### T-002: [x] Build the sandboxed engine and compile scripts

Implement ScriptEngine::new, with_limits, limits, compile, and compile_file. Construct the engine with Engine::new_raw plus StandardPackage (no FileModuleResolver), register no host functions, disable eval/import/export, and apply every Limits value via set_max_operations, set_max_call_levels, set_max_expr_depths, set_max_string_size, set_max_array_size, set_max_map_size, and set_max_strings_interned. compile_file reads the file and reports Read or Compile errors with the origin.


| Field | Value |
|-------|-------|
| Success Criteria | A script compiles; a missing file yields Read; a syntax error yields Compile; eval and import are rejected at compile time; ScriptEngine is Send + Sync. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-001 |
| References | sandboxed-engine, no-host-capabilities, self-contained-scripts, startup-compilation, engine-library |

### T-003: [x] Implement call with conversion and timeout

Implement ScriptEngine::call. Check defines(function, 1) first and return MissingFunction otherwise. Convert the JSON input with rhai::serde::to_dynamic, run engine.call_fn::<Dynamic> with a fresh Scope and a thread_local wall-clock deadline checked from on_progress, then flatten and convert back with rhai::serde::from_dynamic. Map Rhai errors and termination tokens to ScriptError variants.


| Field | Value |
|-------|-------|
| Success Criteria | A script returning a map round-trips through JSON; a missing function yields MissingFunction; a non-map return yields InvalidReturn; an infinite loop terminates as LimitExceeded; a slow script terminates as Timeout. |
| Complexity | 🔴 High |
| Effort | 3-4h |
| Depends On | T-002 |
| References | script-contract, resource-limits, bounded-execution, missing-function, non-map-return, engine-library |

### T-004: [x] Unit test the scripting engine

Add unit tests in agentkit-script covering compile and call, missing function, runtime error, infinite loop hitting max_operations, string and array and map size limits, eval and import blocked, wall-clock timeout, and non-map return. Assert that no filesystem or network access is possible.


| Field | Value |
|-------|-------|
| Success Criteria | cargo test -p agentkit-script passes; each sandbox and limit behaviour has a dedicated test. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-003 |
| References | sandbox-enforced, limits-enforced, missing-function, non-map-return, self-contained-scripts |

### T-005: [x] Change ProviderConfig to mutations

Remove headers and user_agent from ProviderConfig and add mutations: Vec<PathBuf> with a serde default. Update the loader to resolve relative paths against the config file directory. Reject a config that still supplies the removed fields.


| Field | Value |
|-------|-------|
| Success Criteria | ProviderConfig no longer has headers or user_agent; a config with mutations parses; relative paths resolve against the config directory; a config using the removed fields fails to parse. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On |  |
| References | static-config-removal, static-config-removed, mutation-scripts, script-path-resolution |

### T-006: [x] Implement the mutation stage

Add src/proxy/mutation.rs with MutationSet and apply. Build the request object map from provider identity, surface, model, session_id, headers, and the parsed body; call each compiled script in order; read back headers and body; validate header names and values; serialise the body. Return a MutationError on any failure.


| Field | Value |
|-------|-------|
| Success Criteria | A script that sets a header and adds tools is reflected in the returned headers and body; two scripts run in order; an invalid header fails; a non-JSON body is passed as unit. |
| Complexity | 🔴 High |
| Effort | 3-4h |
| Depends On | T-003, T-005 |
| References | header-mutation, body-mutation, script-contract, read-only-context, failure-semantics, fail-closed, invalid-header, non-json-body, script-ordering |

### T-007: [x] Compile mutations at startup

Build one ScriptEngine at startup and compile every provider's mutations once into a HashMap<String, MutationSet> held in AppState. Abort startup with the provider identity and origin on a missing file, a parse error, or a script without fn mutate. Compile a shared script path once even when referenced by multiple providers.


| Field | Value |
|-------|-------|
| Success Criteria | Startup fails with a clear error for a bad script; a valid config starts; a provider with no mutations gets an empty set; a shared script compiles once. |
| Complexity | 🟡 Medium |
| Effort | 2-3h |
| Depends On | T-005, T-006 |
| References | startup-compilation, mutation-scripts, script-path-resolution, shared-script, empty-mutation-list |

### T-008: Integrate mutations into the forwarder and remove the hack

Replace ForwardRequest's provider_user_agent and provider_headers with mutations: Option<&MutationSet> and a surface. In forward_request, assemble out_headers, call MutationSet::apply, then inject credentials last. Delete the hardcoded tools and tool_choice rewrite and the unconditional body mutation from routes.rs.


| Field | Value |
|-------|-------|
| Success Criteria | Mutations run per provider attempt before credential injection; credentials always win; routes.rs contains no hardcoded tools or tool_choice; no mutations preserves passthrough. |
| Complexity | 🔴 High |
| Effort | 3-4h |
| Depends On | T-006, T-007 |
| References | pipeline-placement, credential-isolation, tools-emulation, hack-removed, empty-mutation-list, no-session-id |

### T-009: Switchboard mutation integration tests

Add tests for header mutation reaching upstream, body tools mutation reaching upstream via a fixture script under tests/fixtures/mutations/, script ordering, invalid header failing the request, fail-closed on script error, credential isolation, and no-mutation passthrough. Replace proxy_request_body_tools_always_injected with a provider-scoped mutation test.


| Field | Value |
|-------|-------|
| Success Criteria | cargo test -p agentkit-switchboard passes; the removed hack test is replaced; each acceptance criterion has a test. |
| Complexity | 🟡 Medium |
| Effort | 3-4h |
| Depends On | T-008 |
| References | script-header, script-body, hack-removed, fail-closed-accepted, credential-isolation-accepted, script-ordering, invalid-header, non-json-body, empty-mutation-list |

### T-010: Update docs and run quality gates

Remove the stale custom-headers documentation and document the mutations field and the fn mutate contract. Run cargo clippy --all-targets -- -D warnings and cargo test across the workspace.


| Field | Value |
|-------|-------|
| Success Criteria | No docs describe [providers.headers] or user_agent; the mutations field and script contract are documented; clippy -D warnings is clean; the full test suite passes. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-008, T-009 |
| References | quality-gates, static-config-removal, mutation-scripts, script-contract |

