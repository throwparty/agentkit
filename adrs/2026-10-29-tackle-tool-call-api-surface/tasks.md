# Tackle Tool Call API Surface Tasks

## Tasks

### T-001: Naming types and strict scheme

Add `invokables/tool_name.rs` with `InvokableName`, `WireToolName`, `NamespaceSeparator`, `ToolNameScheme`, and `StrictToolNameScheme`. Implement dot-to-`__` namespace encoding, invalid-character replacement with `_`, the configured maximum length, the 8-hex SHA-256 suffix, decode, and precise errors for empty or impossible names.


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests cover the readable default, literal single and double underscores, non-ASCII input, empty names, and limits too small for a prefix plus hash. The same input and scheme always produce the same wire name. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On |  |
| References | canonical-and-wire-separation,tool-name-scheme,readable-encoding,determinism,readable-default,ambiguous-underscores,empty-name,limit-too-small,non-ascii-name,no-silent-drops |

### T-002: Remaining schemes and custom pattern

Add `GeminiToolNameScheme`, `MetaToolNameScheme`, `CohereToolNameScheme`, and `CustomToolNameScheme`. Encode and decode per scheme, including Gemini leading-character and dot rules, Meta single-dot handling, Cohere leading-digit rejection, and a custom pattern with optional maximum length.


| Field | Value |
|-------|-------|
| Success Criteria | Each scheme has round-trip and rejection tests. Custom patterns reject empty or malformed expressions. Selecting an unimplemented surface scheme is rejected by the scheme factory. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-001 |
| References | tool-name-scheme,custom-scheme-and-pattern,surface-defaults,invalid-custom-pattern,unimplemented-surface-scheme |

### T-003: Per-request ToolNameMap

Implement `ToolNameMap` with forward and reverse maps. Support forward entries for current model-invokable names and for history tool call and result names. `encode` allocates a unique wire name and appends a hash suffix on collision or truncation. `decode` consults only the current model-invokable set and returns `None` for unknown wire names.


| Field | Value |
|-------|-------|
| Success Criteria | Collisions and truncation produce distinct stable names; history names encode; unknown decode returns `None`; the map is request-local; dispatch cannot resolve a name outside the current set. |
| Complexity | 🔴 High |
| Effort | 3-5h |
| Depends On | T-001,T-002 |
| References | tool-name-map,canonical-and-wire-separation,request-local-state,collision-handling,length-and-truncation,historical-tool-not-current,unknown-call-passthrough,dispatch-safety |

### T-004: Registry render entry point

Add `RenderedTools { tools, manifest, names }` and `Registry::render_tools(scheme)`. Render model-invokable tools and the system-prompt manifest with wire names. Keep direct invocation, ACP command advertisement, and dispatch on canonical names.


| Field | Value |
|-------|-------|
| Success Criteria | Every manifest name appears in the tool definitions and vice versa. Direct invocation and command advertisement still use canonical names. Registry tests pass with the rendered entry point. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-003 |
| References | registry-render-entry-point,wire-named-manifest,manifest-matches-tools,canonical-and-wire-separation |

### T-005: Endpoint and model configuration schema

Add `ToolNamesConfig`, `ModelConfig`, and `ToolNameSchemeName`. Replace `EndpointConfig.models` with a normalized map that accepts both `models = [...]` and `models.<name>` tables. Add endpoint-level `tool_names` and per-model `models.<name>.tool_names`.


| Field | Value |
|-------|-------|
| Success Criteria | Both model forms parse to the same map. Existing list-form configurations still load. Unknown fields and invalid scheme names fail with the endpoint and model named. `pattern` without `custom` fails validation. |
| Complexity | 🔴 High |
| Effort | 3-5h |
| Depends On | T-002 |
| References | endpoint-and-model-configuration,backward-compatibility,list-compatibility,quoted-model-keys,scheme-and-pattern-conflict |

### T-006: Scheme resolution and precedence

Add `config/tool_names.rs` with `resolve_tool_names(endpoint, model)`. Resolve model override over endpoint override over wire-format default. Validate `custom` plus `pattern`, reject unimplemented surface schemes, and return a precise error naming the endpoint and model.


| Field | Value |
|-------|-------|
| Success Criteria | Unit tests cover every precedence branch, strict defaults for OpenAI chat and Anthropic messages, the Gemini default for the future Google surface, and each validation failure. |
| Complexity | 🟡 Medium |
| Effort | 2-4h |
| Depends On | T-005 |
| References | endpoint-and-model-configuration,surface-defaults,custom-scheme-and-pattern,configuration-precedence,invalid-custom-pattern,scheme-and-pattern-conflict,unimplemented-surface-scheme |

