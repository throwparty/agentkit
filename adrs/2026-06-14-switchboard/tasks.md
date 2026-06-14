---
status: accepted
created: 2026-06-14
updated: 2026-06-14
author: adrian
decision: accepted
---

# Tasks: Switchboard — Cost-Aware Model Provider Proxy

Each task is an independently testable unit. Tasks can be implemented in any order except where noted. No task depends on the HTTP server being complete — each has its own test harness.

---

## T1: Config Types and TOML Loading

**Spec ref:** §6 TOML Configuration Schema, §6.6 Config File Loading  
**Plan ref:** §4 Phase 1  
**Testable without:** HTTP server, database, credential helpers, routing

### Deliverables

- `crates/agentkit-switchboard/src/config/mod.rs` — all config structs
- `crates/agentkit-switchboard/src/config/loader.rs` — TOML deserialize, Vec→HashMap conversion, validation
- `tests/fixtures/minimal.toml`, `tests/fixtures/duplicate-identity.toml`

### Config types

`SwitchboardConfig`, `ModelConfig`, `Capabilities`, `ProviderConfig`, `ApiSurface`, `BillingModel`, `AuthConfig`, `AuthType`, `OAuthEndpointConfig`, `PricingConfig`, `PerModelPricing`

### Acceptance criteria

- [ ] Minimal TOML produces valid `SwitchboardConfig` with HashMap providers
- [ ] Duplicate identities fail with error message containing the duplicate value
- [ ] Unknown enum variant fails with clear serde error
- [ ] `credential_helper` defaults to `None` when absent
- [ ] `[auth.oauth]` section parses into `OAuthEndpointConfig`
- [ ] `models: Option<Vec<String>>` — `None` when absent, `Some(...)` when present

### Tests

| Test | What it validates |
|------|-------------------|
| `config_parse_valid` | Minimal TOML produces valid config |
| `config_parse_duplicate_identity` | Duplicate identity rejected |
| `config_parse_bad_enum` | Unknown enum variant fails |
| `config_oauth_endpoints` | `[auth.oauth]` parses correctly |
| `config_models_override` | `[models.*]` overrides parsed |
| `config_credential_helper_default` | Default is `None` |

---

## T2: Model Metadata Crate and Merge Logic

**Spec ref:** §5 Model Metadata Layer, §9.2 GET /openai/v1/models  
**Plan ref:** §4 Phase 2  
**Testable without:** HTTP server, database, routing, credentials

### Deliverables

- `crates/agentkit-models/Cargo.toml` + `build.rs` + `src/lib.rs`
- `crates/agentkit-switchboard/src/models/mod.rs` — `ModelMetadata`, `MergedModel`
- `crates/agentkit-switchboard/src/models/db.rs` — snapshot loader, TOML override merger

### Acceptance criteria

- [ ] `agentkit-models` build.rs fetches models.dev data and writes `data/models.dev.json`
- [ ] Bundled snapshot loads at startup (via `include_bytes!` or similar)
- [ ] TOML `[models.*]` overrides take precedence over bundled values (field-level)
- [ ] Model lookup by name returns merged result or `None`
- [ ] Provider pricing overlay: given a provider identity, return models with their pricing
- [ ] No outbound HTTP at switchboard startup

### Tests

| Test | What it validates |
|------|-------------------|
| `models_load_snapshot` | Bundled snapshot deserializes without error |
| `models_merge_override` | TOML override wins over bundled value |
| `models_lookup_found` | Known model returns metadata |
| `models_lookup_missing` | Unknown model returns `None` |
| `models_provider_pricing` | Provider pricing overlay works |

---

## T3: Routing Algorithm (Pure Function)

**Spec ref:** §8.2 Candidate Selection, FR2, FR3  
**Plan ref:** §4 Phase 3 (router.rs)  
**Testable without:** HTTP server, database, credential helpers, network

### Deliverables

- `crates/agentkit-switchboard/src/proxy/router.rs` — `select_provider()` pure function
- `crates/agentkit-switchboard/src/provider/mod.rs` — `ProviderState`, `ProviderStatus`

### Function signature

```rust
fn select_provider(
    model: &str,
    api_surface: ApiSurface,
    session: Option<&SessionAffinity>,
    providers: &HashMap<String, ProviderState>,
) -> Result<ProviderSelection, RoutingError>
```

### Acceptance criteria

