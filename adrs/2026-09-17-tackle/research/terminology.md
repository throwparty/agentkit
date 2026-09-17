# Terminology survey: prompt turn vs message

Research toward tackle (2026-09-17) and the CONTEXT.md definitions it must produce.

## Survey

- **Anthropic Messages API** — defines a turn explicitly: "each 'turn' of a conversation consists of a single message from the user, followed by a single message from the assistant." Messages carry typed content blocks (`text`, `tool_use`, `tool_result`, ...); `tool_use` blocks live in assistant messages, `tool_result` blocks in the *next user message*. Stateless — full history resent per request.
- **OpenAI** — Chat Completions: role-typed `messages` (system/user/assistant/tool); assistant tool calls are a field on the assistant message; tool results are separate `role: "tool"` messages. Responses API: history is **items** (`message`, `function_call`, `function_call_output`, `reasoning`, ...). "Turn" is used loosely for a request/response round-trip, not formalized.
- **Gemini** — unit is `Content { role: user|model, parts: [...] }`; `functionCall`/`functionResponse` are *parts inside* Content, not separate messages; `systemInstruction` sits outside the contents list.
- **ACP** (agentclientprotocol.com) — no formal Turn object in the schema. "Prompt turns" are emergent: one `session/prompt` request → stream of `session/update` notifications (`user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`) → final `stopReason` (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`). `max_turn_requests` implies one prompt turn may contain multiple model requests.
- **LangGraph** — `thread` = conversation identifier; `checkpoint` = state snapshot per super-step; forking = resuming from a prior checkpoint id.
- **Pi** — every session entry (message *or* metadata: model change, compaction, label...) is a tree node with `id`/`parentId`; the leaf is the current position; any entry can be a branch point.
- **Storage ADR** (2026-06-13-acp-server-session-storage) — prompt turn = DAG node (single `parent_id`); messages linear within a turn; fork point = a prompt turn; interior-message splitting deferred.

## Where usage collides

- "Turn" means: one user→assistant exchange (Anthropic), one API round-trip (OpenAI loose usage), or one client-prompt cycle possibly spanning several model requests (ACP `max_turn_requests`).
- "Message" means: a role-typed content unit (all providers), or a streaming fragment of one (ACP `*_message_chunk`).

## Candidates for tackle

**A. Turn-level DAG (storage ADR as written).** Prompt turn = DAG node grouping one prompt cycle's messages; message = role-attributed unit within a turn. Forks at turn granularity. Matches ACP's prompt-turn cycle, Anthropic's turn definition, and the existing storage schema. Diverges from tackle's original phrasing ("any message can be the root") only in granularity.

**B. Message-level tree (Pi style).** Every message is a DAG node; "prompt turn" is a derived grouping of contiguous messages from one prompt cycle. Forks at any message. Most flexible; requires reworking the draft storage schema and context assembly.

**C. Turn DAG + message-resolution fork (storage ADR fork semantics + RFD `messageId`).** Same schema as A; a fork's optional `messageId` resolves to its containing turn. Interior splits stay a future extension.

## Recommendation

**A/C (same schema).** Normative definitions for CONTEXT.md:

> **Message** — a single role-attributed unit of conversation content (user, assistant, tool call, tool result, system/seed). The smallest stored unit of a session; each message belongs to exactly one prompt turn.
>
> **Prompt turn** — everything the agent does in response to one client prompt: an ordered, linear sequence of one or more messages, beginning with the user message and ending with the assistant's final message. The unit of persistence, context assembly, and forking. Prompt turns form a DAG: each turn has at most one parent, and a turn may have any number of children.
