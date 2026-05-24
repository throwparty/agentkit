---
status: implemented
created: 2026-05-24
updated: 2026-05-24
author: adrian
decision: accepted
---

# `lens-kagi` Tasks

## Implementation Complete

### ✅ T1: Kagi Engine Module

**Spec**: K1, K3, K4, K6, K7
**Plan**: Full `kagi.rs` module — struct, trait impl, API client, result mapping, error handling
**Status**: ✅ Complete
**File**: `src/kagi/engine.rs`

**Completed**:
- `KagiSearchEngine` struct with `api_key: Arc<str>`
- `KagiOptions { api_key: String }`
- `KagiResponse`, `KagiMeta`, `KagiItem`, `KagiData` serde structs
- `SearchError` enum (InvalidKey, RateLimited, HttpError, Network, Parse)
- `SearchEngine` trait implementation: `name()`, `search()`, `is_configured()`
- **POST** to `https://kagi.com/api/v1/search` (not GET)
- Auth header: `Authorization: Bearer <api_key>`
- Request body: `{"query": "...", "n": 10, "workflow": "search"}`
- Query validation: reuses `SearchRequest::validate()` — empty check only, no size limits
- Result mapping: `url→link`, `title→title`, `snippet→snippet`, index→position
- Error mapping: 401→InvalidKey, 429→RateLimited, 500+→HttpError
- Defaults: `has_more=false`, `total_pages=1`
- 30s timeout on reqwest client
- Full debug output to stderr (URL, headers, body, status)

### ✅ T2: Wire Kagi into Main

**Spec**: K2, K5
**Plan**: `src/config.rs` (add field), `src/mcp.rs` (import + register), `src/kagi/mod.rs` (re-exports)
**Status**: ✅ Complete
**Files**: `src/config.rs`, `src/mcp.rs`, `src/lib.rs`, `src/kagi/mod.rs`

**Completed**:
- `src/config.rs`: `kagi_search_api_key: Option<String>` field with `--kagi-search-api-key` argument
- `src/mcp.rs`: `kagi` module imported; Kagi engine registered in `from_config()` immediately after Brave
- `src/kagi/mod.rs`: Re-exports `KagiSearchEngine`, `KagiOptions`, `SearchError`
- `src/lib.rs`: `pub mod kagi;` added

## Verification

| Check | Status |
|-------|--------|
| `cargo build` passes | ✅ |
| `cargo test` passes (66 tests) | ✅ |
| `src/kagi/` parallel to `src/brave/` | ✅ |
| `--kagi-search-api-key` accepted | ✅ |
| Kagi registered in engine registry | ✅ |
| `list-search-engines` will include Kagi | ✅ |

## Architecture Comparison: Brave vs Kagi

| Aspect | Brave | Kagi | Notes |
|--------|-------|------|-------|
| Module path | `src/brave/` | `src/kagi/` | Mirror structure |
| Types | `types.rs` | `types.rs` | Different schemas, parallel structure |
| Engine | `engine.rs` | `engine.rs` | Parallel implementation |
| Options | `BraveOptions` | `KagiOptions` | Both have `api_key` |
| Engine struct | `BraveSearchEngine` | `KagiSearchEngine` | Both hold `api_key: Arc<str>` |
| Client | `reqwest::Client` | `reqwest::Client` | Both 30s timeout |
| Trait impl | `SearchEngine` | `SearchEngine` | Same trait |
| name() | `brave` | `kagi` | Unique names |
| Error type | `SearchError` | `SearchError` | Same variant names |
| API endpoint | `GET /api/v1/web` | `POST /api/v1/search` | Different methods |
| Auth header | `X-Subscription-Token` | `Authorization: Bearer` | Different auth |
| Request body | Query params | JSON body | Different request style |
| Pagination | Brave: no pagination | Kagi: no pagination | Both `has_more=false`, `total_pages=1` |

## Files Created/Modified

| File | Action | Notes |
|------|--------|-------|
| `src/kagi/mod.rs` | Created | Module exports |
| `src/kagi/engine.rs` | Created | KagiSearchEngine implementation |
| `src/kagi/types.rs` | Created | Kagi API types and errors |
| `src/config.rs` | Modified | Added `kagi_search_api_key` field |
| `src/mcp.rs` | Modified | Added Kagi import + registration |
| `src/lib.rs` | Modified | Added `pub mod kagi;` |

## No Changes Required

- `src/brave/` — unchanged
- `src/search/mod.rs` — unchanged (trait reused)
- `src/safety.rs` — unchanged
- `src/cache.rs` — unchanged
- Data models (`SearchRequest`, `SearchResponse`) — unchanged
- Tool handlers (`search`, `fetch`, `list-search-engines`) — unchanged
