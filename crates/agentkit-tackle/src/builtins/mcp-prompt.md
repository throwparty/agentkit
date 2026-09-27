---
description: Manage MCP server availability — enable, disable, or report status
parameters:
  - subcommand
  - server
---
Manages which configured MCP servers are available for the rest of the connection. Subcommands: `enable <server>`, `disable <server>`, `status [server]`. Disabling closes the connection and drops the server's tools; enabling reconnects it. This command is intercepted by the harness and never becomes a model turn.
