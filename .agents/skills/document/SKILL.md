---
name: document
description: Writing, restructuring, or reviewing documentation in docs/docs/user/ or docs/docs/dev/.
---

## When to use me

Use me when the task involves writing new docs, restructuring existing ones, or fixing documentation issues. I capture the conventions this project enforces.

## AgentKit component separation

Each component (Litterbox, Switchboard, Lens) has its own directory under `docs/docs/user/<component>/`. Utilities shared across components go under `docs/docs/user/utilities/`.

**Do not** reference one component's internals in another component's docs. For example:
- Credential helpers are a utility — their docs live in `docs/docs/user/utilities/`, not under `switchboard/`.
- The credential JSON format is a Switchboard convention — document it in `docs/docs/dev/switchboard/`, not in the credentials page.

Cross-reference with relative links: `[credential helper](../../utilities/credentials)` from within a component.

## Structure conventions

Each component follows the same layout as Litterbox (`docs/docs/user/litterbox/`):

```
<component>/
  _category_.json          # category label + emoji + position
  index.mdx                # 👋 Introducing <Component>
  setting-up-your-providers.mdx  # 🔧 per-provider config + credentials
  usage.mdx                # 🚀 day-to-day use
  roadmap.md               # 💭 future plans
  agents/
    _category_.json        # 🛠️ Configuring agents
    opencode.mdx           # one per supported agent
    claude-code.mdx
    amazon-kiro.mdx
    zed-agent.mdx
  reference/
    _category_.json        # 📚 Reference
    cli.mdx                # 👩‍💻 CLI reference
    config.mdx             # 🎛️ full TOML schema
```

Only document agents that actually exist. The set of supported agents is defined by the Litterbox `agents/` directory: Amazon Kiro, Claude Code, OpenCode, and Zed Agent.

## Emoji conventions

| Location | Emoji |
|---|---|
| Component intro heading (`index.mdx`) | `👋` |
| Category label for a component | `🔀` (Switchboard), `💩` (Litterbox) |
| Category label for agents | `🛠️` |
| Category label for reference | `📚` |
| Category label for utilities | `🧰` |
| Setup/providers page heading | `🔧` |
| Usage page heading | `🚀` |
| Roadmap page | `💭` |

## Terminology

- Use **cleartext**, not "plaintext", for unencrypted data on disk.
- A credential helper's `get`/`put`/`delete` commands take those exact names (not `store`/`erase`).

## Code over spec

Document the actual implementation, not the ADR spec. Check the source code — ADRs capture intent, but the implementation may differ in details (command names, struct fields, behaviour).

## Sidebar positions

| Section | Position |
|---|---|
| Component index | 1 |
| Setting up providers | 2 |
| Configuring agents (category) | 3 |
| Reference (category) | 50 |
| Roadmap | 100 |
| Utilities (category) | 300 |

Nested pages within a category don't need explicit `sidebar_position` unless ordering matters.

## Doc generation via agentkit-docgen

Each CLI crate should use `agentkit-docgen` to generate CLI and MCP reference docs from the actual code, not hand-write them. The pattern is:

1. Add `agentkit-docgen` as a dependency in `Cargo.toml`.
2. Add a `docgen` subcommand to the CLI enum.
3. Call `agentkit_docgen::generate_cli_docs(&Cli::command())` (and `generate_mcp_docs()` if applicable).
4. The generated output is written to the appropriate `reference/cli.mdx` file.

**Current status:**
- `agentkit-litterbox`: has `docgen cli` subcommand.
- `agentkit-lens`: has `docgen cli` and `docgen mcp` subcommands.
- **`agentkit-switchboard`: missing `agentkit-docgen` dependency and `docgen` subcommand.** This needs to be added.

To regenerate:

```shell
cargo run -p agentkit-litterbox -- docgen cli > docs/docs/user/litterbox/reference/cli.mdx
cargo run -p agentkit-lens -- docgen cli > docs/docs/user/lens/reference/cli.mdx
cargo run -p agentkit-lens -- docgen mcp > docs/docs/user/lens/reference/mcp.mdx
cargo run -p agentkit-switchboard -- --config /dev/null docgen cli > docs/docs/user/switchboard/reference/cli.mdx
```

Note: `agentkit-docgen` only generates one level of subcommands. Nested sub-subcommands (like `auth login`, `auth add`) must be documented manually in the output file. The generated file provides the correct top-level structure and flag definitions; append nested subcommand docs below the generated content.

## Agent configuration docs

For Switchboard, configuring an agent means pointing its OpenAI-compatible endpoint at the proxy — not adding MCP tools. The endpoint is `http://127.0.0.1:3812`. Each agent page should:

1. List prerequisites (proxy running, agent installed).
2. Show the config snippet for pointing at Switchboard (provider config, env var, base URL, etc.).
3. Suggest checking the proxy logs to verify routing.
