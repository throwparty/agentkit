---

## status: draft created: 2026-04-29 updated: 2026-04-30 author: adrian decision: pending

# `lens` MCP Server Plan

## Architecture

### Module Structure

```
crates/agentkit-lens/
├── Cargo.toml
├── src/
│   ├── main.rs          # CLI entry, stdio transport
│   ├── lib.rs           # Public API
│   ├── mcp.rs           # MCP tools (search, fetch, list-search-engines)
│   ├── search/
│   │   ├── mod.rs       # SearchEngine trait, engine registry
│   │   └── brave.rs     # Brave Search API integration
│   ├── fetcher.rs       # HTTP fetch + HTML-to-markdown parsing
│   ├── cache.rs         # Look-aside cache (TTL-based, in-memory)
│   ├── security.rs      # LAN IP blocking, URL validation
│   └── config.rs        # CLI args parsing, validation
├── tests/
│   ├── mock_search.rs   # Mock Brave responses
│   ├── mock_fetch.rs    # Mock HTTP responses
│   └── integration.rs   # End-to-end mock tests
└── README.md
```

### Component Responsibilities

| Module            | Responsibility                                                  | Dependencies                                             |
| ----------------- | --------------------------------------------------------------- | -------------------------------------------------------- |
| `main.rs`         | CLI entry, stdio transport, engine registration                 | `rmcp`, `config.rs`, `search/mod.rs`                     |
| `mcp.rs`          | Tool handlers (search, fetch, list-search-engines), JSON schema | `search/mod.rs`, `fetcher.rs`, `cache.rs`, `security.rs` |
| `search/mod.rs`   | SearchEngine trait, engine registry                             | `serde`, `schemars`                                      |
| `search/brave.rs` | Brave Search API calls, result parsing                          | `reqwest`, `serde`                                       |
| `fetcher.rs`      | HTTP GET, redirect following, HTML-to-markdown                  | `reqwest`, `html-to-markdown-rs`                         |
| `cache.rs`        | Look-aside TTL cache, key normalization                         | `std::collections::HashMap`                              |
| `security.rs`     | URL validation, DNS resolution, LAN IP blocking                 | `std::net::ToSocketAddrs`                                |
| `config.rs`       | CLI arg parsing, duration validation                            | `clap`, `std::time::Duration`                            |

### Data Flow

```
Agent → MCP tool call → mcp.rs (handler)
  ├─ search → engine registry (dispatch by name) → BraveSearchEngine::search() → JSON response
  └─ fetch → security.rs (URL validation) → cache.rs (look-aside check) → fetcher.rs (HTTP call) → HTML parse → JSON response
```

### SearchEngine Trait

```rust
#[async_trait]
pub trait SearchEngine: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, req: SearchRequest) -> Result<SearchResponse>;
}
```

### Engine Registration

Engines are registered at startup in `main.rs` via an engine registry. The registry maps engine names (e.g., "brave") to their configured instances. `list-search-engines` iterates the registry and returns each engine's name and configured status.

### State Handling

- **In-memory state**: Cache and engine registry are process-scoped, not persisted.
- **No session state**: Each tool call is independent. No server-side session tracking.
- **CLI args at startup**: `--brave-api-key` and `--cache-ttl` read from CLI args, validated, and used to construct engine instances and cache. Fail fast if invalid.

## Brave Search API Details

### Endpoint

```
GET https://api.search.brave.com/res/v1/web/search
```

### Authentication

```
X-Subscription-Token: <brave_api_key>
Accept: application/json
Accept-Encoding: gzip
```

### Query Parameters

| Parameter    | Type          | Required | Default    | Description                                    |
| ------------ | ------------- | -------- | ---------- | ---------------------------------------------- |
| `q`          | string        | Yes      | —          | Search query (400 chars max, 50 words max)     |
| `count`      | int           | No       | 20         | Number of results (1-20)                       |
| `offset`     | int           | No       | 0          | Zero-based offset for pagination (0-9)         |
| `country`    | enum\<string> | No       | "US"       | 2-char country code (ISO 3166-1)               |
| `safesearch` | enum\<string> | No       | "moderate" | "off", "moderate", "strict"                    |
| `spellcheck` | boolean       | No       | true       | Enable spell checking                          |
| `freshness`  | enum\<string> | No       | ""         | "pd" (24h), "pw" (7d), "pm" (31d), "py" (365d) |

