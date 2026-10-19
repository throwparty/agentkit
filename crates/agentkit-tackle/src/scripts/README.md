# scripts

The Rhai script host: the hardened engine builders and the acting host
API. The Rhai machinery itself lives in `agentkit-script`; this module
fixes the tackle-specific policy.

## What lives here

```
mod.rs      module doc + re-exports
engine.rs   the two engine builders — policy_engine (decision inputs
            only) and behaviour_engine (full host API) — with the fixed
            safety limits, plus PolicyScripts (the policy source the
            permission pipeline calls)
host/       the acting host API (see host/README.md)
```

Policy engines physically lack the acting host functions; behaviour
engines get the full API. Every `ScriptError` is handled fail-closed by
the caller (the pipeline treats error or timeout as ask).

## Dependencies

- depends on: `agentkit-script` (the Rhai engine), `store` (host calls),
  `permissions` (PolicyScripts implements PolicyScript)
- depended on by: `acp_v1` (behaviour orchestration), `permissions`
  (the policy source)
