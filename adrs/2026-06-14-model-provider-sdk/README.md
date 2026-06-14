# Model Provider SDK: rig vs llm — Side-by-Side Comparison

## Executive Summary

**Recommendation: rig-core (confirmed).** After building working agent loops in both crates with tool calling, multi-turn history, and OpenAI provider configuration, rig is the stronger choice for the ACP harness. It offers better testability (built-in cassette system), richer usage tracking (7 fields vs 2), wider provider coverage (24 vs 12), WASM support, a larger community, and a smaller binary (11MB vs 28MB). The pre-1.0 version risk is mitigated by pinning and wrapping.

---

## Comparison Table

| Dimension | rig-sample | llm-sample |
|---|---|---|
| **Source LoC** (lib + main) | 128 | 124 |
| **Source LoC** (incl. testing infra) | 342 | 124 |
| **Release binary size** (unstripped) | 11 MB | 28 MB |
| **Provider count** | 24 | 12+ |
| **Tool calling** | Automatic (built-in `Agent.chat()`) | Manual loop required |
| **Usage detail** | 7 fields (input, output, total, cached_input, cache_creation, tool_use, reasoning) | 2 fields (prompt_tokens, completion_tokens) |
| **Offline testing** | Built-in cassettes (HTTP recording/replay via `HttpClientExt` trait) | No custom HTTP client injection — requires `wiremock` or proxy |
| **WASM support** | Yes (core library) | No |
| **Version stability** | Pre-1.0 (v0.38.2) | Stable (v1.3.8) |
| **Community** | 7,600+ stars, 800+ forks | 351 stars |
| **Documentation** | Excellent — `rig.rs` website, multiple tutorials, doc-comments with examples | Adequate — docs.rs with basic doc-comments, fewer examples |
| **Error handling** | Typed per-provider errors via `PromptError` | Unified `LLMError` enum with clear variants |
| **Custom HTTP client** | Yes — `ClientBuilder.http_client(impl HttpClientExt)` | No — reqwest client is internal |
| **Auth patterns** | Bearer, custom header, OAuth (Copilot, ChatGPT providers) | Bearer only (`.api_key()`) |

---

## Key Differences

### Tool Calling: Automatic vs Manual

**rig** provides an `Agent` type with a built-in tool execution loop:
```rust
// One call — rig handles tool execution and re-invocation internally
agent.chat(prompt, &mut history).await?;
```

**llm** returns tool calls from `chat_with_tools()` and expects the caller to iterate manually:
```rust
loop {
    let response = provider.chat_with_tools(&history, None).await?;
    if let Some(tool_calls) = response.tool_calls() {
        // execute tools, push results, continue loop
    } else {
        break; // final text response
    }
}
```

For the ACP harness (which owns its own agent loop, tool execution, and MCP integration), the manual approach is actually *preferable* — the harness needs to control tool execution, session state, and termination criteria. rig's auto-loop fights that. However, rig's `Agent` can be bypassed by using lower-level `CompletionModel::chat()` directly.

### Usage Tracking: rig's Advantage

rig exposes 7 usage fields vs llm's 2:

| Field | rig | llm |
|---|---|---|
| Input tokens | ✅ | ✅ |
| Output tokens | ✅ | ✅ |
| Total tokens | ✅ | ❌ |
| Cached input tokens | ✅ | ❌ |
| Cache creation tokens | ✅ | ❌ |
| Tool-use prompt tokens | ✅ | ❌ |
| Reasoning tokens | ✅ | ❌ |
| `Add`/`AddAssign` impl | ✅ | ❌ |

For the harness's cost tracking and session management requirements (FR4), rig's richer data is directly useful. llm's 2-field minimum meets the basic requirement but lacks detail needed for accurate cost estimation (cached vs non-cached tokens have different pricing).

### Testability: Cassettes vs No HTTP Injection

**rig** exposes an `HttpClientExt` trait that allows injecting a custom HTTP client. This enabled building a `RecordingClient` that records/replays HTTP interactions as cassettes without a mock server:
- Record: `RECORDING=true cargo test -- --ignored`
- Replay: `cargo test` (no network, 50ms)
- The cassette file (5 entries for the echo tool round-trip) is committed to the repo

**llm** constructs its own `reqwest::Client` internally with no injection point. The only way to mock HTTP is to use a tool like `wiremock` that binds to a local port, or set up a proxy. This adds complexity to CI and test setup.

### Binary Size: 2.5x Difference