### Response Schema (WebSearchApiResponse)

```json
{
  "type": "search",
  "query": {
    "original": "rust async runtime",
    "more_results_available": true,
    "altered": "rust async runtime",
    "country": "US",
    "safesearch": false,
    "bad_results": false
  },
  "web": {
    "type": "search",
    "results": [
      {
        "title": "Async Rust...",
        "url": "https://example.com",
        "description": "...",
        "page_age": "2 days ago",
        "page_fetched": "2026-04-28T10:00:00Z",
        "language": "en",
        "family_friendly": true,
        "type": "search_result",
        "subtype": "generic"
      }
    ]
  },
  "faq": { "items": [...] },
  "discussions": { "results": [...] },
  "summarizer": { "key": "..." }
}
```

### Mapping: Brave → `SearchResponse`

| Brave Field                      | Our Field            | Notes                       |
| -------------------------------- | -------------------- | --------------------------- |
| `query.original`                 | `query`              | Original search query       |
| `query.more_results_available`   | `has_more`           | Boolean flag for pagination |
| `web.results[i].title`           | `results[i].title`   | Result title                |
| `web.results[i].url`             | `results[i].link`    | Result URL                  |
| `web.results[i].description`     | `results[i].snippet` | Result description/snippet  |
| `web.results[i].page_age`        | optional metadata    | Human-readable age          |
| `web.results[i].language`        | optional metadata    | ISO 639 language code       |
| `web.results[i].family_friendly` | optional metadata    | Boolean flag                |

### Pagination Strategy

Brave uses `offset` + `count` (not page numbers). Our mapping:

```
page 1 → offset=0, count=max_results
page 2 → offset=max_results, count=max_results
page 3 → offset=max_results*2, count=max_results
```

- `offset = (page - 1) * count`
- `has_more = query.more_results_available`
- Maximum offset is 9 (Brave limit)
- When `page * count > 200`, return `has_more = false` (practical ceiling)

### Brave Implementation Details

- **Engine struct**: `BraveSearchEngine { api_key: String }`
- **API key**: From `--brave-api-key` CLI arg, stored in `BraveOptions { api_key: String }`
- **Serde deserialization**: From `WebSearchApiResponse` → `SearchResponse`
- **Timeout**: 30s (per NF1)
- **Error codes**:
  - 401 → invalid/expired API key
  - 429 → rate limit exceeded
  - 400 → bad query (too long, malformed)
  - 500 → server error
- **Query validation**: Enforce 400 char / 50 word limit before sending
- **Safe search**: Default "moderate", configurable per call (not in scope for v1)

## Kagi Search API Details (Deferred)

### Endpoint

```
GET https://kagi.com/api/v0/search
```

### Authentication

```
Authorization: Bot <kagi_api_key>
```

### Query Parameters

| Parameter | Type   | Required | Default | Description           |
| --------- | ------ | -------- | ------- | --------------------- |
| `q`       | string | Yes      | —       | Search query          |
| `limit`   | int    | No       | —       | Max number of results |

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
      "url": "https://en.wikipedia.org/wiki/Example",
      "title": "Example - Wikipedia",
      "snippet": "An example...",
      "published": "2024-09-30T00:00:00Z",
      "thumbnail": {
        "url": "/proxy/...",
        "width": 310,
        "height": 300
      }
    },
    {
      "t": 1,
      "list": ["related", "searches"]
    }
  ]
}
```

### Type Field (`t`)

| t   | Type             | Description          | Included in results |
| --- | ---------------- | -------------------- | ------------------- |
| 0   | Search Result    | Main search result   | Yes                 |
| 1   | Related Searches | Suggested follow-ups | No (filtered out)   |

### Mapping: Kagi → `SearchResponse`

| Kagi Field              | Our Field            | Notes              |
| ----------------------- | -------------------- | ------------------ |
| `data[i].url`           | `results[i].link`    | Result URL         |
| `data[i].title`         | `results[i].title`   | Result title       |
| `data[i].snippet`       | `results[i].snippet` | Result snippet     |
| `data[i].published`     | optional metadata    | ISO 8601 timestamp |
| `data[i].thumbnail.url` | optional metadata    | Proxied image URL  |

### Kagi Implementation Notes

- **Currently**: Closed beta, requires `support@kagi.com` invite
- **Pricing**: $25 for 1000 queries (2.5 cents per search)
- **API key**: From user account at `kagi.com/settings/api`
- **Engine struct**: `KagiSearchEngine { api_key: String }`
- **Serde deserialization**: From `KagiResponse` → `SearchResponse`
- **Timeout**: 30s
- **Pagination**: Not documented in closed beta; implement when spec is released
- **Authorization header**: `Authorization: Bot <token>`

### Engine Registry

```rust
// search/mod.rs
pub struct EngineRegistry {
    engines: Vec<Box<dyn SearchEngine>>,
}

