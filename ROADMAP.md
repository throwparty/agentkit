# AgentKit roadmap

Some future ideas I'd like to flesh out. As components come into existence, their roadmap items will move to their directories.

## Bridgehead

The UI for managing agents.

## Sling

An agentic coding client, which connects to a harness as an ACP client.

## Stethoscope

Language Server Protocol insight.

## Switchboard

A model provider proxy, which forwards requests to upstream providers based on quota and pricing information and model configurations.

## Tackle

An agentic coding agent harness. This component would provide only an ACP server for interaction, and would provide only an MCP client for integration with external systems. Filesystem access would be used only to provide loading of agents (different actors or personas, each with their own prompt and permissions), commands (larger scaffolds for prompts which are exposed to the user via `/commands`) skills (small behaviours that are exposed to the agent for automatic discovery).

- Sessions stored as a graph of messages, where session root messages have no parent, but all other messages have parents.
- One message might be parent to many children, allowing for forking conversations.
- Any message can be the root of a session.
- Branching sessions runs a hook which allows an agent message (terminology?!) to be inserted, allowing sandboxing tools like Litterbox to fork the sandbox.
