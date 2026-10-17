# E2E model-and-tools test suite — plan

Status: **executed** (2026-10-02). All work items written and run; the
recorded red lists and the corrections the run forced are in §9. Written
after code-level verification of every claim below (file:line references
were live at writing; §9 notes where execution moved them).

## 1. Why this suite exists

All 41 ADR tasks are checked `[x]` and the test suite is green, but four
integration seams are unwired — the headline acceptance criteria (AC-001
agent loop, AC-005 permission pipeline over the wire) have never run:

| # | Seam | Evidence |
|---|------|----------|
| 1 | `ModelRequest` has no `tools` field; tool-call stream content is discarded | `src/agent/mod.rs:32-37`, `src/agent/provider.rs:260` (`Ok(_) => {} // tool calls land with the registry (T-021)`) |
| 2 | `run_turn` never calls the registry/pipeline — no tool-call loop despite T-021 | `src/agent/turn.rs:59` loop runs once, returns at :214 (`// v1: the model made no tool requests`) |
| 3 | `PermissionPipeline` has no production caller; `session/request_permission` is never sent | `src/permissions/mod.rs` referenced only by its own `mod tests`; `main.rs` still uses the `DenyPrompt` stub |
| 4 | System prompt = persona body only, no manifest injection (FR-012) | `src/acp/mod.rs:911` (`let system = persona.map(...)`) |

Why existing tests miss all of it: every `session/prompt` in the suite hits
an intercepted path (`/fork`, direct invocation, `/mcp`, compaction) or a
lease error; endpoint-configuring tests point at dead addresses
(`http://127.0.0.1:1`) and never prompt. The plain model path in
`acp/mod.rs` (~line 784 onward) is entered by no test.

## 2. Decisions (settled with the user)

- **Client**: the SDK `Client` role from `agent-client-protocol` (already
  vendored at 2.2.0), with `on_receive_request` answering
  `session/request_permission`. Pattern proven by the crate's
  `examples/yolo_one_shot_client.rs` (auto-approve: pick
  `request.options.first().option_id`, respond
  `RequestPermissionResponse::new(RequestPermissionOutcome::Selected(...))`).
- **Model**: a scripted fake OpenAI-compatible endpoint (wiremock, already a
  dev-dependency) is the default. It records every request and answers from
  the request body. A live-model smoke test is gated by
  `AGENTKIT_TESTS_CAN_SPEND_MONEY=1`, stdio only.
- **Scope**: coverage at the right layer — two new test files; intricate
  permission matrices stay in unit tests (they already exist and are
  comprehensive, see §6).
- **Transports**: the tool-turn scenario runs over **both** stdio and HTTP;
  HTTP is scripted-model only.
- **Assertions**: imperative, on captured notifications and on what the mock
  *received*. Goldens/transcripts later, not now.
- **HTTP client**: hand-rolled test-side adapter (option a) — the stock
  `HttpClient` from `agent-client-protocol-http` speaks a different wire
  dialect (mismatch table in §5.3); adopting it would mean rewriting the
  server, which is a separate project.

## 3. Coverage map (AC → layer → verdict)

| Behaviour | Layer | Existing coverage | Verdict |
|---|---|---|---|
| AC-001 agent loop: streamed chunks, tool calls, stopReason, tools offered, usage cadence, stable-only gating during a real turn | E2E | **nothing** | **NEW: `tests/tool_turn.rs`** (stdio + HTTP) |
| AC-005 permission *wiring*: unconfigured tool prompts, allow-once, allow-always honoured in-session, honest session-scope labels | E2E | unit only (§6) | **Fold into the same scripted session** in `tool_turn.rs` |
| AC-005 permission *matrices*: pipeline order, veto, fail-closed, scoping, expiry | unit | `src/permissions/mod.rs:306-617` — 11 tests, comprehensive | ✅ none needed |
| AC-002 persistence: load replays **full history**, stable ids, single usage snapshot | integration (model-free) | `tests/initialize.rs:152` loads an **empty** session — asserts nothing about replay | **NEW: `tests/restart_load.rs`** via direct invocation |
| AC-007 reusable prompts `/name` | integration + unit | implemented (T-023); unit tests **exist** (`invokables/mod.rs:833-927`, green) — the *wire* path is what is unwired | fold one `/name` prompt into E2E (A10); arity/unknown-command errors → unit test on the expansion fn (already present) |
| AC-003 fork fallback | integration | `tests/fork.rs` | ✅ |
| AC-004 compaction + usage_update drop | integration | `tests/compaction.rs` | ✅ |
| AC-006 direct invocation | integration | `tests/initialize.rs:292`, parse tests in `mcp_pool.rs` | ✅ |
| AC-008 first run | integration | `tests/first_run.rs` | ✅ |
| AC-009 multi-instance lease | integration | `tests/two_process.rs` | ✅ |
| Command surface / capability advertisement | integration | `tests/initialize.rs:32,210` | ✅ |
| HTTP wire basics | integration | `tests/http.rs` (raw JSON-RPC) | ✅ — E2E adapter adds the typed layer |
| HTTP response-only POST routing | unit/regression | **bug found, no test** | **NEW: fix + regression test (§4.4)** |