impl EngineRegistry {
    pub fn new() -> Self { ... }
    pub fn register(&mut self, engine: Box<dyn SearchEngine>) { ... }
    pub fn get(&self, name: &str) -> Option<&dyn SearchEngine> { ... }
    pub fn list(&self) -> Vec<EngineInfo> { ... } // for list-search-engines
}

pub struct EngineInfo {
    pub name: String,
    pub configured: bool,
    pub hint: Option<String>,
}
```

At startup in `main.rs`:

```rust
let mut registry = EngineRegistry::new();
if let Some(api_key) = config.brave_api_key {
    registry.register(Box::new(BraveSearchEngine::new(BraveOptions { api_key })));
}
// Kagi: register when released
```

`list-search-engines` returns:

```json
{
  "engines": [
    { "name": "brave", "configured": true, "hint": null },
    { "name": "kagi", "configured": false, "hint": "Kagi API not yet released" }
  ]
}
```

## Traceability: Spec → Plan

| Spec Req                 | Plan Section                    | Implementation                                   |
| ------------------------ | ------------------------------- | ------------------------------------------------ |
| F1: Brave search         | `search/brave.rs`               | Structured API calls, Brave API mapping          |
| F2: Search format        | `search/brave.rs` → `mcp.rs`    | Brave `web.results[]` → unified `SearchResponse` |
| F3: Pagination           | `search/brave.rs`               | `offset` + `count` → `page` + `has_more` mapping |
| F4: list-search-engines  | `mcp.rs` → `search/mod.rs`      | Registry iteration, configured status check      |
| F5: GET-only fetch       | `fetcher.rs`                    | No POST/PUT/PATCH/DELETE                         |
| F6: HTML-to-markdown     | `fetcher.rs`                    | html-to-markdown-rs, tag stripping               |
| F7: Redirects            | `fetcher.rs`                    | reqwest redirect config (10 hops)                |
| F8: LAN blocking         | `security.rs`                   | DNS resolve → RFC 1918 check                     |
| F10: Look-aside cache    | `cache.rs`                      | Check on `get()`, write on miss, natural expiry  |
| F11: Markdown format     | `fetcher.rs`                    | html-to-markdown-rs (3.3.3)                      |
| F12: Search engine trait | `search/mod.rs`                 | `name()`, `search()` methods, engine registry    |
| NF1: Timeout             | `fetcher.rs`, `search/brave.rs` | 30s reqwest timeout                              |
| NF2: Max size            | `fetcher.rs`                    | Truncate to `max_length`                         |
| NF3: Transport           | `main.rs`                       | rmcp stdio                                       |
| NF4: Dependencies        | `Cargo.toml`                    | reqwest, html-to-markdown-rs (3.3.3), rmcp       |
| NF5: No auth             | `main.rs`                       | No auth middleware                               |
| NF6: No env diff         | `main.rs`                       | Single binary, no env checks                     |

## Risks & Mitigations

| Risk                   | Impact                  | Mitigation                                                                    |
| ---------------------- | ----------------------- | ----------------------------------------------------------------------------- |
| Brave API changes      | Search breaks           | Mock tests in CI, version pinning                                             |
| HTML parsing fragility | Markdown output garbled | html-to-markdown-rs stable; fallback to raw text on parse error               |
| LAN IP evasion         | Security bypass         | Validate after DNS resolve, not before                                        |
| Cache stale data       | Fetch returns outdated  | TTL-based look-aside; configurable                                            |
| Rate limit exceeded    | 429 from Brave          | Brave handles rate limiting; log and retry with backoff                       |
| Kagi not released      | Kagi unavailable        | Engine not registered until release; `list-search-engines` shows unconfigured |
| Brave API key expired  | Search fails            | Fail on first use, clear error message                                        |

## Testing Strategy

### Mock Strategy

Since Brave Search and external websites are unreliable in CI, all integration tests use **mock HTTP responses**:

- `tests/mock_search.rs`: Mock Brave API responses (valid, invalid, rate limited)
- `tests/mock_fetch.rs`: Mock HTTP responses (200, 404, 403)
- `tests/integration.rs`: End-to-end tool calls with mocked backends

### Test Coverage

| Test Type                        | Coverage                               | Location                           |
| -------------------------------- | -------------------------------------- | ---------------------------------- |
| Unit: Brave response parsing     | Brave API response → `SearchResponse`  | `search/brave.rs`                  |
| Unit: Brave API URL construction | Query params, auth header              | `search/brave.rs`                  |
| Unit: Pagination mapping         | `page` → `offset`, `has_more`          | `search/brave.rs`                  |
| Unit: list-search-engines        | Engine registry iteration              | `mcp.rs`                           |
| Unit: HTML-to-markdown           | Tag stripping, semantic preservation   | `fetcher.rs` (html-to-markdown-rs) |
| Unit: LAN IP blocking            | RFC 1918, localhost, link-local ranges | `security.rs`                      |
| Unit: Cache look-aside           | Check hit, write miss, natural expiry  | `cache.rs`                         |
| Unit: CLI duration parsing       | `1s`, `30m`, `4h`, `2d`, `1w`          | `config.rs`                        |
| Integration: search tool         | Full search flow (mocked Brave)        | `tests/mock_search.rs`             |
| Integration: fetch tool          | Full fetch flow (mocked HTTP)          | `tests/mock_fetch.rs`              |
| Integration: Edge cases          | Empty results, timeout, invalid URL    | `tests/integration.rs`             |

### Test Harness

All tests use a mocked HTTP client layer. The Brave API responses and target website responses are embedded as static JSON/HTML strings in the test files, not fetched from the network.

```rust
// Example: mock Brave response in tests/mock_search.rs
#[tokio::test]
async fn test_brave_search_valid() {
    let mock_response = r#"{"type":"search","query":{"more_results_available":false,"original":"test"},"web":{"results":[{"title":"T","url":"https://example.com","description":"D"}]}}"#;
    let client = MockClient::new(200, mock_response);
    let engine = BraveSearchEngine::new(BraveOptions { api_key: "test".into() });
    let result = engine.search(SearchRequest::new("test", 1, 10, None)).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().results.len(), 1);
}
```

### CI Validation

- Run all unit tests on every PR
- Integration tests use mocked HTTP responses (no network calls)
- Lint: `cargo clippy -- -D warnings`
- Format: `cargo fmt --check`
- Build: `cargo build --all-targets`

## Deployment

- **Single binary**: `agentkit-lens`
- **No network listening**: stdio only
- **CLI args**: `--brave-api-key <KEY>`, `--cache-ttl <DURATION>`
- **Duration format**: `1s`, `30m`, `4h`, `2d`, `1w` (parsed in descending order of size)
- **Cache TTL validation**: Fail fast at startup if invalid (zero, negative, or unparseable)
- **No config files**: CLI args only
- **No rollback needed**: Stateless; no persisted state

## Reference Documents

These documents are pulled from upstream sources and stored in `adrs/2026-04-29-lens/refs/` for future implementation reference:

| File                  | Source                 | Content                                                |
| --------------------- | ---------------------- | ------------------------------------------------------ |
| `refs/brave-api.html` | Brave Search API docs  | Full API reference (endpoint, params, response schema) |
| `refs/kagi-api.md`    | Kagi API docs (GitHub) | Search API spec (currently closed beta)                |