- [ ] Subscription provider selected over pay-as-you-go when both serve the model and subscription is not degraded
- [ ] Subscription degraded → falls through to pay-as-you-go
- [ ] Two pay-as-you-go providers ranked by cost (cheaper wins)
- [ ] Tiebreaker: lexical identity string
- [ ] Unknown model returns `RoutingError::ModelNotFound`
- [ ] Model served only by unconfigured provider returns `RoutingError::NoProvider`
- [ ] Session affinity: same session ID returns same provider (when healthy)
- [ ] Session affinity breaks on degradation: re-routes, includes switch info in result
- [ ] All providers degraded → `RoutingError::AllDegraded`

### Tests

| Test | What it validates |
|------|-------------------|
| `routing_prefers_subscription` | Subscription > pay-as-you-go |
| `routing_falls_through_on_quota_exhausted` | Degraded sub → payg |
| `routing_ranks_by_cost` | Cheaper payg wins |
| `routing_tiebreaker_identity` | Lexical tiebreak |
| `routing_model_not_available` | Unknown model → error |
| `routing_no_credential` | Unconfigured provider excluded |
| `routing_session_affinity` | Same session → same provider |
| `routing_session_breaks_on_degradation` | Degraded → re-route |
| `routing_all_degraded` | All degraded → error |

---

## T4: Credential Helper Protocol

**Spec ref:** §6.5 Credential Resolution, §6.7 Credential Helper Protocol, §12.9  
**Plan ref:** §4 Phase 6 (credential module)  
**Testable without:** HTTP server, database, routing, OAuth server

### Deliverables

- `crates/agentkit-switchboard/src/credential/mod.rs` — `ResolvedCredential`, `CredentialSource`
- `crates/agentkit-switchboard/src/credential/env.rs` — env var reader
- `crates/agentkit-switchboard/src/credential/helper.rs` — helper binary exec + protocol parsing

### Acceptance criteria

- [ ] `helper::get(identity)` execs `agentkit-credential-{name} get {identity}`, parses stdout JSON
- [ ] Helper returns valid JSON → returns `ResolvedCredential`
- [ ] Helper not in PATH → returns `None`, logs warning
- [ ] Helper returns non-zero → returns `None`, logs error
- [ ] `env::read(var_name)` returns env var value or `None`
- [ ] Resolution order: helper first, env var fallback, then unconfigured
- [ ] `auth.type == "none"` → `CredentialSource::None` immediately
- [ ] Token expiry check: compares `expires_at` to current time, returns `OAuthState` with refresh info

### Tests

| Test | What it validates |
|------|-------------------|
| `credential_helper_get` | Mock helper stdout parsed correctly |
| `credential_helper_store` | Helper invoked with correct stdin |
| `credential_helper_not_found` | Missing helper → fallback |
| `credential_helper_nonzero_exit` | Helper error → fallback |
| `credential_env_var` | Env var read correctly |
| `credential_env_missing` | Missing env var → None |
| `credential_resolution_order` | Helper > env var > unconfigured |
| `credential_none_type` | `auth.type = "none"` skips all |
| `credential_token_expiry` | Expiry check works |

---

## T5: Credential Helper Binaries

**Spec ref:** §6.7 Credential Helper Protocol, §6.7.4 Shipped Helpers  
**Plan ref:** §4 Phase 7  
**Testable without:** HTTP server, switchboard proxy, OAuth

### Deliverables

- `crates/agentkit-credentials/Cargo.toml` — workspace crate with two bin targets
- `crates/agentkit-credentials/src/lib.rs` — shared `CredentialJson` type, serde
- `crates/agentkit-credentials/src/bin/agentkit-credential-keychain.rs`
- `crates/agentkit-credentials/src/bin/agentkit-credential-file.rs`
- `crates/agentkit-credentials/tests/keychain.rs`
- `crates/agentkit-credentials/tests/file.rs`

### Acceptance criteria

- [ ] `agentkit-credential-keychain get foo` — exits 0 with JSON on stdout if exists, exits 1 if not
- [ ] `agentkit-credential-keychain store foo` — reads JSON from stdin, writes to keychain
- [ ] `agentkit-credential-keychain erase foo` — removes keychain entry
- [ ] `agentkit-credential-file` — same operations via `~/.agentkit/credentials.json`
- [ ] File helper creates `~/.agentkit/` with `0700` perms on first write
- [ ] File helper writes with `0600` perms
- [ ] Invalid JSON on stdin → exit code 2, error message on stderr
- [ ] Missing identity argument → exit code 2, usage on stderr

### Tests

