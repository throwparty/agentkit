---
sidebar_position: 100
---

# 💭 Roadmap

## Additional model providers

- **OpenRouter** — third-party aggregator with broad model selection and per-model pricing. Already covered by the existing `bearer_token` auth type — just needs a `[[providers]]` entry in the TOML config.
- **OpenAI Codex Go/Zen** — new OpenAI subscription tiers with their own API surfaces and auth flows.
- **Anthropic (Claude subscription OAuth + Platform API keys)** — full first-class support for both authentication paths, including Claude Code's internal usage endpoint for quota-aware routing.
- **GitHub Copilot** — subscription-based model access via GitHub's OAuth flow and API surface.
- **AWS Bedrock** — enterprise deployment path. Notably uses different caching infrastructure than the Anthropic API directly (cache isolation per-organization vs per-workspace).
- **Google Vertex AI** — another enterprise path with its own cache isolation model.
- Others — Google AI Studio, Groq, Together, Replicate, Fireworks AI.

## Quota status API

A dedicated page or API endpoint (`GET /quotas`) that surfaces per-provider quota state in real time:

- Remaining rate-limit headroom (requests/tokens per minute).
- Subscription quota utilisation (messages per 5-hour window, weekly caps).
- Degradation state and auto-recovery ETA.
- Total input/output token usage per session and provider.
- Credential source and expiry status.

This would make it easy to understand routing decisions and diagnose why a particular provider is being skipped.

## Cache-aware session costing

When a session switches providers mid-conversation, the new provider must process the full conversation history from scratch — incurring a "cache penalty" that can be 30–50% of the session's token budget.

Future work includes:

- **Estimating cache penalties on provider switch**: before switching, estimate the cost of re-sending the full context to the new provider vs. staying with the degraded provider after it recovers.
- **Provider cache behaviour awareness**: each provider has different caching characteristics that affect switching costs:
  - **Anthropic**: prompt caching is **per-model** — Sonnet and Haiku do not share caches. Switching models always incurs a full cache miss. Caches use prefix matching with 4 breakpoints max, 20-block lookback window, and are isolated per-organization/workspace.
  - **OpenAI**: automatic caching for prompts ≥1024 tokens. No explicit breakpoints needed, but cache is also per-model.
  - **Codex subscription**: uses Responses API with its own (undocumented) caching behaviour — switching between Codex subscription and API key always incurs the penalty.
- **Cached input token pricing**: incorporate per-provider cached-input discounts into provider ranking (e.g. Anthropic cache reads at 0.1x base input price, OpenAI cache reads at 0.5x) to prefer providers/sessions with warm caches.
- **Provider affinity hints**: allow clients to signal preferred providers to avoid unnecessary switching when multiple providers could serve the request.

## Future considerations

- Configuration hot-reload (avoid restarting to change providers).
- Token refresh for additional OAuth providers beyond OpenAI Codex.
- `GET /anthropic/v1/messages` and other API surface support.
- Performance benchmarks and latency budgets for the proxy path.
