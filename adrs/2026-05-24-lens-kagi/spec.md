---
status: draft
created: 2026-05-24
updated: 2026-05-24
author: adrian
decision: pending
---

# `lens-kagi` Feature Specification

## Relationship to Base Spec

This feature extends the existing `lens` MCP server by adding Kagi as a pluggable search engine. The existing spec defines the `SearchEngine` trait, the engine registry, and the `search` tool. Kagi is a first-class engine, not a fork or variant. The same `search` tool accepts `engine: "kagi"` to route to Kagi's backend.

## Problem Statement

Users want access to Kagi's premium search results for higher-quality, less-biased search output. Kagi is in closed beta, but the integration must be ready for release with zero changes to the MCP tool layer. Adding Kagi must not require modifications to tool handlers, data models, or safety logic.

## User Journey

### Agent Searches with Kagi

```
Agent → search(query="rust async runtime", engine="kagi")
  → lens dispatches to KagiSearchEngine
  → KagiSearchEngine calls https://kagi.com/api/v0/search?q=...
  → Kagi returns search results in their native format
  → lens maps Kagi results to SearchResponse (title, link, snippet, position)
  → lens returns SearchResponse to Agent
```

### Agent Lists Engines

```
Agent → list-search-engines
  → lens iterates engine registry
  → returns: [{"name": "brave", "configured": true}, {"name": "kagi", "configured": true}]
```

## Functional Requirements

| ID | Requirement | Acceptance Criteria |
|----|-------------|---------------------|
| K1 | `search` tool accepts `engine="kagi"` | Accepts `engine` parameter; routes to Kagi backend via `EngineRegistry::get("kagi")` without error |
| K2 | Kagi API key via CLI | Accepts `--kagi-search-api-key` argument; passed to `KagiSearchEngine` constructor |
| K3 | Kagi result mapping | Maps Kagi `data[]` items with `t=0` to `SearchResponse.results[]`; filters out `t=1` (related searches) |
| K4 | Pagination with Kagi | Uses `limit` parameter (from `SearchRequest.max_results`); no page-based pagination |
| K5 | Kagi engine registration | Engine registered in `EngineRegistry`; `list-search-engines` reports `configured: true` when API key provided |
| K6 | Error handling | Maps Kagi API errors (HTTP status, JSON) to engine-specific `SearchError` variants (parallel to Brave's `SearchError`) |
| K7 | Query validation | Reuses existing `SearchRequest::validate()` — empty check only; **no size limits** |

## Non-Functional Requirements

| ID | Requirement | Acceptance Criteria |
|----|-------------|---------------------|
| NF1 | Consistent with base spec | No changes to `SearchRequest`, `SearchResponse`, tool handlers, or safety modules. Module structure mirrors Brave (`src/kagi/` with `mod.rs`, `engine.rs`, `types.rs`) |
| NF2 | Module isolation | Kagi implementation in `src/kagi/`; no cross-contamination with `brave/` |
| NF3 | Timeout | 30s timeout (same as Brave, per base NF1) |
| NF4 | No caching changes | Reuses existing `cache.rs`; Kagi search results are not cached (search is not cached by default) |
| NF5 | Configurable at startup | `--kagi-search-api-key` is optional; engine is unconfigured until key provided; server starts with or without it |

## Edge Cases & Error Handling

| Scenario | Behavior |
|----------|----------|
| Kagi API returns `t=1` items | Filtered out; not included in results |
| Kagi API returns empty `data[]` | Return empty results array, no error |
| Kagi API returns 401 | Return error `InvalidKey` |
| Kagi API returns rate limit | Return error `RateLimited` with retry info |
| Kagi API key not provided | Engine unconfigured; `list-search-engines` reports `configured: false` |
| Query empty | `SearchRequest::validate()` returns error before API call |
| Kagi API returns no `snippet` field | Use empty string as default snippet |

## Architecture Decisions

| ID | Decision | Rationale |
|----|----------|-----------|
| AD1 | Kagi at `src/kagi/` (not `src/search/`) | Mirrors Brave structure (`src/brave/`); keeps engines parallel and isolated |
| AD2 | `KagiOptions` struct | `KagiOptions { api_key: String }`; consistent with `BraveOptions` pattern |
| AD3 | No pagination change | Kagi API uses `limit` parameter; page-based pagination is Brave-specific abstraction |
| AD4 | Filter `t=1` items | Kagi returns related searches in `data[]`; only `t=0` (search results) go to `SearchResponse` |
| AD5 | No new CLI routing needed | `engine` parameter already exists; user passes `engine="kagi"` |
| AD6 | Own `SearchError` type | Kagi defines its own `KagiSearchError` in `types.rs`, parallel to Brave's `SearchError` |
| AD7 | Reuse `SearchRequest::validate()` | Query validation (empty check only) already exists in `search/mod.rs`; Kagi engine calls `req.validate()` before API call; **no size limits** |

## Kagi API Details

### Endpoint

```
GET https://kagi.com/api/v0/search
```

### Authentication

```
Authorization: Bot <kagi_api_key>
```

### Query Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `q` | string | Yes | — | Search query |
| `limit` | int | No | 10 | Max number of results |

### Response Schema

```json
{
  "meta": {
    "id": "69c3f5c4168f66b860e951c585550f1c",
    "node": "us-central1",
    "ms": 213,
    "api_balance": 123.456
  },
  "data": [
    {
      "t": 0,
      "url": "https://example.com",
      "title": "Example",
      "snippet": "...",
      "published": "2024-09-30T00:00:00Z"
    },
    {
      "t": 1,
      "list": ["related", "searches"]
    }
  ]
}
```

### Type Field (`t`)

| t | Type | Description | Included in results |
|---|------|-------------|---------------------|
| 0 | Search Result | Main search result | Yes |
| 1 | Related Searches | Suggested follow-ups | No (filtered out) |

### Mapping: Kagi → `SearchResponse`

| Kagi Field | Our Field | Notes |
|------------|-----------|-------|
| `data[i].url` | `results[i].link` | Result URL |
| `data[i].title` | `results[i].title` | Result title |
| `data[i].snippet` | `results[i].snippet` | Result snippet (empty string if missing) |
| `data[i].published` | not included | Not part of SearchResponse schema (may be extended later) |
| Iteration index | `results[i].position` | 1-based position |

### Kagi-Specific Mapping Notes

- Kagi does not provide a `has_more` field. Default `has_more = false`.
- Kagi does not provide pagination metadata. If needed, implement a simple limit-based approach (`limit` parameter, no page offset).
- Kagi does not provide language or family_friendly metadata in the response schema.
- Thumbnail data (`data[i].thumbnail`) is not included in `SearchResponse`; may be extended in a future spec update.

## Implementation Scope

This feature adds Kagi as a search engine. The following files are modified:
- `src/config.rs` — add `kagi_search_api_key` field
- `src/kagi/` — NEW: Kagi integration module (engine.rs, types.rs, mod.rs)
- `src/mcp.rs` — import kagi module, register engine in `from_config()`

The following files are unchanged:
- `src/search/mod.rs` — `SearchRequest`, `SearchResponse`, `SearchEngine` trait, `EngineRegistry`
- `src/mcp.rs` — `search` tool handler, `list-search-engines`, tool router macro
- `src/brave/` — no changes
- `src/safety.rs` — no changes
- `src/cache.rs` — no changes
- Data models — `SearchRequest`, `SearchResponse`, `SearchResult`
