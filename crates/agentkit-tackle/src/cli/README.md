# cli

The command-line surface: transport selection, path overrides, and the
`tackle auth` subcommand.

## What lives here

```
mod.rs    the clap `Cli`/`Commands` definitions, transport subcommands,
          and the config/data-dir path overrides
auth.rs   `tackle auth` — store, inspect, and remove the bearer tokens
          resolved through the credential helper
```

Tackle is a server launched by ACP clients; the CLI only selects a
transport (`stdio`/`http`), overrides paths, and handles the offline
`auth` and `docgen` commands. Session interaction is client-driven over
ACP, never CLI-driven.

## Dependencies

- depends on: `config` (loaded settings), `credentials` (helper name and
  resolution), `mcp` (`{cred:IDENTITY}` marker discovery)
- depended on by: `main` (argument parsing and dispatch)
