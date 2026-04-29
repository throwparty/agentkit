---
status: draft
created: 2026-04-29
updated: 2026-04-29
author: adrian
decision: pending
---

# `lens` MCP Server Tasks

> **Status**: draft | **Created**: 2026-04-29 | **Author**: adrian
>
> This file slices the [plan.md](plan.md) into independently implementable tasks. Each task includes acceptance criteria, dependencies, and test expectations. Tasks are grouped by implementation phase to enable parallel work where possible.
>
> **Total estimated effort**: ~13 days (9 tasks, in parallel where dependencies allow).

---

### Phase 1: Foundation

#### T1: Project Scaffolding

- **Spec**: AD1, AD2, NF3, NF5, NF6
- **Plan**: Module structure, Component Responsibilities (main.rs, lib.rs, Cargo.toml)
- **Dependencies**: None
- **Estimate**: 0.5 days
- **Owner**: Any engineer

**Description**: Create the `crates/agentkit-lens/` directory structure with all source files, add `agentkit-lens` as a workspace member in the root `Cargo.toml`, and define dependencies: `rmcp`, `reqwest`, `clap`, `html-to-markdown-rs` (3.3.3), `serde`, `serde_json`, `tokio`, `schemars`.

**Acceptance Criteria**:
- `cargo build` succeeds from workspace root
- All source files exist (listed in plan Module Structure)
- `clap` derives for `--brave-api-key` and `--cache-ttl` args exist in stubbed `config.rs`
- CI pipeline runs for new crate (lint, fmt, test)

**Tests**:
- `cargo clippy -- -D warnings` passes
- `cargo fmt --check` passes

**Rollout**: Part of PR adding the crate.

---

#### T2: SearchEngine Trait

- **Spec**: F12, F4
- **Plan**: `search/mod.rs` module
- **Dependencies**: T1
- **Estimate**: 0.5 days
- **Owner**: Any engineer

**Description**: Define the `SearchEngine` trait in `src/search/mod.rs` with two methods: `name(&self) -> &str` and `search(&self, req: SearchRequest) -> SearchResponse`. Implement module re-exports. Create placeholder `SearchRequest` and `SearchResponse` structs with `serde` derives and JSON schema via `schemars`.

**Acceptance Criteria**:
- `SearchEngine` trait defined with `name()` and `search()` methods
- `SearchRequest` struct includes: `query`, `engine`, `page`, `max_results`, `region` (all serde-serializable)
- `SearchResponse` struct includes: `results`, `query`, `engine`, `page`, `total_pages`, `has_more`
- Each result includes: `title`, `link`, `snippet`, `position`
- JSON schema generation compiles via `schemars`
- Unit tests for struct serialization round-trip

**Tests**:
- Unit: serde round-trip for `SearchRequest` and `SearchResponse`
- Unit: JSON schema generation succeeds

**Rollout**: No runtime impact; pure API definition.

---

### Phase 2: Core Modules (Parallelizable)

#### T3: Brave Search Implementation

- **Spec**: F1, F2, F3
- **Plan**: `brave/api.rs` (HTTP client), `brave/engine.rs` (trait impl), `brave/types.rs` (API schemas)
- **Dependencies**: T2
- **Estimate**: 2.5 days
- **Owner**: Engineer familiar with HTTP clients

