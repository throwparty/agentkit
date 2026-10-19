# Switchboard: Provider Request Mutations

**Status:** implemented **Created:** 2026-10-15 **Author:** adrian

The switchboard has grown three ad-hoc mechanisms for bending requests to a provider's quirks: a per-provider headers table, a per-provider user_agent field, and a hardcoded rewrite in routes.rs that injects tools and sets tool_choice to none on every openai-chat-completions request. The first two are static and cannot express dynamic values. The third is unconditional and therefore applied to every provider whether or not it is wanted. All three were added to work around a specific provider's requirements.

This ADR replaces them with one formal mechanism: sandboxed Rhai mutation scripts attached to a provider and executed on the assembled outbound request before credentials are injected. Scripts may set or remove headers and modify the parsed JSON request body, covering both the header and the tools-emulation cases. Provider-specific workarounds become user-supplied scripts maintained by the community at users' own risk and the switchboard ships none.

The sandboxed Rhai engine is packaged as a reusable library crate. Tackle will use the same library.

## Problem

Switchboard cannot mutate outbound provider requests in a general or dynamic way. The current static header table and user_agent field are baked into the binary and cannot express dynamic values. The hardcoded tools injection is applied to every chat-completions provider rather than the one that needs it.

## Goals

- Provide a formal reusable mechanism for mutating outbound provider requests before submission
- Support dynamic header setting and JSON request-body mutation including tool emulation
- Remove the hardcoded provider workarounds from the switchboard binary
- Delegate provider-specific workarounds to user and community maintained scripts at users' own risk
- Keep the mechanism fully sandboxed with a minimal API surface

## Non-goals

- Response mutation (request-side only)
- Mutating the request method or URL
- A mutation package registry or installer (future consideration)
- Shipping or bundling community mutation scripts
- Hot-reloading scripts at runtime

## Functional Requirements

### FR-001: Mutation Scripts

A provider declares an ordered list of Rhai script paths that are compiled at startup and executed before each outbound request to that provider

**Slug:** `mutation-scripts`

### FR-002: Script Contract

A mutation script defines a global function fn mutate(request) that receives and returns a request map with mutable headers and body plus read-only provider surface model and session_id

**Slug:** `script-contract`

### FR-003: Header Mutation

Scripts can set or remove outbound request headers dynamically

**Slug:** `header-mutation`

### FR-004: Body Mutation

Scripts can read and modify the parsed JSON request body

**Slug:** `body-mutation`

### FR-005: Tools Emulation

The hardcoded chat-completions tools and tool_choice injection is removed from routes.rs and expressed as a provider-scoped mutation script

**Slug:** `tools-emulation`

### FR-006: Static Config Removal

The per-provider headers table and the user_agent field are removed from ProviderConfig so all request mutation happens through scripts

**Slug:** `static-config-removal`

### FR-007: Sandboxed Engine

Scripts execute in a sandbox with no filesystem network or module access and no host functions while eval import and export are disabled

**Slug:** `sandboxed-engine`

### FR-008: Resource Limits

Script execution is bounded by operation call-depth expression-depth string array and map size limits and by a wall-clock timeout

**Slug:** `resource-limits`

### FR-009: Startup Compilation

Scripts are read and compiled once at startup and a missing or unparseable script aborts startup

**Slug:** `startup-compilation`

### FR-010: Pipeline Placement

Mutations run once per provider attempt on the assembled outbound request before credential injection

**Slug:** `pipeline-placement`

### FR-011: Failure Semantics

A script that errors exceeds a limit or yields an invalid request fails the request rather than forwarding it unmutated

**Slug:** `failure-semantics`

### FR-012: Script Path Resolution

Relative script paths resolve against the config file directory and multiple scripts run in declared order with later scripts observing earlier output

**Slug:** `script-path-resolution`

### FR-013: Read-Only Context

provider surface model and session_id are supplied to scripts read-only and the method and URL are never exposed

**Slug:** `read-only-context`

### FR-014: Reusable Engine Library

The sandboxed Rhai engine is packaged as a standalone library crate with no switchboard-specific types so other components can embed it

**Slug:** `engine-library`

## Non-functional Requirements

### NFR-001: No Host Capabilities

No host functions are registered so the script API is exactly the request map schema

**Slug:** `no-host-capabilities`

### NFR-002: Fail-Closed

Script failure never results in an unmutated request being forwarded upstream

**Slug:** `fail-closed`

### NFR-003: Credential Isolation

Scripts never receive credential values because mutation runs before credentials are injected

**Slug:** `credential-isolation`

### NFR-004: Bounded Execution

Per-call limits prevent a runaway script from stalling request handling

**Slug:** `bounded-execution`

### NFR-005: Self-Contained Scripts

A mutation is a single self-contained file with no imports or external dependencies

**Slug:** `self-contained-scripts`

## Acceptance Criteria

### AC-001: Script Header Mutation

A provider script that sets a header results in that header on the outbound request

**Slug:** `script-header`

### AC-002: Script Body Mutation

A provider script that adds tools and sets tool_choice produces the expected upstream body

**Slug:** `script-body`

### AC-003: Static Config Removed

ProviderConfig no longer has a headers table or a user_agent field

**Slug:** `static-config-removed`

### AC-004: Hack Removed

routes.rs contains no hardcoded tools or tool_choice injection

**Slug:** `hack-removed`

### AC-005: Sandbox Enforced

A script that attempts filesystem network eval or import access fails

**Slug:** `sandbox-enforced`

### AC-006: Limits Enforced

A script containing an infinite loop terminates with a limit error

**Slug:** `limits-enforced`

### AC-007: Fail-Closed

A script error returns an error response and no request reaches the provider

**Slug:** `fail-closed-accepted`

### AC-008: Script Ordering

With two scripts declared in order the second observes the first script's mutations

**Slug:** `script-ordering`

### AC-009: Credential Isolation

A script cannot read or alter the injected credential

**Slug:** `credential-isolation-accepted`

### AC-010: Quality Gates

cargo clippy --all-targets -- -D warnings is clean and cargo test passes

**Slug:** `quality-gates`

## Edge Cases

### EC-001: Non-JSON Body

When the request body is not JSON the body field is () and a script that mutates it fails

**Slug:** `non-json-body`

### EC-002: Missing Function

A script without a global mutate function is rejected at startup

**Slug:** `missing-function`

### EC-003: Invalid Header

A script that produces an invalid header name or value fails the request

**Slug:** `invalid-header`

### EC-004: Empty Mutation List

A provider with an empty mutation list behaves exactly as before

**Slug:** `empty-mutation-list`

### EC-005: Non-Map Return

A script that returns a non-map value fails the request

**Slug:** `non-map-return`

### EC-006: No Session

session_id is () when the client sends no X-Session-Id header

**Slug:** `no-session-id`

### EC-007: Shared Script

The same script path may be referenced by multiple providers and is compiled once

**Slug:** `shared-script`
