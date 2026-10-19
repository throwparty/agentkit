# config

Layered configuration: a user layer and a project layer, merged with
project precedence, plus the TOFU trust records that gate the project
layer.

## What lives here

```
mod.rs     module doc + re-exports
schema.rs  the typed configuration: Config, Loaded, ConfigError, the
           endpoint/wire-format/auth types, mcp_servers, scripts,
           defaults, and the session/turn/client-tools settings
load.rs    layered loading and merging: precedence, trust gating,
           rejection of project-declared security fields, and
           script-source resolution
trust.rs   TOFU trust records (trust.toml): per-project, per-path,
           SHA-256 pinned, human-editable for revocation
tests.rs   merge precedence, rejection paths, trust lifecycle
```

Security-relevant fields (model endpoints, the credential helper) are
user-configuration only; a project layer declaring them is rejected with
a named error. Project `mcp_servers` entries are accepted but
trust-gated.

## Dependencies

- depends on: `trust` (self-contained), `toml`, `serde`
- depended on by: `main` (startup), `providers` (endpoints/auth), `mcp`
  (server declarations), `definitions` (trust hashing), `acp_v1` (session
  config options)