**Description**: Split Brave implementation into three sub-modules. **`brave/api.rs`** is a centralized HTTP client (mirroring Brave's `BraveAPI/index.ts`): constructs API URLs, builds query params (`q`, `count`, `offset`, `country`, `search_lang`, etc.), sets auth headers (`X-Subscription-Token`), adds `Accept: application/json` and `Accept-Encoding: gzip`, and handles structured error parsing (JSON → text fallback). Sets `text_decorations=false` to avoid HTML markup in snippets. **`brave/types.rs`** defines Brave API response schemas (`WebSearchApiResponse`, `SearchError`) and error variants (`InvalidKey`, `RateLimited`, `HttpError`). **`brave/engine.rs`** implements `SearchEngine` trait: constructs `BraveOptions` from `--brave-api-key`, delegates HTTP calls to the API client, maps `web.results[]` to `SearchResponse`, and computes `has_more` from Brave's `more_results_available` field (not pagination arithmetic). Document the hard pagination cap: `offset` max is 9, so max 90 results per request (`count` × `offset`). Note: Brave uses `count` (1-20) + `offset` (0-9); our spec uses `page` + `max_results` — map: `offset = (page - 1) * count`, cap `offset` at 9.

**Acceptance Criteria**:
- `brave/api.rs` — centralized HTTP client with URL construction, query param building, auth headers, gzip support
- `brave/types.rs` — Brave API response schema (`WebSearchApiResponse`), error schema (`RateLimitErrorResponse`), typed error enum
- `brave/engine.rs` — `BraveSearchEngine` implements `SearchEngine` trait
- `BraveOptions { api_key: String }` struct defined
- `search()` delegates to API client and maps `web.results[]` → `SearchResponse`
- `has_more` derived from `response.query.more_results_available` (not computed from offset)
- `total_pages` computed from `(response.query.more_results_available, count)`
- Hard pagination cap documented: offset max 9, max 90 results per request
- Timeout set to 30s (per NF1)
- Structured errors parsed: 401 → `InvalidKey`, 429 → `RateLimited`, other → `HttpError(status, detail)`

**Tests**:
- Unit: Brave response parsing (mocked JSON → `SearchResponse`) — verify field mapping
- Unit: Brave API URL construction (mocked client) — verify query params
- Unit: `has_more` from `more_results_available` (last page false, intermediate true)
- Unit: `total_pages` boundary conditions (single page, multi-page)
- Unit: Error parsing — 401 → `InvalidKey`, 429 → `RateLimited`, 500 → `HttpError`
- Mock: Brave API returns 200 with valid JSON
- Mock: Brave API returns 401 (invalid key)
- Mock: Brave API returns 429 (rate limited) with structured error body
- Mock: `offset` capped at 9, returns 400 on higher offset

**Rollout**: Requires `--brave-api-key` to function; `list-search-engines` reports `configured: false` until key provided.

---

#### T4: CLI Config Parsing

- **Spec**: Problem Statement, AD7
- **Plan**: `config.rs` module
- **Dependencies**: T1
- **Estimate**: 0.5 days
- **Owner**: Any engineer

**Description**: Implement CLI argument parsing with `clap`. Define `Config` struct containing `brave_api_key: Option<String>` and `cache_ttl: Duration`. Parse `--cache-ttl` string (format: `1s`, `30m`, `4h`, `2d`, `1w`) into `Duration`. Validate at parse time; return error if invalid or zero/negative.

**Acceptance Criteria**:
- `Config` struct with `brave_api_key` and `cache_ttl` fields
- `--brave-api-key <KEY>` parses correctly (optional string)
- `--cache-ttl <DURATION>` parses: `1s`, `30m`, `4h`, `2d`, `1w`
- Invalid `--cache-ttl` returns parse error (not runtime)
- `cache_ttl` defaults to 5 minutes when omitted
- Duration string parsed in descending order of size

**Tests**:
- Unit: Valid duration strings parse to correct `Duration`
- Unit: Invalid duration strings fail with clear error
- Unit: Zero/negative durations rejected
- Unit: Default value is 5 minutes when omitted

**Rollout**: No runtime impact; CLI parsing only.

---

#### T5: Security Module

- **Spec**: F8, Edge Cases: Invalid URL format, DNS resolves to LAN IP
- **Plan**: `security.rs` module
- **Dependencies**: None (independent)
- **Estimate**: 1 day
- **Owner**: Engineer comfortable with networking

**Description**: Implement URL validation and LAN IP blocking. Parse URL scheme and host. Resolve host to IP addresses via `std::net::ToSocketAddrs`. Check if any resolved IP is in RFC 1918 ranges (10.x, 172.16-31.x, 192.168.x), localhost (127.x, ::1), or link-local (169.254.x). Reject with error if LAN IP detected. Validate URL format (scheme, host, path) before resolution.

**Acceptance Criteria**:
- `validate_url(url: &str) -> Result<()>` function defined
- Rejects URLs with invalid format (missing scheme, invalid host)
- Resolves hostname to IP addresses before checking
- Rejects RFC 1918 IPs (10.x.x.x, 172.16-31.x.x, 192.168.x.x)
- Rejects localhost (127.x.x.x, ::1)
- Rejects link-local (169.254.x.x)
- Accepts public IPs
- Error messages include resolved IP and reason for rejection

**Tests**:
- Unit: Valid public URL passes validation
- Unit: `192.168.1.1` rejected (private)
- Unit: `10.0.0.1` rejected (private)
- Unit: `172.16.0.1` rejected (private)
- Unit: `127.0.0.1` rejected (localhost)
- Unit: `169.254.1.1` rejected (link-local)
- Unit: `8.8.8.8` accepted (public)
- Unit: `example.com` resolves to public IP, accepted
- Mock: DNS resolution for private IP

**Rollout**: Security feature; no rollback needed but must not block valid fetches.

---

#### T6: Cache Module (Look-aside)

- **Spec**: F10, Edge Cases: Cached response expired, Cache TTL invalid
- **Plan**: `cache.rs` module
- **Dependencies**: T4
- **Estimate**: 1 day
- **Owner**: Engineer familiar with concurrency

**Description**: Implement in-memory TTL cache for HTTP responses. Key is normalized URL (lowercase scheme+host+path, remove fragments). Value is `FetchResponse` with content. On `get()`: check if key exists and not expired → return hit. If miss: fetch content, write to cache, return miss. Never update in place; stale entries expire naturally. Support configurable TTL via `Duration`.

**Acceptance Criteria**:
- `Cache` struct with configurable TTL
- `get(uri: &str) -> Option<FetchResponse>` returns cached response if present and not expired
- `put(uri: &str, response: FetchResponse)` stores response with expiry timestamp
- Cache key normalization: remove fragments, lowercase scheme+host+path
- Stale entries never returned (check expiry on `get()`)
- TTL parsed from `--cache-ttl` CLI arg (via T4)
- TTL validation at startup (zero/negative rejected)

**Tests**:
- Unit: Fresh cache hit returns cached response
- Unit: Expired cache entry returns `None`
- Unit: New entry written on miss
- Unit: Key normalization (fragment removal, case normalization)
- Unit: Multiple keys do not interfere
- Unit: TTL of 5 minutes works correctly
- Mock: Cache TTL validation fails for invalid string

**Rollout**: No runtime impact unless cache TTL causes unexpected staleness.

---

#### T7: Fetcher Module

- **Spec**: F5, F6, F7, F8, NF1, NF2
- **Plan**: `fetcher.rs` module
- **Dependencies**: T5
- **Estimate**: 2 days
- **Owner**: Engineer familiar with HTTP clients

**Description**: Implement HTTP fetch with GET-only requests, redirect following (up to 10 hops), and HTML-to-markdown conversion using `html-to-markdown-rs` (3.3.3). Use `reqwest` for HTTP client. Set 30s timeout. Truncate content to `max_length` (default 8000). Preserve headings. Strip script, style, nav, header, footer tags before conversion. Handle Unicode content. Return `FetchResponse` with content, status, content_type, and content_length.

**Acceptance Criteria**:
- `fetch(uri: &str, max_length: usize) -> Result<FetchResponse>` defined
- GET-only requests (no POST, PUT, PATCH, DELETE)
- Follows redirects up to 10 hops
- HTML-to-markdown conversion using `html-to-markdown-rs`
- 30s timeout enforced (per NF1)
- Content truncated to `max_length` (default 8000, per NF2)
- Headings preserved, scripts/styles stripped
- Unicode content handled correctly
- Empty response returns empty string, not error
- 403/404 returns error with status code

**Tests**:
- Unit: Valid HTML → markdown conversion (mocked HTML)
- Unit: Redirect following (mocked redirect chain)
- Unit: Content truncation at `max_length`
- Unit: Empty response returns empty string
- Mock: HTTP 200 with valid HTML
- Mock: HTTP 404 returns error
- Mock: HTTP 403 returns error
- Mock: 30s timeout returns error
- Mock: Redirect chain exceeds 10 hops

**Rollout**: No rollback needed; fetcher is a simple HTTP client with markdown conversion.

---

### Phase 3: Integration

#### T8: MCP Tool Handlers

- **Spec**: F1, F2, F3, F4, F5, F11, F12, Data Models (all)
- **Plan**: `mcp.rs` module, Data Flow, Traceability (F1-F4, F11, F12)
- **Dependencies**: T3, T5, T6, T7
- **Estimate**: 2 days
- **Owner**: Engineer familiar with MCP framework

**Description**: Implement three MCP tools using `rmcp` macros (`#[tool]`/`#[tool_router]`): `search`, `fetch`, and `list-search-engines`. Dispatch `search` to engine instance by name. Handle pagination with `has_more` field. Implement `fetch` with cache lookup, security validation, and markdown conversion. Implement `list-search-engines` to return list of registered engines with configured status. Wire all dependencies (search engines, cache, fetcher, security).

**Acceptance Criteria**:
- `search` tool: accepts `query`, `engine`, `page`, `max_results`, `region`; returns `SearchResponse`
- `fetch` tool: accepts `uri`, `max_length`, `start_index`, `format`; returns `FetchResponse`
- `list-search-engines` tool: accepts no args; returns `EnginesResponse` with engine names and configured status
- Cache lookup before fetch (per T6)
- Security validation before fetch (per T5)
- Markdown format only (per F11)
- Engine dispatch by name (per T3)
- JSON schema generation for all tool inputs/outputs
- Unit tests for tool handlers (mocked backends)

**Tests**:
- Unit: `search` tool dispatches to correct engine
- Unit: `fetch` tool checks cache before HTTP call
- Unit: `fetch` tool applies security validation
- Unit: `list-search-engines` returns correct engine list
- Integration: `search` with mocked Brave response
- Integration: `fetch` with mocked HTTP response
- Integration: `fetch` with cached response
- Integration: Edge cases (empty results, invalid URL, timeout)

**Rollout**: All three tools must work together; requires full integration testing.

---

#### T9: Main Entry Point

- **Spec**: AD1, AD2, AD7, Problem Statement
- **Plan**: `main.rs`, State Handling
- **Dependencies**: T3, T4, T6, T8
- **Estimate**: 1.5 days
- **Owner**: Any engineer

**Description**: Implement CLI entry point with stdio transport via `rmcp`. Mirror Brave's reference implementation: parse CLI args (`--brave-api-key`, `--cache-ttl`) via T4 config parser. Validate `cache_ttl` at startup (fail fast if invalid, exit with clear error message). Construct search engine instances with options structs (Brave) at startup. Initialize cache with validated TTL. Set up `rmcp` server using `#[tool_router]` macro on `LensTools` struct from T8. Use `McpServer::builder().with_stdio_transport()` pattern. Start MCP server with stdio transport.

**Acceptance Criteria**:
- CLI args parsed and validated at startup
- `--brave-api-key` passed to Brave engine constructor
- `--cache-ttl` validated; binary exits with error if invalid
- Search engine instances constructed and registered with MCP server
- Cache initialized with validated TTL
- `#[tool_router]` macro on `LensTools` struct (all three tools)
- `McpServer::builder().with_stdio_transport()` pattern
- stdio transport starts successfully
- `list-search-engines` returns correct configured status

**Tests**:
- Integration: Full startup with valid config
- Integration: Full startup with invalid cache TTL (exits with error)
- Integration: Full startup with no Brave API key (starts but search fails)

**Rollout**: Final integration step; requires all previous tasks.

**Acceptance Criteria**:
- CLI args parsed and validated at startup
- `--brave-api-key` passed to Brave engine constructor
- `--cache-ttl` validated; binary exits with error if invalid
- Search engine instances constructed and registered with MCP server
- Cache initialized with validated TTL
- stdio transport starts successfully
- `list-search-engines` returns correct configured status

**Tests**:
- Integration: Full startup with valid config
- Integration: Full startup with invalid cache TTL (exits with error)
- Integration: Full startup with no Brave API key (starts but search fails)

**Rollout**: Final integration step; requires all previous tasks.

---

### Task Dependency Graph

```
T1 (Scaffolding)
 ├── T2 (SearchEngine Trait) → T3 (Brave Search) ───┐
 ├── T4 (CLI Config) ─── T6 (Cache) ─────────────────┤
 └── T5 (Security) ─── T7 (Fetcher) ─────────────────┤
                                                     ├── T8 (MCP Tools) → T9 (Main Entry)
                                                     │
                                                     └──────────────────────┘
```

**Parallelism**: T3, T4, T5, T6, T7 can run in parallel once T1 and T2 are complete. T8 waits for T3, T5, T6, T7. T9 waits for T3, T4, T6, T8.