| Test | What it validates |
|------|-------------------|
| `keychain_get_store_erase` | Full lifecycle via keychain |
| `keychain_get_not_found` | Missing entry exits 1 |
| `file_get_store_erase` | Full lifecycle via file |
| `file_permissions` | File created with 0600 |
| `file_directory_creation` | `~/.agentkit/` created with 0700 |
| `helper_invalid_json` | Bad stdin → exit 2 |
| `helper_missing_args` | No identity → exit 2 |

---

## T6: Session Manager (In-Memory + SQLite)

**Spec ref:** §8.6 Session Affinity and Persistence, §8.6.3 Session Database Schema, FR4, FR11  
**Plan ref:** §4 Phase 4  
**Testable without:** HTTP server, routing, credentials, network

### Deliverables

- `crates/agentkit-switchboard/src/session/mod.rs` — `SessionManager` trait
- `crates/agentkit-switchboard/src/session/memory.rs` — HashMap impl
- `crates/agentkit-switchboard/src/session/sqlite.rs` — sqlx impl
- `crates/agentkit-switchboard/src/db/mod.rs` — connection pool, migration runner
- `crates/agentkit-switchboard/src/db/migrations/001_session_schema.sql`

### SessionManager trait

```rust
#[async_trait]
trait SessionManager: Send + Sync {
    async fn lookup(&self, session_id: &str) -> Result<Option<SessionAffinity>, SessionError>;
    async fn assign(&self, session_id: &str, provider: &str, model: &str, surface: &str) -> Result<(), SessionError>;
    async fn update_tokens(&self, session_id: &str, input: u64, output: u64) -> Result<(), SessionError>;
    async fn increment_switch(&self, session_id: &str, new_provider: &str) -> Result<(), SessionError>;
    async fn insert_routing_event(&self, event: RoutingEvent) -> Result<(), SessionError>;
}
```

### Acceptance criteria

- [ ] `lookup` returns `None` for unknown session, `Some(...)` for known
- [ ] `assign` creates row; second call updates in-place (upsert)
- [ ] `update_tokens` increments cumulative counters
- [ ] `increment_switch` updates provider_identity and increments switch_count
- [ ] `insert_routing_event` appends to routing_events table
- [ ] SQLite: database file created at configured path on first access
- [ ] SQLite: migrations run on connect
- [ ] SQLite: write failure logs error, does not panic
- [ ] SQLite: corrupt file renamed to `.sessions.db.corrupted`, fresh DB created
- [ ] Memory impl passes same test suite as SQLite (parameterized)

### Tests

| Test | What it validates |
|------|-------------------|
| `session_lookup_missing` | Unknown session → None |
| `session_assign_and_lookup` | Assign then lookup returns correct data |
| `session_assign_upsert` | Second assign updates existing row |
| `session_update_tokens` | Token counters accumulate |
| `session_increment_switch` | Switch count increments, provider changes |
| `session_routing_event` | Event inserted and queryable |
| `session_db_corruption` | Corrupt DB → fallback, rename, recreate |
| `session_memory_impl` | Memory impl passes all trait tests |
| `session_sqlite_impl` | SQLite impl passes all trait tests |

---

## T7: Quota State Machine and Header Parsing

**Spec ref:** §8.4 Quota Tracking, §8.5 Degradation and Recovery, FR3, FR8, FR12  
**Plan ref:** §4 Phase 5  
**Testable without:** HTTP server, database, credentials, network

### Deliverables

- `crates/agentkit-switchboard/src/provider/quota.rs` — `QuotaState`, header parsing, degradation machine

### Acceptance criteria

- [ ] OpenAI `x-ratelimit-remaining-requests` header parsed to `u32`
- [ ] OpenAI `x-ratelimit-remaining-tokens` header parsed to `u64`
- [ ] Anthropic `anthropic-ratelimit-requests-remaining` header parsed
- [ ] Missing headers → state remains `None` (no error, no degradation)
- [ ] 429 with `retry-after: 30` → degraded for 30s
- [ ] 429 with `insufficient_quota` in body → permanently degraded
- [ ] 429 with no headers → degraded for 60s (default)
- [ ] 401/403 → permanently degraded
- [ ] 5xx → degraded for 30s, exponential backoff (30s, 60s, 120s, 240s, 300s max)
- [ ] Timeout → degraded for 10s, exponential backoff (10s, 20s, 40s, 120s max)
- [ ] Successful response → clear degradation, reset retry_count
- [ ] Subscription provider: 429 → degraded for 5-hour cooldown
- [ ] Degradation expiry checked lazily (on routing decision, not via timer)

### Tests

