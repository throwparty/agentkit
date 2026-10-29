# Tackle Tool Call API Surface

**Status:** implemented  **Created:** 2026-10-29  **Author:** adrian

Tackle addresses tools by canonical invokable names: `mcp.<server>.<tool>`, `prompt.<name>`, `agent.<actor>`, and `client.<surface>.<method>`. The registry currently emits those names directly to model providers through `Registry::model_tools` and `Registry::manifest`, and the OpenAI chat-completions provider passes them to rig unchanged. Strict providers reject names containing dots: OpenAI and Anthropic require `^[a-zA-Z0-9_-]{1,64}$`, DeepSeek documents `^[a-zA-Z0-9_-]+$`, Gemini allows dots but requires a leading letter or underscore, Meta allows at most one dot, and Cohere disallows a leading digit. A dotted MCP name such as `mcp.github.search_issues` therefore fails before the model runs. A request-only Switchboard mutation cannot fix the response half because the model returns the rewritten name. This ADR refines FR-009 of `2026-09-17-tackle`, which assumed the model sees canonical namespaced names. Switchboard proxy changes are out of scope.


## Problem

Canonical invokable names are not valid tool names on several model surfaces, and the registry has no surface-aware encoding, decoding, warning, or configuration. Tool use fails on strict providers, and there is no traceability from a wire name back to its canonical invokable.

## Goals

- Keep canonical invokable names in the registry, storage, permission pipeline, and dispatch
- Render wire-safe tool names per API surface
- Decode model tool calls back to canonical names before dispatch
- Keep rendered names human-readable and troubleshooting-friendly
- Warn when a rendered name loses information
- Configure naming per endpoint and per model, with sensible wire-format defaults

## Non-goals

- Switchboard proxy changes or response rewriting
- Changing the canonical `mcp.<server>.<tool>` naming convention
- Runtime discovery of provider naming rules
- Persisting wire names in sessions or telemetry
- Encoding `tool_choice` (not present in `ModelRequest` today)
- Translating names for clients other than Tackle


## Functional Requirements

### FR-001: Canonical And Wire Separation

The registry, session storage, permission pipeline, and dispatch use `InvokableName` exclusively. `WireToolName` exists only at the model surface; no wire name is persisted or used for dispatch.

**Slug:** `canonical-and-wire-separation`

### FR-002: Tool Name Scheme

`invokables/tool_name.rs` defines `ToolNameScheme` with `encode(InvokableName) -> WireToolName` and `decode(WireToolName) -> Option<InvokableName>`, plus implementations for `strict`, `gemini`, `meta`, `cohere`, and `custom`. The scheme and map live beside the registry; provider modules consume them and do not implement naming rules.

**Slug:** `tool-name-scheme`

### FR-003: Readable Encoding

Encoding replaces each namespace dot with `__`, replaces every other character outside the scheme's allowed set with `_`, preserves readable segments, and enforces the scheme's maximum length. On collision or truncation, append `_` plus the first 8 hex characters of SHA-256 of the canonical name. The result is deterministic for a canonical name and scheme.

**Slug:** `readable-encoding`

### FR-004: Tool Name Map

`ToolNameMap` is per request. It maps canonical names from the current model-invokable set and from historical tool calls and results to wire names. Model tool calls are decoded only against the current model-invokable set; a wire name outside that set passes through unchanged to the existing unknown-invokable error path.

**Slug:** `tool-name-map`

### FR-005: Registry Render Entry Point

`Registry::render_tools(scheme)` returns `RenderedTools { tools, manifest, names }`. Tools and manifest contain wire names; `names` is the `ToolNameMap`. The registry stores no scheme. Direct invocation and ACP command advertisement continue to use canonical names.

**Slug:** `registry-render-entry-point`

### FR-006: Provider Encode And Decode

The model provider encodes historical tool call and result names with `names`, and decodes every model tool call before returning `ModelResponse`. Tool definitions arrive from `Registry::render_tools` already rendered. `ModelResponse.tool_calls[].name` is always canonical.

**Slug:** `provider-encode-and-decode`

### FR-007: Wire-Named Manifest

The system prompt invokable manifest is rendered through the same `ToolNameMap` as the tool definitions, so the model sees exactly the wire names it can call. Canonical names do not appear in model-visible text.

**Slug:** `wire-named-manifest`

### FR-008: Endpoint And Model Configuration

Configuration adds `tool_names` at the endpoint and per model: `[endpoints.<name>.tool_names]` and `[endpoints.<name>.models.<model>.tool_names]`, where the model key is a quoted TOML key. Each table accepts `scheme`, optional `max_length`, and optional `pattern`. The existing `models = [...]` list form remains valid. Precedence is model over endpoint over wire-format default.

**Slug:** `endpoint-and-model-configuration`

### FR-009: Surface Defaults

Wire-format defaults are `strict` for `openai-chat-completions` and `anthropic-messages`, `gemini` for `google-generate-content` when implemented, and `strict` for unknown formats. `strict` allows `[A-Za-z0-9_]` with a maximum of 64 characters and uses `__` for namespace boundaries.

**Slug:** `surface-defaults`

### FR-010: Custom Scheme And Pattern

`scheme = custom` requires `pattern` and may set `max_length`. `pattern` without `custom` is a configuration error. Selecting an unimplemented surface scheme, such as `gemini` before that provider exists, is a configuration error.

**Slug:** `custom-scheme-and-pattern`

### FR-011: Lossy Rewrite Warnings

A rewrite that loses information, meaning truncation or a hash suffix, emits `tracing::warn!()` with the canonical name, wire name, endpoint, model, and surface. Routine dot-to-`__` and invalid-character-to-`_` rewrites are reversible through the map and do not warn. Warnings are deduplicated per endpoint, model, and canonical name.

