# agentkit-tackle

Tackle: an ACP-native agentic coding harness. It is a single binary,
launched by an ACP client (stdio by default, HTTP for multi-client
hosts), that runs prompt turns against a model, dispatches tool calls
through a permission pipeline, persists the conversation as a turn DAG,
and connects out to MCP servers for external capability.

Terminology is normative in the workspace-root [`CONTEXT.md`](../../CONTEXT.md).

## Composition

Core concepts are root-level modules; anything that exposes or consumes
an RPC is a separate module of glue that maps internal concepts to the
external model.

```
cli/            transport selection, path overrides, `tackle auth`
main.rs         process lifecycle: telemetry, config, engine graph, transport

acp_v1/         the ACP v1 RPC surface (glue): transport, handlers,
                capability negotiation, wire mapping, notifications,
                the permission bridge, session replay, client-tool exec
state.rs        TackleState — shared runtime state (store, config, MCP
                pool, scripts, definitions)

agent/          the core loop and orchestration: run_turn,
                run_prompt_turn, direct invocation, tool-call execution,
                nested agents, fork, compaction, seed, titling, config
                options, the turn sink and TurnRuntime

providers/      the model provider: ModelProvider + model types, rig
                plumbing, the OpenAI chat-completions wire format
credentials.rs  tackle credential policy over agentkit-credentials
invokables/     the unified invokable registry, client-tool descriptors,
                tool-call titles, direct-invocation parsing
mcp/            the MCP client pool: connections, spawn hygiene, stderr
                relay, namespacing, elicitations
permissions/    the fixed-order permission pipeline and grant store
scripts/        the Rhai engine builders and the behaviour host API
store/          the turn DAG: SQLite + in-memory backends, assembly
config/         layered configuration and TOFU trust records
definitions/    persona/actor/prompt types and their loading
builtins/       embedded default assets
telemetry/      logging and OpenTelemetry
```

## Invocation and data flow

```
ACP client ──JSON-RPC──▶ acp_v1
                          │  initialize → capability negotiation (state)
                          │  session/new, session/load, session/prompt …
                          ▼
                        session orchestration ──▶ agent (run_turn)
                          │                          │
                          │                          ▼
                          │                       providers ──▶ model endpoint
                          │                          ▲
                          │            tool calls    │
                          ▼                          │
                     invokables ◀──── dispatch ──────┘
                       │  │  │
              permissions  scripts  mcp ──▶ MCP servers
                       │
                     store ◀── turns, messages, usage
```

- **Inbound**: the ACP client drives everything over JSON-RPC; `acp_v1`
  negotiates capabilities, then maps requests onto core operations.
- **Model**: `agent` runs the turn loop through `providers`; streamed
  text and thoughts flow back through a sink the ACP layer turns into
  `session/update` notifications.
- **Tools**: model tool calls dispatch through `invokables`, gated by the
  `permissions` pipeline, executed against `mcp` (MCP tools), the client
  connection (client tools), or nested `agent` invocations.
- **Scripts**: `scripts` hosts policy (`pre_tool_use` verdicts) and
  behaviour (fork, compaction, titling) scripts; script-driven model calls
  pass back through the normal pipeline.
- **Storage**: every turn and message persists to `store` (SQLite); the
  in-memory backend and the daggy mirror are the test oracle.

## Configuration and definitions

Layered configuration (user layer overridden by project layer, with
security-relevant fields user-only) lives in `config/`. Persona, actor,
and prompt definitions are markdown with frontmatter, discovered by
`definitions/`. Built-in defaults are embedded and registered at the
lowest precedence. The project layer is trust-gated (TOFU, hash-pinned)
via `config::trust`.