| Test | What it validates |
|------|-------------------|
| `quota_headers_openai` | OpenAI headers parsed |
| `quota_headers_anthropic` | Anthropic headers parsed |
| `quota_headers_missing` | Missing headers → None |
| `quota_429_retry_after` | 429 with retry-after degrades for that duration |
| `quota_429_insufficient_quota` | Permanent degradation |
| `quota_429_default` | 429 without headers → 60s |
| `quota_401_permanent` | 401 → permanent |
| `quota_5xx_backoff` | 5xx with exponential backoff |
| `quota_timeout_backoff` | Timeout with backoff |
| `quota_success_clears` | 200 clears degradation |
| `quota_subscription_429` | Subscription 429 → 5h cooldown |
| `quota_degradation_expired` | Past degraded_until → re-enabled |

---

## T8: Request Translation (Chat Completions ↔ Responses API)

**Spec ref:** §8.3 Request Forwarding (translation step), Chat Completions↔Responses API  
**Plan ref:** §4 Phase 3 (forwarder translation logic)  
**Testable without:** HTTP server, network, database, credentials

### Deliverables

- `crates/agentkit-switchboard/src/proxy/translation.rs` — request/response translation functions

### Translation: Chat Completions → Responses API (request)

```
Input:  { model, messages: [{role, content}], stream, temperature, max_tokens }
Output: { model, input: [{type: "message", role, content: [{type: "input_text"|"output_text", text}]}],
          instructions (from system message), stream, temperature, max_tokens,
          store: false, reasoning: {effort: "medium"} }
```

### Translation: Responses API → Chat Completions (response)

```
Input:  { output: [{type: "message", role, content: [{type: "output_text", text}]}], usage }
Output: { choices: [{index: 0, message: {role, content}, finish_reason: "stop"}], usage }
```

### Acceptance criteria

- [ ] Chat Completions request with `messages` array → Responses API `input` array
- [ ] System message → `instructions` field (removed from input array)
- [ ] User message → `input` with `input_text` content type
- [ ] Assistant message → `input` with `output_text` content type
- [ ] `stream`, `temperature`, `max_tokens` passed through unchanged
- [ ] `store: false` and `reasoning: {effort: "medium"}` added
- [ ] Responses API response → Chat Completions `choices` array
- [ ] `usage` passed through unchanged
- [ ] Non-streaming only: streaming input returns error
- [ ] Unknown message role → error

### Tests

| Test | What it validates |
|------|-------------------|
| `translate_request_basic` | Messages → input conversion |
| `translate_request_system_message` | System → instructions |
| `translate_request_streaming` | Streaming passthrough |
| `translate_request_params` | temperature, max_tokens passthrough |
| `translate_request_adds_fields` | store, reasoning added |
| `translate_response_basic` | Output → choices conversion |
| `translate_response_usage` | Usage passthrough |
| `translate_response_streaming` | Streaming input rejected |
| `translate_unknown_role` | Unknown role → error |

---

## T9: Auth Login OAuth Flow

**Spec ref:** §7.1 Auth Login Flow, §7.2 Auth Token Command, FR10  
**Plan ref:** §4 Phase 6 (auth module)  
**Testable without:** HTTP proxy server, routing, database, real upstream providers

### Deliverables

- `crates/agentkit-switchboard/src/auth/mod.rs` — subcommand dispatch
- `crates/agentkit-switchboard/src/auth/openai_codex.rs` — OAuth flow

### OAuth flow steps

1. Read config, find provider by identity
2. Read `[auth.oauth]` config (authorize_url, token_url, scopes)
3. Generate PKCE verifier + S256 challenge
4. Start local HTTP server on port 1455
5. Build authorize URL with params (client_id, redirect_uri, code_challenge, etc.)
6. Open browser
7. Receive callback, validate state
8. Exchange code for tokens at token_url
9. Extract `chatgpt_account_id` from JWT
10. Store via credential helper

### Acceptance criteria

- [ ] `auth login` reads provider config and `[auth.oauth]` section
- [ ] PKCE verifier + challenge generated correctly (S256)
- [ ] Authorize URL built with all required params
- [ ] Local callback server starts on port 1455
- [ ] Callback received, state validated
- [ ] Authorization code exchanged for tokens
- [ ] Tokens stored via credential helper `store`
- [ ] `chatgpt_account_id` extracted from JWT
- [ ] Token refresh: expired token → refresh → store new tokens
- [ ] `auth status` shows credential source and expiry
- [ ] `auth token <identity>` prints env var assignment
- [ ] `auth logout <identity>` calls helper `erase`
- [ ] Missing `[auth.oauth]` config prints clear error

