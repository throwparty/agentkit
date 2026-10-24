# mcp

The MCP client pool: talking to configured and client-provided MCP
servers, and knowing what tools they offer.

## What lives here

```
mod.rs     module doc + re-exports
pool.rs    McpPool and its connections; tool namespacing
           (mcp.<server>.<tool>), per-server status, elicitation
           forwarding, tool results, connection timeout
spawn.rs   stdio spawn hygiene: explicit argv (no shell), only named env
           entries, and {cred:IDENTITY} resolution at spawn
stderr.rs  child-stderr relay: line-prefixed with the server name, under
           a rate cap
tests.rs   spawn hygiene, credential resolution, and the stderr relay
```

This module is the protocol client only. The unified catalogue of
invokables (MCP tools alongside prompts, agents, and client tools) lives
in `invokables`, which consumes `NamespacedTool` and calls
`McpPool::call_tool`.

## Dependencies

- depends on: `config` (server declarations), `credentials` (helper
  resolution at spawn), `rmcp`
- depended on by: `invokables` (tool catalogue and dispatch), `acp::v1`
  (server status and elicitation forwarding), `cli::auth`
  (`{cred:IDENTITY}` discovery)
