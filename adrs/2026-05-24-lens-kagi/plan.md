---
status: draft
created: 2026-05-24
updated: 2026-05-24
author: adrian
decision: pending
---

# `lens-kagi` Implementation Plan

## Scope

Add Kagi as a search engine in the existing `lens` MCP server. Only 3 files modified (`src/config.rs`, `src/mcp.rs`) and 1 new directory created (`src/kagi/`). No changes to existing Brave code, tool handlers, data models, or safety modules.

## Architecture Map

### Module Structure (Mirrors Brave)

```
crates/agentkit-lens/src/
├── brave/          # EXISTING — no changes
│   ├── mod.rs
│   ├── engine.rs
│   └── types.rs
├── kagi/          # NEW — parallel to brave/
│   ├── mod.rs      # Re-exports KagiSearchEngine, KagiOptions, types
│   ├── engine.rs   # KagiSearchEngine impl, API client, result mapping
│   └── types.rs    # Kagi API types, KagiSearchError
├── search/         # EXISTING — no changes
│   └── mod.rs      # SearchEngine trait, SearchRequest, SearchResponse, EngineRegistry
├── config.rs       # ADD kagi_search_api_key field
├── mcp.rs          # ADD kagi module import + engine registration in from_config()
├── main.rs         # No changes
├── safety.rs       # No changes
├── cache.rs        # No changes
└── lib.rs          # No changes
```

### No Changes Required

| Module | Reason |
|--------|--------|
| `src/search/mod.rs` | `SearchEngine` trait, `SearchRequest`, `SearchResponse`, `EngineRegistry` already defined and reused |
| `src/mcp.rs` tool router | `search` tool uses `registry.get(&args.engine)` — generic by name; `engine="kagi"` routes automatically |
| `src/brave/` | Kagi is independent; no shared code between Brave and Kagi |
| `src/fetch.rs` | Kagi only affects search, not fetch |
| `src/safety.rs` | LAN IP blocking is used by both search and fetch; no changes needed |
| `src/cache.rs` | No caching changes (search results are not cached by default) |

### Modified Files

| File | Change | Spec Ref |
|------|--------|----------|
| `src/config.rs` | Add `kagi_search_api_key: Option<String>` field with `--kagi-search-api-key` arg | K2 |
| `src/kagi/mod.rs` | NEW — re-export `KagiSearchEngine`, `KagiOptions`, types | AD1 |
| `src/kagi/engine.rs` | NEW — full KagiSearchEngine impl | K1, K3, K4, K6, K7 |
| `src/kagi/types.rs` | NEW — Kagi API response types, `KagiSearchError` | K6 |
| `src/mcp.rs` | Import `kagi` module; add registration block in `from_config()` | K5 |

## Data Flow

```
Agent → search(query="rust", engine="kagi")
  → mcp.rs tool router (dispatch by "kagi")
  → search/mod.rs (EngineRegistry::get("kagi") → KagiSearchEngine)
  → kagi/engine.rs (KagiSearchEngine::search())
  → KagiSearchEngine calls GET https://kagi.com/api/v0/search?q=...
  → Parses JSON, filters t=0 items
  → Maps to SearchResponse
  → Returns to mcp.rs → Agent
```

## Kagi Implementation Details

### Kagi Module (`src/kagi/`)

**Structs (types.rs)**

```rust
// Kagi API response structures
#[derive(Deserialize, Debug)]
pub struct KagiResponse {
    pub meta: KagiMeta,
    pub data: Vec<KagiItem>,
}

#[derive(Deserialize, Debug)]
pub struct KagiMeta {
    pub id: String,
    pub node: String,
    pub ms: u64,
    pub api_balance: f64,
}

#[derive(Deserialize, Debug)]
pub struct KagiItem {
    pub t: u8,
    pub url: String,
    pub title: String,
    pub snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

// Kagi engine-specific search error (parallel to brave::SearchError)
#[derive(Debug, thiserror::Error)]
pub enum KagiSearchError {
    #[error("Invalid or expired Kagi API key")]
    InvalidKey,

    #[error("Rate limit exceeded")]
    RateLimited,

    #[error("HTTP error: {status} - {detail}")]
    HttpError { status: u16, detail: String },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Parse error: {0}")]
    Parse(String),
}
```

**Engine (engine.rs)**

