# providers

The model provider abstraction: the trait the agent loop talks to, the
model-seam types that cross it, and the per-wire-format implementations.

## What lives here

```
mod.rs                       ModelProvider trait + model-seam types:
                             ChatRole, ChatMessage, ModelToolCall,
                             ToolDefinition, ToolCallOutcome, ModelRequest,
                             ModelUsage, ModelResponse, ModelError
rig.rs                       shared rig-core plumbing: tackle ⇄ rig
                             message and tool-call conversions
openai_chat_completions.rs   the OpenAI chat-completions wire format:
                             RigProvider, provider_for (endpoint-qualified
                             model → provider), streaming, tests
```

v1 implements one wire format. A second format adds a sibling module and a
`WireFormat` variant; `provider_for` selects between them.

## Dependencies

- depends on: `config` (endpoints, auth), `credentials` (helper resolution),
  `rig-core`
- depended on by: `agent` (the loop), `invokables` (tool definitions),
  `acp_v1` (turn orchestration)
