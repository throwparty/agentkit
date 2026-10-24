# acp::v1

The ACP v1 RPC surface: transport, request handlers, and the glue that
maps the core's concepts to the wire protocol. `mod.rs` is declarations
and re-exports only; the orchestration lives in `crate::agent`.

## What lives here

```
mod.rs          module doc + re-exports (no logic)
server.rs       run_stdio and the per-connection handler builder
                (run_agent_over): one thin closure per ACP method,
                delegating to the handler modules below
initialize.rs   the initialize and authenticate handlers
sessions.rs     the session-lifecycle handlers (new, list, close,
                delete, load, resume, set_config_option, fork)
prompt.rs       the session/prompt dispatch ladder: the slash-command
                intercepts (/fork, /!, /mcp, /editor, compaction) and
                the model turn
support.rs      shared handler support: the Cx alias, the turn-runtime
                builder, and the message/notice emitters
capabilities.rs UnstableFeature and the capability map
negotiation.rs  ConnectionNegotiation: advertised features, client
                capability surface, editor toggles, tracked terminals,
                the per-connection MCP pool
commands.rs     available-commands advertisement, /mcp and /editor,
                server connection, the first-turn MCP status notice
notices.rs      ConnectionNotices (the script-model-call NoticeSink)
replay.rs       session/load replay (stored messages → notifications)
permission.rs   AcpPermissionRequester (the request_permission bridge)
sink.rs         ConnectionTurnSink (TurnSink → session/update)
client_tools.rs client-capability capture, the catalogue for a
                connection, and the ClientToolExecutor implementation
http.rs         the HTTP transport
tests.rs        capability capture, per-connection state, project consent
```

The core hands this layer a `TurnRuntime` (pool, sink, requester,
executor, tracker, notices, client-tool catalogue); the handlers build it
from the live connection and call into `crate::agent`.

## Dependencies

- depends on: `agent` (the orchestration it drives), `state`, `invokables`,
  `mcp`, `permissions`, `scripts`, `providers`, `agent_client_protocol`
- depended on by: `main` (the binary entry point)