**Slug:** `lossy-rewrite-warnings`

### FR-012: No Silent Drops

If no unique valid wire name fits within the maximum length, the request fails with a precise error naming the canonical name, endpoint, model, and surface. Tools are never silently dropped. An empty canonical name also fails.

**Slug:** `no-silent-drops`

## Non-functional Requirements

### NFR-001: Determinism

Given the same canonical names, scheme, and tool set, encoding produces the same wire names. Encoding uses no randomness and no ambient state.

**Slug:** `determinism`

### NFR-002: Request-Local State

The `ToolNameMap` is built for one model request and discarded with it. No cross-request or process-global naming state exists.

**Slug:** `request-local-state`

### NFR-003: Backward Compatibility

Existing canonical sessions, stored tool calls, and configurations remain valid. The `models = [...]` list form parses and behaves as models with default options.

**Slug:** `backward-compatibility`

### NFR-004: Observability

Lossy rewrites log the canonical name, wire name, endpoint, model, and surface. Logs contain no credentials or tool arguments.

**Slug:** `observability`

### NFR-005: Performance

Encoding and decoding are linear in the number of tools plus history entries. They add no network calls and no additional model requests.

**Slug:** `performance`

### NFR-006: Testability

Unit tests cover every scheme and `ToolNameMap`; provider integration tests cover encode, decode, and history rewrite using recorded fixtures and no live endpoints.

**Slug:** `testability`

### NFR-007: Dispatch Safety

Only canonical names resolve in the registry. A decoded model call cannot execute an invokable that is not in the current model-invokable set.

**Slug:** `dispatch-safety`

## Acceptance Criteria

### AC-001: Strict Provider Tool Call

Against an endpoint using `strict`, a dotted MCP tool is offered under a safe wire name, the model calls that name, the provider decodes it, and the canonical tool executes.

**Slug:** `strict-provider-tool-call`

### AC-002: Canonical Persistence

A completed tool call stores only the canonical `InvokableName` in the session DAG and telemetry.

**Slug:** `canonical-persistence`

### AC-003: Manifest Matches Tools

Every wire name in the system prompt manifest appears in the tool definitions, and every tool definition appears in the manifest.

**Slug:** `manifest-matches-tools`

### AC-004: Readable Default

With no collision, `mcp.github.search_issues` renders as `mcp__github__search_issues`.

**Slug:** `readable-default`

### AC-005: Collision Handling

Two canonical names that sanitize to the same readable base receive distinct hash suffixes and distinct wire names.

**Slug:** `collision-handling`

### AC-006: Length And Truncation

A canonical name longer than the configured maximum is truncated to a readable prefix plus a hash suffix, remains within the limit, and emits a warning.

**Slug:** `length-and-truncation`

### AC-007: Configuration Precedence

A per-model override wins over an endpoint override, which wins over the wire-format default.

**Slug:** `configuration-precedence`

### AC-008: List Compatibility

A configuration using `models = [...]` parses and behaves identically to the table form with default options.

**Slug:** `list-compatibility`

### AC-009: Unknown Call Passthrough

A model call whose wire name is not in the current model-invokable set passes through unchanged and produces the existing unknown-invokable error.

**Slug:** `unknown-call-passthrough`

### AC-010: Impossible Name Fails

When no unique valid wire name fits the configured limit, the request fails with a precise error and no tool is silently dropped.

**Slug:** `impossible-name-fails`

### AC-011: Lossy Warning Only

Routine namespace and character rewrites produce no warning; truncation and hash suffixes produce exactly one deduplicated warning per endpoint, model, and canonical name.

**Slug:** `lossy-warning-only`

## Edge Cases

### EC-001: Ambiguous Underscores

Canonical components may contain single or double underscores. The per-request map resolves the ambiguity; decoding never splits the wire name on `__` alone.

**Slug:** `ambiguous-underscores`
### EC-002: Quoted Model Keys

Model names containing slashes or dots are written as quoted TOML keys, for example `[endpoints.switchboard.models.'meta/llama-4']`.

**Slug:** `quoted-model-keys`
### EC-003: Empty Name

An empty canonical name fails the request with a precise error rather than producing an empty wire name.

**Slug:** `empty-name`
### EC-004: Limit Too Small

A configured `max_length` too small to fit a readable prefix and hash suffix fails the request with a precise error.

**Slug:** `limit-too-small`
### EC-005: Historical Tool Not Current

A historical tool call whose canonical name is not in the current tool set is still encoded for history, but a new model call to that wire name passes through and hits the unknown-invokable path.

**Slug:** `historical-tool-not-current`
### EC-006: Model Returns Canonical Name

A model call that already matches a canonical name but not a current wire name is treated as unknown and passes through unchanged.

**Slug:** `model-returns-canonical-name`
### EC-007: Invalid Custom Pattern

An invalid `pattern` is rejected at configuration load with the endpoint and model named.

**Slug:** `invalid-custom-pattern`
### EC-008: Scheme And Pattern Conflict

Setting `pattern` without `scheme = custom` is rejected at configuration load.

**Slug:** `scheme-and-pattern-conflict`
### EC-009: Unimplemented Surface Scheme

Selecting `gemini` before the Gemini provider exists is rejected at configuration load rather than failing at request time.

**Slug:** `unimplemented-surface-scheme`
### EC-010: Non-ASCII Name

A canonical name containing non-ASCII characters is sanitized into the scheme's allowed set, decoded through the map, and treated as lossy only if truncation or a hash suffix is required.

**Slug:** `non-ascii-name`