| | debug | release (unstripped) |
|---|---|---|
| rig-sample | ~30 MB | 11 MB |
| llm-sample | ~200+ MB | 28 MB |

The llm crate pulls in heavyweight dependencies (`awc` — actix web client) even with only the `openai` feature enabled. The `awc` crate in turn depends on `actix-http`, `actix-tls`, `actix-codec`, `actix-rt`, `bytestring`, and the `tokio-rustls` / `rustls` ecosystem. These dependencies are unused when using the reqwest-based OpenAI provider, but are compiled regardless.

### Observations

**rig**:
- Agent's auto tool loop is convenient but runs `execute_tool` internally — the harness cannot intercept, validate, or modify tool calls before execution
- The `Prompt` trait is single-turn only; multi-turn requires `Agent.chat()` or direct `CompletionModel` usage
- Cassette system works well but the `RecordingClient` must implement `send_multipart` and `send_streaming` (which return errors — unused by the sample)
- `Usage` struct and `Add` impl make cumulative session tracking trivial
- Custom HTTP client injection via `HttpClientExt` is a major testability win

**llm**:
- Manual tool loop gives the harness full control over the tool execution lifecycle
- `LLMProvider` trait also exposes streaming, embedding, TTS, STT, and model listing — more than rig's `CompletionModel` alone
- `chat_with_tools` requires the caller to pass `tools: Option<&[Tool]>` explicitly, but falls back to builder-configured tools when `None`
- No custom HTTP client injection possible; testability suffers
- Even with only `features = ["openai"]`, the crate pulls in `awc` and `actix-*` deps, inflating binary size and compile time
- The `UserLocation` struct is public but only used internally for web search — a minor API inconsistency
- No WASM support limits future flexibility

---

## Recommendation

**Select rig-core.** The recommendation from the spec (§6) is confirmed after hands-on validation.

The deciding factors are:

1. **Testability (NFR4)**: rig's `HttpClientExt` trait enables cassette-based testing without network or mock servers. llm's lack of HTTP client injection is a hard blocker for reliable offline testing.

2. **Usage detail (FR4)**: rig's 7-field `Usage` struct with `Add`/`AddAssign` provides the granularity needed for accurate cost estimation. llm's 2 fields are insufficient for cache-aware or reasoning-aware pricing.

3. **Binary size (NFR2)**: rig (11MB) is 2.5x smaller than llm (28MB) with one provider enabled. This gap widens when more debug info is retained.

4. **WASM (future)**: rig's WASM support keeps the door open for browser-based scenarios without a library swap.

5. **Community and maintenance**: rig's 7,600+ stars, active development (multiple releases/month), and production users provide confidence despite pre-1.0 status.

6. **Auth flexibility**: rig's support for OAuth/session tokens via ChatGPT and Copilot providers is needed for subscription-based scenarios not covered by simple API keys.

### Mitigation: rig's Pre-1.0 Risk

- Pin to a specific version in `Cargo.toml`
- Wrap rig types behind the harness's own provider trait
- Monitor changelog for breaking changes
- rig's production users (St. Jude, ilert, Neon) demonstrate API stability in practice

### Fallback

If a breaking change in rig causes significant integration pain, the llm-sample crate provides a working alternative with the same test interface. The cost would be binary size, testability, and WASM support.

---

## Appendix A: API Surface for Harness Integration

### rig types to abstract

| rig type | Purpose | Wrap? |
|---|---|---|
| `CompletionModel` | Text generation | Yes — harness's `ModelProvider` trait |
| `Chat` trait | Multi-turn chat with history | Yes |
| `Message` | Chat message with role/content/tool_calls | Yes — harness owns session history type |
| `ToolDefinition` | Tool schema sent to model | Convert from harness tool definitions |
| `Usage` | Token counts | Map to harness's `TokenUsage` |
| `PromptError` | Error type | Convert to harness error enum |

### rig traits for custom providers

- `rig_core::completion::CompletionModel` — implement for text completion
- `rig_core::embedding::EmbeddingModel` — implement for embeddings
- `rig_core::provider::Provider` — implement for client-level integration

### llm traits (for reference)

- `llm::LLMProvider` — combines `ChatProvider` + `CompletionProvider` + `EmbeddingProvider` + `ModelsProvider`
- Also includes `SpeechToTextProvider` and `TextToSpeechProvider`
- `ChatProvider::chat_with_tools(&self, messages: &[ChatMessage], tools: Option<&[Tool]>)`
- `ChatMessageBuilder` API for constructing messages