```rust
use super::types::*;
use crate::search::{SearchEngine, SearchRequest, SearchResponse, SearchResult};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct KagiOptions {
    pub api_key: String,
}

pub struct KagiSearchEngine {
    api_key: Arc<str>,
    client: reqwest::Client,
}

impl KagiSearchEngine {
    pub fn new(options: KagiOptions) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .gzip(true)
            .build()
            .expect("Failed to build HTTP client");

        Self {
            api_key: Arc::from(options.api_key),
            client,
        }
    }

    fn build_url(&self, req: &SearchRequest) -> String {
        let count = req.max_results.min(20);
        let mut url = format!(
            "https://kagi.com/api/v0/search?q={}&limit={}",
            urlencoding::encode(&req.query),
            count
        );
        url
    }

    fn parse_response(&self, api_response: KagiResponse, req: &SearchRequest) -> SearchResponse {
        let results: Vec<SearchResult> = api_response
            .data
            .into_iter()
            .filter(|item| item.t == 0)  // Filter out related searches (t=1)
            .enumerate()
            .map(|(i, item)| SearchResult {
                title: item.title,
                link: item.url,
                snippet: item.snippet.unwrap_or_default(),
                position: (i + 1) as u32,
            })
            .collect();

        SearchResponse {
            results,
            query: req.query.clone(),
            engine: self.name().to_string(),
            page: req.page,
            total_pages: 1,  // No pagination from Kagi
            has_more: false,  // No pagination from Kagi
        }
    }
}

#[async_trait::async_trait]
impl SearchEngine for KagiSearchEngine {
    fn name(&self) -> &str {
        "kagi"
    }

    fn is_configured(&self) -> bool {
        !self.api_key.is_empty()
    }

    async fn search(&self, req: SearchRequest) -> crate::search::SearchEngineResult {
        if let Err(e) = req.validate() {
            return Err(e.into());
        }

        let url = self.build_url(&req);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bot {}", self.api_key.as_ref()))
            .header("Accept", "application/json")
            .header("Accept-Encoding", "gzip")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    KagiSearchError::HttpError {
                        status: 0,
                        detail: "Request timed out after 30s".to_string(),
                    }
                } else {
                    KagiSearchError::Network(e)
                }
            })?;

        let status = response.status().as_u16();

        if status == 401 {
            return Err(KagiSearchError::InvalidKey.into());
        }
        if status == 429 {
            return Err(KagiSearchError::RateLimited.into());
        }

        let body = response.text().await?;

        if status >= 400 {
            return Err(KagiSearchError::HttpError {
                status,
                detail: body.clone(),
            }.into());
        }

        let api_response: KagiResponse =
            serde_json::from_str(&body).map_err(|e| KagiSearchError::Parse(e.to_string()))?;

        Ok(self.parse_response(api_response, &req))
    }
}
```

### Wiring (mcp.rs)

**Current Brave registration (keep as-is):**
```rust
if let Some(ref api_key) = cfg.brave_search_api_key {
    registry.register(Box::new(brave::BraveSearchEngine::new(brave::BraveOptions {
        api_key: api_key.clone(),
    })));
}
```

**Add Kagi registration (immediately after Brave):**
```rust
if let Some(ref api_key) = cfg.kagi_search_api_key {
    registry.register(Box::new(kagi::KagiSearchEngine::new(kagi::KagiOptions {
        api_key: api_key.clone(),
    })));
}
```

**Add import:**
```rust
use crate::kagi;
```

### Config (config.rs)

**Add field:**
```rust
#[arg(long = "kagi-search-api-key")]
pub kagi_search_api_key: Option<String>,
```

## Error Handling

### Error Mapping

| HTTP Status | Error Variant | Message |
|-------------|---------------|---------|
| 401 | `KagiSearchError::InvalidKey` | "Invalid or expired Kagi API key" |
| 429 | `KagiSearchError::RateLimited` | "Rate limit exceeded" |
| 400 | `KagiSearchError::HttpError(400, detail)` | "HTTP error: 400 - ..." |
| 500+ | `KagiSearchError::HttpError(status, detail)` | "HTTP error: 500 - ..." |
| parse fail | `KagiSearchError::Parse(msg)` | "Parse error: ..." |
| timeout | `KagiSearchError::HttpError(0, "timed out")` | "Request timed out after 30s" |

### Query Validation

Reuses existing `SearchRequest::validate()` — **empty check only**. No size limits (400 chars, 50 words) per spec update.

### Edge Case Handling

| Scenario | Behavior |
|----------|----------|
| `data[]` contains `t=1` items | Filtered out with `.filter(|item| item.t == 0)` |
| `data[]` is empty | Return empty results array (not error) |
| `snippet` is null | Use `Option<String>` with `unwrap_or_default()` → empty string |
| No `limit` sent | Use `max_results.min(20)` from request (default 10, max 20) |
| Kagi API unreachable | Timeout after 30s, return error |

