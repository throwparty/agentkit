# Harness prior-art review

Research toward tackle (2026-09-17). Sources: official docs and repos, retrieved 2026-09-17.

## Pi (earendil-works/pi, formerly badlogic/pi-mono)

"Minimal terminal coding harness... small at the core while being extended through TypeScript extensions, skills, prompt templates, themes, and pi packages." (pi.dev/docs/latest)

- **Permissions: none built in.** "Pi does not include a built-in permission system for restricting filesystem, process, network, or credential access. By default, it runs with the permissions of the user and process that launched it." Boundaries come from containerization (Gondolin micro-VM extension, plain Docker, OpenShell) — which assumes Pi itself runs *inside* the confined environment. Tackle rejects this school: an agent with shell access inside its environment could mutate its own configuration and access control. Permission assessment and enforcement are core harness functions and must live outside the development environment.
- **Sessions** (`~/.pi/agent/sessions/--<path>--/<timestamp>_<session-id>.jsonl`): JSONL, one file per session, tree structure — every entry carries `id`/`parentId`; the **leaf** is the current position. Version 3 format.
- **Entry types**: `session` header (metadata, not in tree; optional `parentSession` for forks), `message` (roles: `system` with prompt sections + tool loadout patches, `user`, `assistant` with usage + stopReason, `toolResult`, `bashExecution`, `custom`), `model_change`, `thinking_level_change`, `compaction` (summary + `firstKeptEntryId` + full system checkpoint), `branch_summary`, `custom` (extension state, NOT in LLM context), `custom_message` (extension-injected, IN LLM context), `label`, `session_info`.
- **Branching**: `/tree` navigates and branches in place (leaf moves to an earlier entry; editing a user message and resubmitting creates a new branch). `/fork` starts a new session file from an earlier user message; `/clone` duplicates the active branch into a new file. When switching away from a branch, Pi attaches a `branch_summary` entry — an LLM-generated summary of the abandoned path, with `fromId` pointing at the previous leaf.
- **Prompt templates** (= slash commands): Markdown + frontmatter (`description`, `argument-hint`); global `~/.pi/agent/prompts/*.md`, project `.pi/prompts/*.md` (trust-gated); filename becomes the command (`review.md` → `/review`); positional args `$1`, `$@`, `${1:-default}`.
- **Skills**: Agent Skills standard (SKILL.md directories).
- **Extensions**: TypeScript modules registering tools, commands, event handlers (e.g. `before_agent_start`), custom TUI renderers; inject context via `custom_message` entries.

## Codex CLI (openai/codex)

- **Config**: `~/.codex/config.toml` (user) + `.codex/config.toml` (project, trust-gated; project files cannot override provider/auth/notification/telemetry keys). Profiles as separate `$CODEX_HOME/<name>.config.toml` files selected with `--profile`.
- **Approvals**: `approval_policy = on-request | never | { granular = { sandbox_approval, rules, mcp_elicitations, request_permissions, skill_approval } }`. `untrusted` and `on-failure` retired.
- **Permission profiles**: `default_permissions` with built-ins `:read-only`, `:workspace`, `:danger-full-access` plus custom `[permissions.<name>]` tables; network proxy with domain allow/deny for sandboxed commands.
- **Hooks** (`hooks.json` or inline `[hooks]`): events `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PermissionRequest`, `PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`, `Stop`, `Interrupt`. Handlers are **commands or MCP tools**; hook output can be injected as model-visible `additionalContext` (with a token threshold for oversized output); async option for background hooks.
- **Multi-agent**: `agents.<name>` roles with `config_file` + `description`; `spawn_agent`/`send_input`/`resume_agent`/`wait_agent`/`close_agent` tools.
- **Apps/connectors**: per-tool `approval_mode = auto | prompt | writes | approve`, `destructive_hint` / `open_world_hint` gating.
- **Sessions**: rollout files; `history.persistence = save-all | none` into `history.jsonl`.

## OpenCode

- **Agents**: `~/.config/opencode/agent/*.md` (plus project `.opencode/agent/`): Markdown body = system prompt; frontmatter `description`, `mode` (primary/subagent), `temperature`, `tools` (glob → bool), `permission` (tool → glob → `allow|ask|deny`). Verified locally against `adrian.md`.
- **Commands**: Markdown prompt files in `~/.config/opencode/command/` (with helper `.sh` scripts alongside).
- **Skills**: `SKILL.md` directories under `~/.config/opencode/skills/`.
- **Sessions**: SQLite (`opencode.db` + WAL/SHM) plus `storage/` and `snapshot/` directories under `~/.local/share/opencode/`.

## ACP session/fork RFD (agentclientprotocol.com/rfds/session-fork, josevalim)

