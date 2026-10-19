# invokables

The unified invokable registry: every dispatchable operation, namespaced
and permission-gated, behind one dispatch point.

## What lives here

```
mod.rs          module doc + re-exports
name.rs         namespacing: mcp_name / prompt_name / agent_name and
                kind_of (the one place the prefix convention is decoded)
invokable.rs    the Invokable enum and its visibility flags
registry.rs     Registry — building from definitions, MCP tools, and
                client tools; prompt expansion; the model-tool list, the
                system-prompt manifest, and available_commands
direct.rs       /!<invokable> parsing for user-typed input
client_tools.rs the client.* tool descriptors (what exists), not their
                execution (that is the ACP glue layer)
titles.rs       human-readable, argument-aware titles and tool kinds for
                tool calls across all namespaces
tests.rs        dispatch tables, expansion, interception, truncation
```

An invokable is `mcp.<server>.<tool>`, `prompt.<name>`, `agent.<actor>`,
or `client.<surface>.<method>`. Visibility (`user_invokable`,
`model_invokable`) is a flag on the invokable, not a type distinction.
The registry is the only dispatch point; the ACP command surface and the
model's tool exposure are views over it.

## Dependencies

- depends on: `definitions` (prompts, actors), `mcp` (MCP tool
  descriptors and dispatch), `providers` (the tool-definition type),
  `agent_client_protocol` (command wire types)
- depended on by: `acp_v1` (dispatch, titles, client-tool catalogue)