### Tests

| Test | What it validates |
|------|-------------------|
| `auth_login_pkce` | PKCE verifier + challenge correct |
| `auth_login_authorize_url` | URL contains all required params |
| `auth_login_callback` | Callback received, state validated |
| `auth_login_token_exchange` | Code exchanged for tokens |
| `auth_login_store` | Tokens stored via helper |
| `auth_login_missing_oauth_config` | Missing config → error |
| `auth_token_command` | Prints correct env var |
| `auth_status_command` | Shows provider states |
| `auth_logout_command` | Calls helper erase |
| `token_refresh` | Expired → refresh → store |

---

## T10: HTTP Server, Route Dispatch, and Integration

**Spec ref:** §3.1 Deployment Diagram, §8.1 Route Dispatch, §9 Endpoints, FR1, FR5  
**Plan ref:** §4 Phase 3 (server module)  
**Depends on:** T1 (config), T2 (models), T3 (routing), T4 (credentials), T6 (sessions), T7 (quota), T8 (translation)

### Deliverables

- `crates/agentkit-switchboard/src/main.rs` — tokio main, clap derive, server start
- `crates/agentkit-switchboard/src/cli.rs` — Cli struct, subcommands
- `crates/agentkit-switchboard/src/server/mod.rs` — axum Router, middleware stack, graceful shutdown
- `crates/agentkit-switchboard/src/server/routes.rs` — handler functions
- `crates/agentkit-switchboard/src/server/middleware.rs` — request ID, session ID extraction, logging
- `crates/agentkit-switchboard/src/proxy/forwarder.rs` — URL rewrite, auth injection, wire up translation
- `crates/agentkit-switchboard/src/provider/registry.rs` — `Arc<HashMap<identity, ProviderState>>`

### This task wires together all independent units into a running proxy

The server:
1. Loads config (T1)
2. Loads model metadata (T2)
3. Initializes provider registry with credential resolution (T4)
4. Starts session manager (T6)
5. Registers routes: `POST /openai/v1/chat/completions`, `GET /openai/v1/models`, `GET /health`
6. On request: dispatch by path → select provider (T3) → resolve credential (T4) → translate if needed (T8) → forward → update quota (T7) → update session (T6)

### Acceptance criteria

- [ ] `switchboard --config cfg.toml` starts and binds to configured address
- [ ] `POST /openai/v1/chat/completions` with valid model returns 200
- [ ] `POST /openai/v1/chat/completions` with unknown model returns 503
- [ ] `POST /openai/v1/chat/completions` with `stream: true` returns SSE (for API key providers)
- [ ] `POST /openai/v1/chat/completions` with `stream: true` returns 400 (for Codex subscription providers)
- [ ] `GET /openai/v1/models` returns merged model list
- [ ] `GET /health` returns provider states and session DB info
- [ ] Unknown path returns 404
- [ ] Response includes `X-Switchboard-Provider`, `X-Switchboard-Billing`, `X-Switchboard-Session` headers
- [ ] Graceful shutdown on SIGTERM/SIGINT
- [ ] Startup time < 2s with 10 providers configured

### Tests

| Test | What it validates |
|------|-------------------|
| `server_starts` | Binary starts and binds |
| `route_dispatch_by_path` | Path prefix selects correct handler |
| `route_unknown_path` | 404 for unknown paths |
| `proxy_completes_request` | Full round-trip via wiremock |
| `proxy_streams_response` | SSE streaming via wiremock |
| `proxy_429_retry` | First provider 429 → retry with second |
| `proxy_all_degraded_503` | All degraded → 503 |
| `proxy_session_persistence` | Session → restart SQLite → same provider |
| `proxy_model_list` | GET /openai/v1/models returns merged list |
| `proxy_health` | GET /health returns provider states |
| `proxy_codex_translation` | Chat Completions → Responses API via wiremock |
| `proxy_credential_helper` | Request with helper-based credential |
| `server_graceful_shutdown` | SIGTERM shuts down cleanly |

---

## Sequencing

Tasks T1-T9 are independent and can be parallelized:

```
Week 1:   T1 ──┐
          T2 ──┤
          T3 ──┤
          T4 ──┤
          T5 ──┤
          T6 ──┤
          T7 ──┤
          T8 ──┤
          T9 ──┘
                │
Week 2:   T10 ──  (wires everything together)
```

T10 is the only task with dependencies — it needs T1-T9 complete to wire up the full proxy. Everything else can be built and tested in isolation.