- New `session/fork` method; agent declares `session: { fork: {} }` capability; request takes `sessionId` + same options as `session/load` (`cwd`, `mcpServers`); response returns the new `sessionId`.
- Future extension: optional `messageId` to fork at a specific message ("checkpoints").
- Motivating use case: side-branch work (summaries, PR descriptions) without polluting the parent history; potentially subagents.
- **No inserted-message or hook concept in the RFD** — that is tackle's extension.

## Goose recipes (block/goose)

Goose's recipe model is the richest "reusable prompt" design in the ecosystem (goose-docs.ai/docs/guides/recipes):

- **Format**: YAML (recommended) or JSON; a recipe bundles `title`, `description`, `instructions` and/or `prompt`, `parameters` (declared, substituted via `{{ name }}` templating), `extensions` (MCP servers the recipe may use — an explicit allowlist), and `settings` (`goose_provider`, `goose_model`, `temperature`, `max_turns`).
- **Template inheritance**: `{% extends "parent.yaml" %}` with `{% block %}` overrides (Jinja-style), enabling prompt families.
- **Invocation**: `goose run --recipe file.yaml --params k=v`; custom slash commands can launch recipes; Desktop shows clickable "activity" bubbles; recipes can be shared as files/URLs/GitHub repos.
- **Structured output**: recipes can enforce JSON output for automation/CI consumption.
- **Extension secrets**: recipes declare required env keys; goose resolves from environment or secret storage.

Relevance to tackle: a recipe is essentially a *reusable prompt plus its execution configuration* — parameters, allowed tools, model settings. This supports folding skills/commands into a single reusable-prompt mechanic with declared parameters and per-invokable configuration, rather than maintaining skills and commands as distinct concepts.

## Session interchange formats (researched 2026-09-18)

No standard exists for DAG-structured session history interchange. The nearest:

- **ACP replay stream** — `session/load`'s `session/update` notification sequence is a protocol-native session representation; recorded transcripts double as conformance fixtures.
- **OpenTelemetry GenAI semantic conventions** — stable attributes in v1.37+; span hierarchy `invoke_agent` → `chat`/`execute_tool`; `gen_ai.conversation.id`/`gen_ai.session.id` correlation; opt-in content capture (`gen_ai.input.messages`, `gen_ai.output.messages`, `gen_ai.content.tool_call` events) enables replay and investigation in Langfuse, Arize Phoenix, Datadog, OpenLLMetry. Emitted by Codex, Claude Code, VS Code Copilot.
- **OpenInference** (Arize) — competing LLM tracing convention, compared with OTel GenAI.
- **AAIF** (IETF draft-schemacommons-aaif-00, June 2026) — vendor-neutral agent *definition* interchange (identity, instructions, tool catalogue incl. MCP, telemetry, provenance/signature) plus a companion agent-state checkpoint schema for pause/resume and migration. A draft; prior art for definition portability, not a dependency.

Tackle's coverage: ACP replay natively, OTel GenAI per FR-025, and a content-addressed definition store in the schema for definition provenance and time travel.

## Synthesis

1. **Tree-structured session entries with `id`/`parentId` + leaf pointer** is the proven pattern (Pi v2+). Pi branches at entry (message) granularity; the storage ADR (2026-06-13) branches at prompt-turn granularity. Both satisfy "one node, many children".
2. **Inserted messages on branch exist in the wild**: Pi's `branch_summary` (auto-generated context from the abandoned path) and `custom_message` (extension-injected, context-participating). Tackle's fork hook generalizes both: a hook (MCP tool call) whose output is inserted as a context-participating message.
3. **Three schools of permissions**: OpenCode (per-agent glob rules, allow/ask/deny), Codex (central profiles + granular approvals + hooks), Pi (nothing — sandbox instead, with the harness inside the sandbox). Tackle combines OpenCode's declaration shape with **in-core enforcement**: the harness assesses permissions and enforces tool access control itself, deliberately isolated from the sandboxed development environment where the agent holds shell access.
4. **Commands/skills/prompt-templates converge**: all are Markdown + frontmatter, discovered from project + global dirs, filename = slash command, with argument interpolation. The distinction is delivery (user-invoked expansion vs agent-discovered on-demand loading), not substance — supporting the unified-invokable direction.
5. **Hooks**: Codex's event list is the industry baseline, and its support for MCP-tool handlers validates tackle's "fork hook calls an MCP tool" design. No mainstream harness hooks session fork — Pi's branch summaries are the nearest neighbour; this is a differentiator, not table stakes.
6. **Config discovery**: user + project layers with project trust gating (Codex, Pi) — matches the planned `~/.config/agentkit/tackle/` + `./.agentkit/tackle/` layout via `agentkit-path`.
