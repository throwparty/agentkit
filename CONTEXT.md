# AgentKit Context

Normative terminology and conventions for AgentKit components. Tackle (see `adrs/2026-09-17-tackle/`) is the first component to adopt this vocabulary; other components should align when touched.

## Terminology

- **Message** — a member of a conversation submitted by a participant with a given role. Tool calls, tool results, user messages, and model messages are all messages. The smallest stored unit of a session.
- **Turn** — an *interaction turn* is the message sequence beginning with a user message and ending before the next user message: everything the agent does in response to one client prompt. The unit of persistence, context assembly, and forking. Turns form a DAG: each turn has at most one parent, and a turn may have any number of children.
- **Harness turn** — a turn authored by the harness rather than an interaction: **seed turns** and **compaction turns**. Harness turns are never revert points.
- **Revert point** — a user-authored message, or the position before it. Users revert to something they sent or a revision of it, and retry the turn that followed it. Non-user messages are never revert points.
- **Session** — a named view onto the turn DAG with a head pointer; the unit of client interaction.
- **Persona** — a reusable, complete behavioural prompt. The actor's system prompt is the persona body verbatim plus harness-injected sections (skills manifest, invokable listing, environment context), which are not persona content.
- **Actor** — a persona reference plus model selection. A configuration, not a running thing. Actors carry no permission declarations: the permission pipeline (grant store, policy scripts, fixed ask fallback) is global, not per-actor.
- **Agent** — an actor executing within a session; a running instance. Display name `<actor>-<instance>`. Multiple agents of the same persona and actor may run concurrently; agents have no identity across invocations.
- **Invokable** — a named, permission-gated operation addressable by the model or the user. Namespaced by source mechanism: `mcp.<server>.<tool>`, `prompt.<name>`, `agent.<actor>`. Appearance contexts (`user_invokable`, `model_invokable`) are flags on the invokable, not type distinctions.
- **Fork** — a new session sharing ancestor turns with an existing one. No turns are copied.
- **Seed message** — a harness-authored system message inserted into a forked session (e.g. the output of a `session_forked` script). Static, user-facing, never revert points.
- **Ephemeral session** — a fork marked `kind: ephemeral`, used for harness-driven work (summarisation, titling). Never a revert point; excluded from session listings by default; usage attributed to its parent session.
- **Compaction turn** — a harness turn holding a summary message and a first-retained-turn reference. Context assembly stops the parent-chain walk at it: the model receives the minimized view, the DAG retains the full history. Reversible by deletion.
- **Grant store** — session-scoped memory of standing permission decisions. Written only by user `allow_always`/`reject_always` responses; never exposed to scripts.
- **Policy script** — a Rhai script registered at a decision point (e.g. `pre_tool_use`); returns a verdict, cannot act.
- **Behaviour script** — a Rhai script registered at a workflow event (e.g. `session_forked`, `post_turn`); acts via the host API, does not gate, and never invokes tools directly — it drives the model via `send_prompt`, whose tool calls pass through the normal permission pipeline.

## Conventions

- Project configuration lives at `./.agentkit/<component>/config.toml`; user configuration at `~/.config/agentkit/<component>/` (platform-adjusted via `agentkit-path`). Project configuration overrides user configuration except where security-relevant fields are restricted to user configuration.
- Litterbox will migrate from `.litterbox.toml` to `.agentkit/litterbox/config.toml` for consistency.
- Component state (databases, durable files) lives under the platform data directory via `agentkit-path`'s `data_dir`, never inside the repository.
