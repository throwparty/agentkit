---
status: draft
created: 2026-06-14
updated: 2026-06-14
author: adrian
decision: pending
---

# Specification: Model Provider SDK Selection

## 0. References

- [ACP Server Spec](../2026-04-28-acp-server/spec.md) — existing ACP harness
- [ACP Server Session Storage Spec](../2026-06-13-acp-server-session-storage/spec.md) — session storage layer
- [rig-core crate](https://crates.io/crates/rig-core) — v0.38.2
- [llm crate](https://crates.io/crates/llm) — v1.3.8
- [multi-llm crate](https://crates.io/crates/multi-llm) — v1.0.0
- [rig.rs](https://rig.rs) — project website
- [rig custom provider example](https://github.com/joshua-mo-143/rig-custom-provider-example) — documented extensibility walkthrough
- [Building modular LLM-powered apps with rig](https://www.blog.brightcoding.dev/2025/09/28/building-modular-llm-powered-apps-with-rig-a-rust-framework-overview/) — community article
- [rig-core overview](https://grokipedia.com/page/rig-core_Rust_crate) — community reference
- [Building LLM apps in Rust with rig](https://jordcodes.com/articles/rust/building-llm-apps-in-rust-with-rig) — tutorial

## 1. Problem

The ACP server (spec: `adrs/2026-04-28-acp-server/`) currently returns static text responses. To become a functional agent harness, it must call real AI models. This requires a Rust library that:

- Sits **between the harness and the AI model** — the harness owns the MCP client, tool exposure, session lifecycle, and agent loop. The SDK handles only the model interaction layer.
- Supports **multiple providers** so the harness is not locked to one vendor.
- Handles **tool calling and streaming** — the agent loop calls tools, streams responses to clients, and maintains multi-turn conversations.
- Exposes **token usage and cost data** per request and per session.
- Supports **various authentication mechanisms** (API keys, subscription tokens, provider-native auth).
- Is **extensible** — adding a new provider should not require forking the SDK.
- Supports each provider's **native API interface** — OpenAI's chat completions/Responses API, Anthropic's messages API, Google's Gemini API, Ollama's native API, etc. The library should abstract over these differences, not require all providers to speak the same wire format.

If no Rust library satisfies these requirements, we may consider WASM-interop with Vercel's AI SDK (JS), but that adds significant complexity and should be a last resort.

## 2. User Journeys

### 2.1 SDK Evaluator

**Scenario**: A developer needs to select a Rust library for interacting with AI models from within the ACP agent harness.

**Actions**:
1. Researches available Rust crates for multi-provider LLM interaction
2. Compares each candidate against the evaluation criteria (C1–C6)
3. Identifies gaps (features no library provides) that must be built in-house
4. Produces a recommendation with documented rationale

**Outcome**: A single library is selected with clear justification. Gaps are documented so the planning phase can scope the harness-level work.

### 2.2 Sample Crate Implementer

**Scenario**: A developer builds sample crates for each candidate library to compare ergonomics, API design, and feature coverage before the final SDK selection.

**Actions**:
1. Scaffolds two binary crates, one depending on rig-core and one on llm
2. In each crate, configures the OpenAI provider client
3. Defines a static tool (always returns a hardcoded result)
4. Runs a single prompt turn where the model calls the tool and responds
5. Compares API surface, error handling, compile times, binary size, and documentation quality between the two implementations

**Outcome**: The chosen SDK is validated with working code, not just documentation. The runner-up library has a comparable sample crate, so the decision is reversible if the selected library proves problematic during integration.

### 2.3 Provider Author

**Scenario**: A developer implements a new model provider not included in the SDK.

**Actions**:
1. Implements the SDK's provider trait for the new backend
2. Registers the provider alongside existing built-in providers
3. Runs the same agent loop code without modification

**Outcome**: The SDK's extensibility is validated — adding a provider does not require forking the SDK or changing calling code.

## 3. Evaluation Framework

We evaluated candidates against six weighted criteria:

| # | Criterion | Weight | Rationale |
|---|-----------|--------|-----------|
| C1 | Usage/cost reporting | High | Needed for session-level cost tracking and budget enforcement. Must be auth-aware — cost depends on which auth method (subscription vs. pay-as-you-go) is used for the session |
| C2 | Agent interaction modeling | High | Tool calling, multi-turn, streaming are core to agent functionality |
| C3 | Multi-provider support | High | Avoid vendor lock-in; each provider has its own wire format (chat completions, messages API, Gemini API, etc.) and the library must abstract over them |
| C4 | Provider extensibility | Medium | We may need to add niche providers or custom wrappers |
| C5 | Ecosystem fit | Medium | Async runtime, active maintenance, WASM cross-compilation, community health |
| C6 | Load balancing | Low | Ability to distribute requests across multiple provider instances or fall back between providers. Currently a nice-to-have; may become core later |

## 4. Candidates

### 4.1 rig (rig-core)

**Source**: [github.com/0xPlaygrounds/rig](https://github.com/0xPlaygrounds/rig)
**Version**: 0.38.2 (pre-1.0, breaking changes expected)
**License**: MIT
**Stars**: 7,600+
**Production users**: St. Jude, ilert, Neon, Ryzome, Nethermind

**Provider support (24 built-in)**:

Anthropic, Azure OpenAI, ChatGPT (OAuth), Cohere, Copilot (OAuth), DeepSeek, Galadriel, Gemini, Groq, HuggingFace, Hyperbolic, Llamafile, MiniMax, Mira, Mistral, Moonshot, Ollama, OpenAI (Responses + Completions API), OpenRouter, Perplexity, Together, VoyageAI, xAI, Xiaomi MiMo, Z.ai.

**Authentication**:
- API keys via `BearerAuth` trait (OpenAI, most providers)
- Custom headers via `ApiKey` trait (Anthropic uses `x-api-key`)
- OAuth/session tokens via ChatGPT and Copilot provider modules
- Builder pattern for provider-specific auth configuration
- Environment variable auto-detection (`from_env()`) and explicit construction (`from_val()`)

**Agent interaction**:
- `Agent` type with `prompt()`, `chat()`, `prompt_typed()` methods
- Multi-turn tool calling with configurable max turns
- Streaming via SSE (`stream()` on `CompletionModel`)
- Tool definition via `ToolDefinition` struct with JSON Schema parameters
- Provider-managed tools via `ProviderToolDefinition`

**Usage tracking**:
- `CompletionResponse.usage` field returns `Usage { input_tokens, output_tokens, total_tokens, cached_input_tokens, cache_creation_input_tokens, tool_use_prompt_tokens, reasoning_tokens }`
- `Usage` implements `Add` and `AddAssign` for cumulative session tracking
- `GetTokenUsage` trait for streaming response accumulation
- Full GenAI OpenTelemetry semantic convention compatibility

**Cost tracking**: Not built-in. Raw token counts are available; cost estimation would be our layer.

**Custom provider implementation**:
- `CompletionModel` trait with associated types `Response`, `StreamingResponse`, `Client`
- `EmbeddingModel` trait for embeddings
- `Provider` + `ProviderBuilder` traits for client-level integration
- Documented custom provider example: [rig-custom-provider-example](https://github.com/joshua-mo-143/rig-custom-provider-example)
- Type-level capability gating via `Capable<T>` / `Nothing` markers

**Ecosystem fit**:
- Tokio-native async throughout
- WASM-compatible core library
- Cassette-based test infrastructure (replay offline, live-only separate)
- Large community; 7,600+ GitHub stars, 800+ forks

### 4.2 llm (graniet/llm)

**Source**: [github.com/graniet/llm](https://github.com/graniet/llm)
**Version**: 1.3.8 (stable)
**License**: MIT
**Stars**: 351

**Provider support (12+)**:

OpenAI, Anthropic, Ollama, DeepSeek, xAI, Phind, Groq, Google (Gemini), Cohere, Mistral, HuggingFace, ElevenLabs.

**Authentication**:
- Builder pattern with `.api_key()` method
- Environment variable per provider
- Basic API keys only — no OAuth or subscription token support

**Agent interaction**:
- `chat()`, `completion()`, streaming per provider
- Tool calling with unified interface
- Multi-step chains (different backends per step)
- Conversation memory with sliding window
- Reactive agent builder (`agent` feature)

**Usage tracking**:
- `response.usage()` returns `prompt_tokens` and `completion_tokens`
- Basic token counts only — no caching, reasoning, or tool-use breakdown

**Cost tracking**: None.

**Custom provider implementation**:
- `LLMProvider` trait combining `ChatProvider` + `CompletionProvider` + `EmbeddingProvider`
- Feature-gated per provider via Cargo features
- Less documented than rig's approach

**Ecosystem fit**:
- Tokio-native async
- No WASM support
- Feature-gated compilation reduces binary size
- Moderate community (351 stars)

### 4.3 multi-llm (darval/multi-llm)

**Source**: [github.com/darval/multi-llm](https://github.com/darval/multi-llm)
**Version**: 1.0.0 (stable)
**License**: Apache-2.0
**Downloads**: 85 total

**Provider support (4)**:

OpenAI, Anthropic, Ollama, LM Studio.

**Authentication**:
- Configuration structs with `api_key` field per provider

**Agent interaction**:
- Tool calling supported
- Streaming: **deferred to post-1.0** — no streaming support
- Prompt caching (Anthropic 5-min and 1-hour)

**Usage tracking**:
- Optional feature-gated events system (`features = ["events"]`)
- Events include `CacheHit`, `TokenUsage`, etc.

**Cost tracking**: None.

**Custom provider implementation**:
- `LlmProvider` trait
- Simple but limited — only 4 providers implemented

**Ecosystem fit**:
- Tokio-based async
- No WASM support
- Single author, very low adoption
- Requires Rust 1.75+

**Verdict: Ruled out.** Multi-llm is eliminated on maintenance grounds. The crate has 85 total downloads, a single maintainer, and has seen no commits or releases since November 2025. Streaming is deferred to a post-1.0 release date that has not materialised. The 4-provider ceiling (no Gemini, no local-first path beyond Ollama) is too narrow for our requirements. There is no path to widening provider coverage without forking the crate.

### 4.4 Others Briefly Considered

These candidates were evaluated and ruled out with minimal investigation due to obvious disqualifying traits:

- **allms** (crates.io): Unclear maintainer, hard to find source repository. Skipped.
- **rllm** (graniet/rllm): A wrapper around the `llm` crate (already evaluated), adds nothing new.
- **rust-genai** (jeremychone): Marked experimental by author, limited provider coverage.
- **llm_client** (ShelbyJenkins): Project re-focused to llama.cpp only, no longer multi-provider.

## 5. Comparative Analysis

| Criterion | rig | llm | multi-llm |
|-----------|-----|-----|-----------|
| **C1: Usage reporting** | ✅ Detailed (input/output/cached/reasoning/tool tokens, Add impl, OTel) | ⚠️ Basic (prompt_tokens, completion_tokens) | ⚠️ Feature-gated events |
| **C1: Cost reporting** | ❌ (tokens only) | ❌ | ❌ |
| **C2: Tool calling** | ✅ Multi-turn, typed, provider-managed tools | ✅ Unified tool calling | ✅ First-class |
| **C2: Streaming** | ✅ SSE streaming | ✅ Per-provider streaming | ❌ Post-1.0 |
| **C2: Multi-turn agents** | ✅ Agent type with multi-turn, memory | ✅ Memory + agent builder | ❌ |
| **C3: Provider count** | 24 | 12 | 4 |
| **C3: Provider breadth** | ✅ OpenAI + Anthropic + Gemini + local (Ollama) | ✅ Same core set | ⚠️ Missing Gemini, others |
| **C3: Subscription auth** | ✅ ChatGPT/Copilot OAuth | ❌ | ❌ |
| **C4: Custom provider** | ✅ Documented example, type-safe traits | ✅ Basic traits | ✅ Basic traits |
| **C5: WASM** | ✅ Core library | ❌ | ❌ |
| **C5: Stability** | ⚠️ Pre-1.0 (v0.38) | ✅ v1.3.8 | ✅ v1.0.0 |
| **C5: Community** | Large (7.6k ★, 848 forks) | Medium (351 ★) | Tiny (single author) |
| **C5: Maintenance** | Very active (multiple releases/month) | Active | Minimal |
| **C6: Load balancing** | ❌ (manual multi-client only) | ❌ | ❌ |

### 5.1 Key Insight: Auth-Aware Cost Tracking

None of the three candidates provide built-in cost estimation or pricing tables. All return raw token counts. Implementing cost estimation is necessary regardless of SDK choice, but it must be **auth-aware**: the same model may have different pricing depending on whether the session uses a subscription (flat-rate, per-seat) vs. pay-as-you-go (per-token) billing. For example:

- GitHub Copilot subscriptions provide a flat-rate per-seat with usage limits enforced by quotas, not token pricing.
- OpenAI API keys charge per-token with per-model rates.
- Anthropic API keys charge per-token with cache-based discounts.

This means the cost estimation layer must know the authentication method used for a given session to select the correct pricing model. Neither candidate addresses this — it is harness-level logic regardless of SDK choice.

Raw token counts are still essential input. rig's `Usage` struct provides the most detailed breakdown (input, output, cached, reasoning, tool-use tokens) with `Add`/`AddAssign` for cumulative session tracking.

### 5.2 Key Insight: Auth Flexibility

Only rig supports multiple auth patterns (Bearer token, custom headers, OAuth/session tokens) through its provider-independent `ApiKey` trait and provider-specific builders. The ChatGPT and Copilot provider modules demonstrate subscription-style auth. This is important because the ACP harness may need to use OAuth tokens (e.g., for GitHub Copilot) alongside API keys, and the auth method feeds into cost calculation.

### 5.3 Key Insight: Load Balancing Gap

None of the candidates provide built-in load balancing, failover, or request distribution across multiple provider instances. They are all single-client, single-provider libraries. This is not a blocker — load balancing can be implemented as a harness-level policy that wraps multiple provider clients and selects one per request based on cost, availability, or latency. If this becomes core functionality, implementing it on top of a library's provider trait is straightforward since the trait provides a uniform interface across instances.

Rig's `ClientBuilder` supports constructing multiple clients for the same provider with different configurations (e.g., different API keys or base URLs), which gives us the building blocks for manual load distribution.

## 6. Recommendation

Recommendation is provisional based on documentation research. Before committing to rig, build sample crates for both rig and llm (see §10) to validate API ergonomics, compile times, tool calling behaviour, and provider swap mechanics with real code.

**Provisionally select rig (rig-core).**

Rationale by criterion:

- **Usage reporting** (C1): rig's `Usage` struct is the most detailed available, with separate fields for input, output, cached, reasoning, and tool-use tokens. The `Add`/`AddAssign` impls make cumulative session tracking straightforward. Only token counts are missing cost estimation, which we implement as our own layer.

- **Agent interaction** (C2): rig's `Agent` type with multi-turn tool calling, streaming, and conversation memory directly matches our needs. The `CompletionModel` trait maps cleanly to our provider abstraction.

- **Multi-provider support** (C3): 24 providers is the widest coverage. Crucially includes subscription/OAuth auth via ChatGPT and Copilot providers.

- **Provider extensibility** (C4): The `CompletionModel` trait + documented custom provider example means we can add providers without forking. The type-level capability gating (`Capable<T>`) prevents runtime errors for unsupported features.

- **Ecosystem fit** (C5): WASM support for future browser-based scenarios, active maintenance, large community, and production users in similar domains (coding agents, LLM proxies) reduce risk despite pre-1.0 status.

- **Load balancing** (C6): No candidate supports this natively. rig's `ClientBuilder` makes it straightforward to construct multiple clients (same provider, different configs) that can be wrapped in a harness-level router. This is adequate for the foreseeable future.

### 6.1 Risk: Pre-1.0 Stability

Rig is at v0.38 and explicitly warns of breaking changes. Mitigations:
- Pin to a specific version in `Cargo.toml`
- Wrap rig types behind our own provider trait so swapping the implementation is a single-module change
- Monitor changelog for API breaks
- Rig's production users (St. Jude, ilert, Neon) indicate the API is usable despite version number

### 6.2 Risk: No Built-in Cost Tracking

Addressed by implementing a `CostTracker` layer that maps `Usage` → estimated cost. This layer must be **auth-aware**: the same model may have different pricing depending on whether the session uses subscription billing (flat-rate quotas) or pay-as-you-go (per-token rates). The auth method for the session must be available to the cost calculation logic. This is harness-level work regardless of SDK choice.

Cost tracking is deferred to a later phase — it is not part of the sample crates.

## 7. Formal Requirements

### FR1: Multi-Provider Completion

**Description**: The harness must send prompts to and receive responses from multiple AI model providers through a unified interface.

**Acceptance Criteria**:
- AC1.1: Agent can complete a text prompt with OpenAI (GPT-4o or newer) and receive a response
- AC1.2: Agent can complete a text prompt with Anthropic (Claude 3.5+ or newer) and receive a response
- AC1.3: Agent can complete a text prompt with Ollama (local model) and receive a response
- AC1.4: Switching providers requires changing only the provider client configuration, not the calling code

### FR2: Tool/Function Calling

**Description**: The model can request tool calls, and the harness can execute them and return results.

**Acceptance Criteria**:
- AC2.1: Agent can define tools with JSON Schema parameters
- AC2.2: Agent sends tool definitions to the model in completion requests
- AC2.3: When the model requests a tool call, the harness can execute the tool and feed results back
- AC2.4: Multi-turn tool use works (model calls tool → tool returns → model uses result)
- AC2.5: Agent terminates when `stopReason: "end_turn"` is received or max turns reached

### FR3: Streaming Responses

**Description**: Model responses must be streamable to enable real-time UX in the ACP server's `session/update` notifications.

**Acceptance Criteria**:
- AC3.1: Agent can initiate a streaming completion request
- AC3.2: Stream yields content chunks as they arrive from the provider
- AC3.3: Stream is consumable both as text deltas and as accumulated response
- AC3.4: Token usage is accessible after stream completion

### FR4: Token Usage Reporting

**Description**: Each completion request returns token usage data suitable for cost tracking.

**Acceptance Criteria**:
- AC4.1: Response includes input token count
- AC4.2: Response includes output token count
- AC4.3: Response includes total token count
- AC4.4: Cached input tokens are reported separately (for Anthropic)
- AC4.5: Reasoning tokens are reported separately (for models that support it)
- AC4.6: Usage values can be accumulated across a session

### FR5: Provider Authentication

**Description**: The SDK must support the authentication mechanisms required by each provider.

**Acceptance Criteria**:
- AC5.1: Bearer token / API key auth works for OpenAI
- AC5.2: Custom header auth (x-api-key) works for Anthropic
- AC5.3: No-auth (no API key) works for local providers like Ollama
- AC5.4: Credentials can be provided via environment variables or explicit values

### FR6: Custom Provider Implementation

**Description**: Adding a new provider should be possible without modifying the SDK.

**Acceptance Criteria**:
- AC6.1: SDK exposes a trait that can be implemented outside the SDK crate
- AC6.2: Implementing the trait for a new provider requires less than 200 lines of code for a basic text completion provider
- AC6.3: The custom provider can be used with the same calling code as built-in providers

### FR7: Conversation Memory

**Description**: The SDK must support maintaining conversation history across turns.

**Acceptance Criteria**:
- AC7.1: Chat history can be accumulated across multiple turns in a session
- AC7.2: History can be truncated (sliding window) when token limits are approached
- AC7.3: System messages and tool results are correctly interleaved with user/assistant messages

### NFR1: Async Runtime

**Description**: All SDK operations must be async and compatible with tokio.

**Acceptance Criteria**:
- All completion, streaming, and embedding operations return `Future` types
- No blocking calls on the tokio runtime

### NFR2: Binary Size

**Description**: SDK dependencies should not bloat the final binary unnecessarily.

**Acceptance Criteria**:
- Binary with one provider enabled stays under 15MB (stripped)
- Feature flags control which provider implementations are compiled

### NFR3: Error Handling

**Description**: Provider errors (network, auth, rate limits, model unavailable) must be surfaced clearly.

**Acceptance Criteria**:
- NFR3.1: HTTP 4xx/5xx responses are surfaced as typed errors
- NFR3.2: Network timeouts are distinguishable from provider errors
- NFR3.3: Rate limit errors include retry-after information if the provider supplies it
- NFR3.4: Invalid authentication produces a distinct error type

### NFR4: Testability

**Description**: Tests must not require real API keys for basic correctness.

**Acceptance Criteria**:
- NFR4.1: SDK supports request/response fixtures or HTTP mocks for offline testing
- NFR4.2: Offline tests validate serialization and deserialization
- NFR4.3: CI runs offline tests without network access

## 8. Edge Cases and Failure Handling

### 7.1 Provider Rate Limiting

The SDK should surface HTTP 429 responses as rate-limit errors. The harness (not the SDK) is responsible for retry logic and backoff.

### 7.2 Model Context Window Exceeded

If the conversation history exceeds the model's context window, the SDK should surface this as an error. The harness is responsible for context window management (truncation, summarization).

### 7.3 Streaming Interruption

If a streaming connection drops mid-response, the SDK should surface the partial response before the error. The harness decides whether to retry or surface the partial result.

### 7.4 Invalid Tool Calls

If the model returns a malformed tool call (invalid JSON arguments, unknown tool name), the SDK should surface this as an error with the raw tool call data so the harness can respond with a tool error message.

### 7.5 Authentication Failure

If provider credentials are missing or invalid, the SDK should fail fast at client construction time (not on first request) where possible.

### 7.6 Unknown Model

If the requested model is not available from the configured provider, the SDK should surface the provider's error message.

## 9. Out of Scope (for this ADR)

- **Cost estimation logic**: We will implement cost estimation as our own layer on top of the chosen SDK. The model pricing table and cumulative cost tracking belong in the harness, not the SDK.
- **Retry/fallback logic**: The harness owns retry policies, fallback between providers, and circuit breaking.
- **Session storage**: The `acp-storage` crate (see `adrs/2026-06-13-acp-server-session-storage/`) handles persistence of conversation history.
- **MCP client**: The harness implements its own MCP client for tool exposure; the model SDK only communicates with AI model APIs.

## 10. Traceability

| Requirement | Covers | Verification artifact |
|-------------|--------|----------------------|
| FR1 (multi-provider) | C3 | Integration tests with OpenAI + Anthropic + Ollama |
| FR2 (tool calling) | C2 | Multi-turn agent test |
| FR3 (streaming) | C2 | Streaming response test |
| FR4 (usage tracking) | C1 | Assert non-zero token counts in response |
| FR5 (auth) | C3, C5 | Client construction with various auth inputs |
| FR6 (custom provider) | C4 | Implement a dummy provider in 200 lines |
| FR7 (memory) | C2 | Multi-turn conversation history test |
| NFR1 (async) | C5 | Compilation with tokio test |
| NFR2 (binary size) | C5 | Measure release binary with `du` |
| NFR3 (errors) | C3 | Error type tests |
| NFR4 (testability) | C5 | Offline fixture tests pass in CI |
| C6 (load balancing) | C6 | Multiple clients wrapping same provider trait |
| Sample crate validation | C1–C5 | Side-by-side comparison of rig and llm crates covering compile times, binary size, tool call round-trip, error messages, documentation quality |

## 11. Next Steps (Sample Crates)

Before committing to a final SDK selection, build sample crates for the two leading candidates (rig and llm). Each crate is a standalone binary that validates the library can do the basic agent loop. This makes the comparison concrete and keeps the final decision reversible.

### Scope per crate

Both crates demonstrate the same capabilities with their respective library:

1. **A single prompt turn**: Operator sends a message → model responds.
2. **A tool call round-trip**: Model requests a tool → the crate executes a statically defined tool (returns a hardcoded result) → model receives the result and produces a final response.
3. **Token usage reporting**: Usage struct inspected and logged after each turn.

### Crate structure

```
adrs/2026-06-14-model-provider-sdk/
├── rig-sample/          # depends on rig-core
│   ├── Cargo.toml
│   ├── src/main.rs      # CLI arg parsing, agent loop, tool definitions
│   └── tests/
│       ├── cassette.rs  # offline replay test (rig's built-in cassettes)
│       └── live.rs      # live integration (gated with #[ignore])
├── llm-sample/          # depends on llm
│   ├── Cargo.toml
│   ├── src/main.rs
│   └── tests/
│       ├── mock.rs      # offline test via HTTP mocking
│       └── live.rs      # live integration (gated with #[ignore])
└── README.md            # comparison findings, recommendation confirmation or reversal
```

*(Note: crates live under `adrs/2026-06-14-model-provider-sdk/`, not `samples/`.)*
```

### What the samples explicitly avoid

- Session persistence (that's the `acp-storage` crate's job)
- Cost tracking (requires pricing tables and auth-aware logic — separate work)
- Streaming (adds complexity; validate non-streaming first)
- Chain-of-thought, prompt caching, or other provider-specific minutiae
- Multi-client load balancing
- Abstracting behind a common trait (each crate uses the library's own types directly — abstraction is a separate phase)

### Testing

- rig-sample: offline test with recorded cassettes (rig's built-in cassette infrastructure)
- llm-sample: offline test with HTTP mocking (e.g., `wiremock` — llm has no cassette system)
- Live integration test with a real OpenAI API key for each crate, gated behind `#[ignore]`

### Deliverables

1. `adrs/2026-06-14-model-provider-sdk/rig-sample/` crate using rig-core with the working agent loop
2. `adrs/2026-06-14-model-provider-sdk/llm-sample/` crate using llm with the working agent loop
3. `adrs/2026-06-14-model-provider-sdk/README.md` documenting side-by-side comparison of compile times, binary size, tool call behaviour, error messages, documentation quality, and final recommendation
4. Offline tests for both crates that run in CI without network access
5. Core API surface notes identifying traits and types that would need to be abstracted for harness integration