### T-007: Turn preparation and provider construction

Resolve the scheme in `prepare_turn` after `model_ref` is known. Call `Registry::render_tools`, pass the `ToolNameMap` into `provider_for`, and carry wire-named tools and system prompt in `LoopInputs`. Update the `/context` builders in ACP v1 and v2 to use the same rendered tools and manifest.


| Field | Value |
|-------|-------|
| Success Criteria | A turn uses the selected scheme; `/context` reports the same wire names as the model request; direct invocation paths remain canonical; no caller builds tools without a scheme. |
| Complexity | 🟡 Medium |
| Effort | 3-5h |
| Depends On | T-004,T-006 |
| References | registry-render-entry-point,wire-named-manifest,provider-encode-and-decode,surface-defaults,configuration-precedence |

### T-008: Provider history encoding and response decoding

Store the `ToolNameMap` on `RigProvider`. Change `rig::to_rig_message` to encode historical tool call and result names and `rig::tool_calls_of` to decode model tool calls. Leave unknown wire names unchanged so the existing unknown-invokable path handles them.


| Field | Value |
|-------|-------|
| Success Criteria | A strict-scheme turn sends safe wire names, decodes the model call to canonical, and executes the canonical tool. `ModelResponse.tool_calls` never contains a wire name. Unknown wire names and canonical-name hallucinations pass through. Provider unit tests cover history and response paths. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-003,T-007 |
| References | provider-encode-and-decode,canonical-persistence,strict-provider-tool-call,unknown-call-passthrough,model-returns-canonical-name,dispatch-safety |

### T-009: Static model list and config call sites

Update `agent/config_options.rs` to use normalized map keys for the static model fallback. Update `cli/auth.rs`, `agent/script_loop.rs`, provider tests, and any `EndpointConfig` construction from `models: Vec::new()` to the default map. Preserve discovery failure notices.


| Field | Value |
|-------|-------|
| Success Criteria | Model discovery failure still falls back to the configured static list. Notices are unchanged. All constructors compile and existing config tests pass with the list form. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-005 |
| References | endpoint-and-model-configuration,backward-compatibility,list-compatibility |

### T-010: Lossy rewrite warnings

Emit `tracing::warn!()` only for truncation and hash suffixes. Include the canonical name, wire name, endpoint, model, and surface. Deduplicate per endpoint, model, and canonical name. Routine dot and invalid-character rewrites stay silent.


| Field | Value |
|-------|-------|
| Success Criteria | Tests capture exactly one warning for a lossy name across repeated requests, no warning for a routine namespace rewrite, and the expected fields on the warning. |
| Complexity | 🟢 Low |
| Effort | 1-3h |
| Depends On | T-003,T-006 |
| References | lossy-rewrite-warnings,observability,lossy-warning-only |

### T-011: End-to-end and integration coverage

Add a turn-level test with a dotted MCP tool through a strict endpoint. Verify the model is offered a safe name, the tool executes, storage and telemetry contain only canonical names, the manifest matches the tools, unknown calls pass through, and an impossible name fails without dropping the tool.


| Field | Value |
|-------|-------|
| Success Criteria | The integration test passes against a recorded strict endpoint. No wire name appears in the session DAG. The unknown-call and impossible-name paths return the specified errors. |
| Complexity | 🔴 High |
| Effort | 4-6h |
| Depends On | T-008,T-009,T-010 |
| References | strict-provider-tool-call,canonical-persistence,manifest-matches-tools,unknown-call-passthrough,impossible-name-fails,testability,performance,dispatch-safety |

### T-012: Documentation

Update `docs/docs/user/tackle/reference/config.mdx` with endpoint and per-model `tool_names`, list and table model forms, precedence, and the custom pattern escape hatch. Update `docs/docs/user/tackle/tools.mdx` with wire naming, warnings, and strict-provider behavior.


| Field | Value |
|-------|-------|
| Success Criteria | The docs include working TOML examples for list-form models, table-form models, endpoint defaults, per-model overrides, and custom patterns. The tools page explains canonical versus wire names and the warning behavior. |
| Complexity | 🟢 Low |
| Effort | 1-2h |
| Depends On | T-006,T-008 |
| References | endpoint-and-model-configuration,surface-defaults,custom-scheme-and-pattern,lossy-rewrite-warnings,backward-compatibility |