## State Handling

- **No new state**: Kagi state is identical to Brave — one `api_key` string held in the engine struct.
- **No session tracking**: Each search is independent; no server-side state beyond engine registry.
- **CLI at startup**: `--kagi-search-api-key` read from CLI args, validated, used to construct engine.
- **Fail fast**: If `--kagi-search-api-key` provided but empty, engine is constructed but `is_configured()` returns false.

## Testing Strategy

### Module Tests (in engine.rs)

| Test | Coverage |
|------|----------|
| `test_kagi_engine_name` | `name()` returns "kagi" |
| `test_kagi_engine_configured` | `is_configured()` returns true with key, false without |
| `test_build_url_basic` | URL contains query, limit param |
| `test_build_url_with_limit` | URL respects max_results |
| `test_parse_response_basic` | Mapping: url→link, title→title, snippet→snippet, index→position |
| `test_parse_response_t1_filtered` | t=1 items excluded from results |
| `test_parse_response_empty` | Empty data returns empty results |
| `test_parse_response_missing_snippet` | Missing snippet → empty string |

### Integration Tests

| Test | Coverage |
|------|----------|
| `search(engine="kagi")` with valid response | Full Kagi search flow (mocked HTTP) |
| `search(engine="kagi")` with empty response | Empty results returned |
| `search(engine="kagi")` with Kagi error | Error mapped correctly |
| `list-search-engines` with Kagi key | Kagi shows configured=true |
| `list-search-engines` without Kagi key | Kagi shows configured=false |

### Test Data

Embed Kagi API response as static string in test module:

```rust
const VALID_KAGI_RESPONSE: &str = r#"{
  "meta": {"id":"test","node":"test","ms":100,"api_balance":0},
  "data": [
    {"t":0,"url":"https://example.com","title":"Test","snippet":"A test result"},
    {"t":1,"list":["related","searches"]}
  ]
}"#;

const EMPTY_KAGI_RESPONSE: &str = r#"{
  "meta": {"id":"test","node":"test","ms":100,"api_balance":0},
  "data": []
}"#;
```

### CI Validation

- `cargo clippy -- -D warnings` passes
- `cargo fmt --check` passes
- `cargo build --all-targets` succeeds
- All new unit tests pass in CI (mocked HTTP, no network)

## Traceability: Spec → Plan

| Spec Req | Plan Section | Implementation |
|----------|-------------|----------------|
| K1: `engine="kagi"` | Engine registration in mcp.rs | Routes to `KagiSearchEngine` by name |
| K2: `--kagi-search-api-key` | config.rs field | Add field with clap arg |
| K3: Kagi result mapping | `parse_response()` in engine.rs | `t=0` filter, index → position |
| K4: Pagination | `build_url()` limit param | `limit` from max_results, no page offset |
| K5: Engine registration | mcp.rs wiring | Register `KagiSearchEngine` in `from_config()` |
| K6: Error handling | engine.rs error mapping | 401→InvalidKey, 429→RateLimited, etc. |
| K7: Query validation | Reuses `SearchRequest::validate()` | Empty check only; **no size limits** |
| NF1: Consistent | Module structure | Mirrors `src/brave/` at `src/kagi/` |
| NF2: Isolation | Module map | `kagi/` parallel to `brave/` |
| NF3: Timeout | `engine.rs` client | 30s reqwest timeout |
| NF4: No cache change | Scope | No cache modifications |
| NF5: Configurable | config.rs, mcp.rs | Optional CLI arg, unconfigured until key provided |

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Kagi API changes | Search breaks | Module tests in CI; version pinning on endpoint |
| Kagi not released yet | Engine unavailable | Engine unconfigured until key provided; `list-search-engines` shows false |
| Kagi API key invalid | Search fails | Fail on first use; clear error message |
| Kagi rate limiting | 429 response | Kagi handles rate limiting; mapped to `RateLimited` |
| t=1 items leak into results | Extra data in output | Explicit `t=0` filter; test with mixed response |
| Missing snippet field | Parse error | Use `Option<String>` in serde; default to empty string |

## Open Questions

| Question | Context | Resolution |
|----------|---------|------------|
| What is Kagi's documented query length limit? | Kagi docs say "query" but no explicit max | **No size limits** — reuses `SearchRequest::validate()` (empty check only) |
| Should Kagi support pagination? | Kagi API has no pagination metadata; for v1, return `has_more=false` | Documented as open; can add `page` parameter when Kagi releases it |
| Should we expose Kagi's `api_balance`? | Not in `SearchResponse` schema; may add in future spec update | Deferred — no changes to output schema |