## 4. Work items

### 4.1 `crates/agentkit-tackle/tests/tool_turn.rs` — the E2E test

**Config written by the test** (pattern: `tests/config_options.rs:14-28`):

```toml
[endpoints.mock]
base_url = "<wiremock uri>"
wire_format = "openai-chat-completions"
auth = "none"
models = ["MiniMax-M2"]        # static list → no /models discovery needed

[defaults]
model = "mock/MiniMax-M2"      # bare name "MiniMax-M2" IS in the bundled
                               # snapshot (204_800) → context_window() > 0
                               # → usage_update cadence fires
                               # (acp/mod.rs:1458-1462, usage gate at :1006)

[mcp_servers.echo]
transport = "stdio"
command = "<CARGO_BIN_EXE_tackle-mcp-echo>"
```

`MiniMax-M2` chosen deliberately: `context_window("mock/MiniMax-M2")` →
`Some(204_800)` (proven by `config_options.rs:207`), so the per-request
`usage_update` path (`acp/mod.rs:1000-1013`) is live. With an unknown model
window `size == 0` and usage notifications are silently skipped.

**Scripted model** — wiremock with a custom `wiremock::Respond` impl,
content-based (order-independent, tolerates the concurrent titling fork):

1. request text contains `Write a title` → plain streamed text
   (`titling.rhai` forks an ephemeral session and prompts with that exact
   sentence; its turn goes through the same mock).
2. any message JSON containing the marker `E2E-MARKER-9f3a` (which only the
   *model itself* ever emits, as tool-call arguments) → final answer text
   `done: E2E-MARKER-9f3a` with `finish_reason: stop`.
3. request contains the user trigger → tool-call response:
   `delta.tool_calls[{index:0, id:"call_e2e", function:{name:"mcp.echo.echo",
   arguments:"{\"text\":\"E2E-MARKER-9f3a\"}"}}]` then
   `finish_reason:"tool_calls"`, terminal usage chunk, `[DONE]`.
4. else → plain `ok`.

Rule 2 is deliberately robust to how the (future) tool-loop fix represents
tool results: `ChatRole` is only `User | Assistant` today
(`agent/mod.rs:14-17`), so the fix will have to surface the result as text
in one of those — either way the marker reaches request 2's history. If the
fix forgets to pass the result back, the mock re-issues a tool call, the
turn runs to `max_model_requests`, and the test fails red — correct
behaviour for a wiring bug.

SSE shape copied from the proven fixture
`provider.rs:523-539` (data lines, terminal usage chunk, `[DONE]`); the
tool-call chunk shape follows rig 0.42's
`StreamingToolCall` (`providers/openai/completion/streaming.rs:33-52`).
OpenAI's `EMITS_COMPLETE_SINGLE_CHUNK_TOOL_CALLS` is `false`
(`openai/completion/mod.rs:1488`), i.e. rig aggregates deltas and emits the
complete call at `finish_reason` — the scripted shape must therefore span
≥2 chunks (delta + finish). To be validated empirically on first run; if rig
errors instead of parsing, only the fixture's SSE shape changes, not the
test design.

