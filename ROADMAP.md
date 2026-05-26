# AgentKit roadmap

Some future ideas I'd like to flesh out. As components come into existence, their roadmap items will move to their directories.

## Sling

An agentic coding client, which connects to a harness as an ACP client.

## Stethoscope

Language Server Protocol insight.

## Tackle

An agentic coding agent harness. This component would provide only an ACP server for interaction, and would provide only an MCP client for integration with external systems. Filesystem access would be used only to provide loading of agents (different actors or personas, each with their own prompt and permissions), commands (larger scaffolds for prompts which are exposed to the user via `/commands`) skills (small behaviours that are exposed to the agent for automatic discovery).