**Client** (house pattern + `yolo_one_shot_client.rs`):

- `on_receive_notification` captures every `SessionNotification` into
  `(role, text)` / tool-call / usage vectors, tagged with session id.
- `on_receive_request(RequestPermissionRequest, responder, connection)`:
  records the request (options, tool_call), then answers from a queue:
  - round 1 → option whose label is `Allow for this session`
    (`ask_options()` labels are spec'd at `permissions/mod.rs:93-100`:
    "Allow once" / "Allow for this session" / "Reject once" / "Reject for
    this session");
  - round 2 (veto) → `Allow once` if asked at all.

**Scenario** (one session, prompts in order):

1. `session/prompt "run the echo tool"` → mock returns tool call →
   (fixed product) pipeline asks → client selects *Allow for this session*
   → tool executes → second model request consumes the result → final text
   → `stopReason: end_turn`.
2. `session/prompt "run the echo tool again"` → allow-always grant means
   **no second permission request**; tool runs; final text.
3. Veto prompt with a `[scripts.veto] events = ["pre_tool_use"]` script
   (`file = "veto.rhai"` in the test's config-dir; user-layer scripts are
   not TOFU-gated — trust gating is for *project* scripts,
   config.example.toml:74-82) returning
   `#{ decision: "deny", reason: "vetoed by test policy" }`
   (contract: `scripts-api.md:21-40`) → tool result carries the denial,
   model sees it, final answer.
4. One `/name`-style prompt only if a reusable prompt fixture proves cheap
   (see §4.3) — otherwise AC-007 stays on the unit level.

**Assertions** (all after the prompt response returns — no hangs even when
seams are unwired, because a broken agent answers `end_turn` immediately):

| # | Assertion | Source | Status now |
|---|---|---|---|
| A1 | `ToolCall` update with tool name `mcp.echo.echo` | AC-001 | 🔴 red (seam 2) |
| A2 | ≥1 `RequestPermissionRequest`; options contain both an allow-once and an allow-for-session label; `tool_call` names `mcp.echo.echo` | AC-005 | 🔴 red (seam 3) |
| A3 | `ToolCallUpdate` reaches `Completed` with content containing `E2E-MARKER-9f3a` (echo output) | AC-001 | 🔴 red |
| A4 | final `agent_message_chunk`s contain `done: E2E-MARKER-9f3a`; prompt returns `EndTurn` | AC-001 | 🔴 red (round 2 never happens) |
| A5 | ≥2 `usage_update` **for our session id** (filter by id — titling's fork is a different session) with `used == 42` (scripted `prompt_tokens`) | usage cadence | 🔴 red (only 1 request today) |
| A6 | mock received request with a `tools` array containing `mcp.echo.echo` | seam 1 / FR-013 | 🔴 red (no tools field) |
| A7 | mock received `system` message containing `!mcp.echo.echo` **and** `Echoes the provided text` | FR-012 | 🔴 red (seam 4, persona-only) |
| A8 | prompt 2 produces **no** new `RequestPermissionRequest` (allow-always honoured, session-scoped) | AC-005 | 🔴 red (needs A2 first) |
| A9 | veto prompt: tool result / model context carries the deny reason; no execution side effect | AC-005 | 🔴 red |

Option ids: assert on **labels**, not ids — ids are product-chosen and not
spec'd; `ask_options()` fixes only the label strings.

**Live-model smoke test**, same file:
`#[ignore]` + skip unless `AGENTKIT_TESTS_CAN_SPEND_MONEY=1`; stdio only;
one prompt, one tool call, `end_turn`.

### 4.2 `crates/agentkit-tackle/tests/restart_load.rs` — AC-002, model-free

Key insight: AC-006 direct invocation creates *real turn history without a
model* (user message + ToolCall + ToolResult rows,
`acp/mod.rs:699-727`). Sequence:

1. spawn agent (config: mcp echo only — no endpoint needed),
   `session/new`, `/!mcp.echo.echo {"text":"persist-me"}` → `EndTurn`;
2. drop the agent (process exits), respawn on the same `--db-path`;
3. `session/load` → assert replayed notifications contain the user message
   and the tool-call turn, message ids stable across restart, exactly one
   usage snapshot.

Supersedes the misleading `session_load_replays_history_with_stable_ids`
(`initialize.rs:152`, which loads an empty session) — mark the old test as
superseded or tighten it rather than leaving a false-green name.

### 4.3 AC-007 reusable prompts

- E2E fold-in: only if prompt fixtures are declarative and cheap
  (a `prompts/<name>.md` in config-dir). Decide when the file layout is
  read; if it needs trust/extra config, drop to unit only and note it.
- Unit: arity mismatch / unknown command → precise error naming the usage
  string, next to the expansion function (find `prompt expansion` in the
  T-023 implementation before writing).

### 4.4 Fifth finding: HTTP response-only POST hang (fix + regression test)

`src/acp/http.rs:114-134` `dispatch_and_await`:

- registers a `pending` entry for **any** message with an `id` (line 122),
  including a client's *response* to an agent-initiated request;
- then awaits an agent-written line with that id (line 131) — which never
  comes, because the agent consumes responses silently (they enter via
  `incoming`, never pass `outgoing_router`).

Consequence: the client's POST of a `session/request_permission` answer
hangs until the 300 s idle reaper (`http.rs:26`). Today this is unreachable
(seam 3 means permission is never sent); the moment the seams are fixed the
HTTP half of the tool-turn test deadlocks. Fix (small, transport-level):

- if the parsed body is response-only (`id` present, `method` absent):
  forward to `incoming`, return `{}` immediately, never touch `pending`;
- regression test: POST a response-shaped message with a short
  `tokio::time::timeout` — completes promptly, currently hangs.

This is transport plumbing required for the HTTP E2E to be meaningful; the
four product seams remain follow-up work driven by the red list.

### 4.5 Unit layer

- **Permission matrices: already complete** —
  `src/permissions/mod.rs:306-617` covers deny short-circuit, script
  allow/deny finality, ask fall-through, fail-closed, `previously_granted`,
  cancel-reject-once, allow-always scoping + actor/invokable keying, grant
  expiry, ask-reason content, TOFU consent. Do not duplicate.
- **Tool-call stream parsing** (seam-1 fix guard): a unit test asserting
  `stream_completion` surfaces tool calls cannot be written *today* —
  `ModelResponse` has no field to surface them through
  (`agent/mod.rs:46-50`). Write it alongside the product fix, not before.

## 5. Wire-format & infrastructure facts (learned, verified)

### 5.1 Model endpoint

- rig 0.42 openai-compatible streaming: SSE `data:` lines, terminal chunk
  with `usage`, `data: [DONE]` (`provider.rs:523-539` fixture is proven).
- Request body: `model` is the *bare* wire name; system rides as a
  `role: system` message; `tools` would be a top-level `tools` array
  (currently never sent — seam 1).
- Non-streaming path (`complete()`) exists for scripts; the mock's `Respond`
  must branch on `body["stream"]` to answer both shapes.

### 5.2 Config & resolution

- `[endpoints.<name>] base_url / wire_format / auth / models`; models
  addressed `<endpoint>/<model>`; `[defaults].model` picks the default.
- Model resolution chain (`acp/mod.rs:912-934`): session metadata → actor
  frontmatter → `[defaults].model` → first listed selector option; empty is
  a hard error (`no model configured…`).
- `context_window()` strips the endpoint prefix and consults the bundled
  models.dev snapshot (`acp/mod.rs:1458-1462`); unknown window ⇒ no
  `usage_update`.
- Titling: fires detached after every turn for untitled sessions
  (`acp/mod.rs:1081-1086`, `titling.rhai`); forks an ephemeral session and
  prompts `"Write a title for this session…"` — the mock must tolerate it,
  and assertions must filter notifications by session id.
- MCP servers: `[mcp_servers.echo] transport = "stdio"`, tool namespaced
  `mcp.echo.echo`, description `Echoes the provided text`
  (`bin/tackle-mcp-echo.rs`).

### 5.3 HTTP transport: our shape vs stock `HttpClient`

| | Stock `HttpClient` (agent-client-protocol-http 2.2.0) | Our `http.rs` |
|---|---|---|
| Routes | one URL for POST + SSE GET + DELETE | `POST /rpc`, `GET /events?connection=`; no DELETE |
| Connection id | header `acp-connection-id` everywhere | `x-acp-connection` header on POST, **query param** for SSE |
| Session id | `acp-session-id` header | not handled |
| Response delivery | POST bodies discarded (except initialize); responses arrive over SSE | blocking POST body; `outgoing_router:66-70` routes by-id lines *away* from SSE |
| SSE topology | connection stream + per-session streams | single broadcast stream |
| Close | DELETE | 300 s idle reaper |

⇒ stock client would hang on *every* request (it ignores POST bodies while
we only answer via bodies) — symmetric to the §4.4 bug. Hence the hand-rolled
adapter:

- POST outgoing frames to `/rpc?connection=<id>` (or `x-acp-connection`),
  feed the JSON body back as an incoming message;
- one `GET /events?connection=<id>` SSE task feeding the same incoming
  channel (raw plumbing already proven in `tests/http.rs:46-72`);
- bridge both into the SDK so `Client.builder()...connect_with(adapter, …)`
  runs the *identical* closure and assertions as the stdio run.

### 5.4 ACP schema bits

- `RequestPermissionRequest { session_id, tool_call: ToolCallUpdate, options:
  Vec<PermissionOption>, meta }` (schema v1 `client.rs:968`); respond with
  `RequestPermissionOutcome::Selected(SelectedPermissionOutcome { option_id })`
  or `Cancelled`.
- `PromptRequest::new(session_id, Vec<ContentBlock>)`; responses carry
  `StopReason::{EndTurn, MaxTurnRequests, …}`.
- Permission labels (spec'd): `ask_options()` in `permissions/mod.rs:93-100`.

## 6. Expected red list (what the suite should expose on first run)

1. No tool call reaches the client / no loop → A1, A3, A4 red (seams 1+2).
2. No `session/request_permission` → A2 red (seam 3); A8/A9 follow.
3. Mock receives no `tools` array → A6 red (seam 1).
4. System prompt lacks manifest → A7 red (seam 4).
5. One `usage_update` instead of two → A5 red (consequence of seam 2).
6. (After seams are fixed, before §4.4 fix) HTTP run deadlocks at the
   permission round-trip → regression test in §4.4 catches it first.

`restart_load.rs` is **not** expected green: it is the probe that exposes
FR-005/AC-002 gaps (see §9.2). The rest of the existing suite is green —
it protects against regressions while the seams get wired.

## 7. Verification

1. `cargo test -p agentkit-tackle` — record the red list (above); everything
   else green.
2. `cargo clippy --all-targets` and `cargo fmt --check`.
3. Once (manually): `AGENTKIT_TESTS_CAN_SPEND_MONEY=1 cargo test -p
   agentkit-tackle tool_turn -- --ignored`.
4. No edits to existing tests except superseding the empty-load assertion in
   `initialize.rs:152`.

## 8. Out of scope

- Rewriting `http.rs` to the standard ACP HTTP profile (stock `HttpClient`
  adoption) — §5.3 is its justification; separate decision, separate task.
- Fixing the four product seams — this suite *exposes* them; fixes are
  follow-up driven by §6, each with its unit test (e.g. seam 1 needs a
  `ModelResponse`/callback extension before a unit test can compile).
- Golden/transcript conformance for the new scenarios.

## 9. Execution record (2026-10-02)

The suite as planned; every failure below is *recorded* — each test
accumulates its whole red list in one run instead of aborting on the
first assertion.

### 9.1 `tool_turn.rs` — red list (stdio and HTTP runs, identical)

`scripted_model_tool_turn_over_stdio` and
`scripted_model_tool_turn_over_http` both fail with the same ten
assertions:

1. A1 no `ToolCall` announcement for `mcp.echo.echo`
2. A2 0 permission requests (expected 1 after prompt 1)
3. A3 no `Completed` tool update carrying `E2E-MARKER-9f3a`
4. A4 no final text `done: E2E-MARKER-9f3a`
5. A5 1 `usage_update` after prompt 1 (expected ≥ 2 — one per model
   request in the turn)
6. A8 0 permission requests after prompt 2 (expected 1)
7. A6 no `tools` array in any model request (4 requests seen)
8. A7 no manifest in any system message
9. A9 the deny reason `vetoed by test policy` never reached a model request
10. A10 `/greet World` reached the model unexpanded (literal seen,
    expansion not)

The HTTP run reaches all four prompts through the hand-rolled adapter
(§5.3) with no transport-specific failures — the reds are product-only.

### 9.2 `restart_load.rs` — red list (3 of 5)

1. The first-session seed is missing from the replay.
2. The completed tool call is missing from the replay.
3. No usage snapshot after either load (`before=0 after=0`; FR-005 wants
   exactly one per load).

Green: direct invocation ends `EndTurn`; the user message replays;
message ids are stable across the restart (same session loaded in a
fresh process, not a per-phase fresh session).

### 9.3 Findings beyond the planned seams

| # | Finding | Evidence |
|---|---------|----------|
| 6 | `/name` wire path unwired: `expand_user` has no production caller, so user prompts reach the model verbatim | A10 red; `src/invokables/mod.rs:211` |
| 7 | `session/load` never emits the FR-005 usage snapshot | restart_load red 3; the only `UsageUpdate` send sites are `acp/mod.rs:855` and `:1011` |
| 8 | `replay_updates` parses message content as a block array **before** the role match — `Role::ToolCall` rows store raw arguments (`{"text":…}`), fail the parse, and are skipped, so direct-invocation tool calls are never replayed | restart_load red 2; both rows confirmed present via `assemble_context`; `acp/mod.rs:1393-1396` |
| 9 | Prompt turns are appended with `parent_id = NULL` (`acp/mod.rs:526`, whose comment claims "head-following turns chain via the store's head"; `store/mod.rs:541` inserts the parent verbatim, memory backend likewise) → `assemble_context`'s parent walk returns **only the head turn**. Earlier turns (seed, prior interactions) are neither assembled into model context nor replayed: multi-turn model context is current-turn-only | restart_load red 1; direct DB dump shows seed + tool rows present but excluded from the walk |

### 9.4 Corrections and deviations vs the plan

- §6's "`restart_load.rs` … expected **green**" was wrong: the test's
  assertions encode FR-005/AC-002 literally and expose findings 7–9.
  The rest of the pre-existing suite is green.
- §6 item 6 (HTTP deadlock at the permission round-trip) does not
  manifest yet — the seams are unwired, so no permission request crosses
  the wire. The §4.4 regression test (`a_response_only_post_answers_immediately`)
  is in place for when it does.
- A5 is asserted per turn (≥ 2 usage updates after prompt 1,
  baseline-counted), matching §6's red expectation rather than a
  literal reading of "≥2 after every prompt".
- AC-007's coverage row: unit tests existed all along
  (`invokables/mod.rs:833-927`, green — §4.3's advice to find the T-023
  implementation first was right); only the wire path is red (A10).
- `initialize.rs:152` renamed to `session_load_on_an_empty_session_answers`
  with a doc pointer at `restart_load.rs`.
- Test-infra fix: `tests/http.rs`'s parallel `spawn_http_agent` had a
  bind→drop→spawn port race — a sibling test could grab the dropped port
  and the readiness probe would connect to the thief (~1/10 failures,
  made likelier by the third test). The window is now serialized behind
  a static mutex with a child-liveness check; the same check guards
  `tool_turn.rs`'s spawn.

### 9.5 Verification run

- `cargo test -p agentkit-tackle --no-fail-fast` — exactly three failing
  tests on three consecutive runs: `tool_turn` (stdio + HTTP) and
  `restart_load`. Everything else green; `1 ignored` is the gated
  live-model smoke (§7.3, not run — spends real money).
- `cargo clippy --all-targets -p agentkit-tackle` — clean.
- `cargo fmt --check` — clean for every file this work added or edited
  in full; **pre-existing** drift remains in `src/agent/provider.rs`,
  `src/cli.rs`, `src/main.rs`, `src/mcp/mod.rs`, and in
  `tests/initialize.rs` outside the renamed test's hunks — all present
  at HEAD before this work.
