# scripts::host

The behaviour host API: the acting surface behaviour scripts get and
policy engines physically lack.

## What lives here

```
mod.rs        module doc + re-exports
api.rs        the host traits and payloads: SessionAccess,
              PromptDispatcher, CompletionSink, NoticeSink, HostError,
              HistoryRow, Completion, ContextUsage, block_on
budgets.rs    the fixed internal budgets (forks, prompts, cost cap,
              history caps), never user configuration
access.rs     StoreAccess — the SessionAccess implementation over the
              store (fork, prompt, history search, compaction recording,
              context usage)
behaviour.rs  BehaviourHost and register_host_api — the Rhai host
              functions and the budget enforcement they perform
tests.rs      budget enforcement, host-function behaviour, payload caps
```

Every host function goes through `SessionAccess`. Host functions are
synchronous (rhai 1.26 has no async) and block on the store through the
ambient runtime.

## Dependencies

- depends on: `store` (the session engine), `agentkit-script`
- depended on by: `scripts::engine` (registration), `acp::v1`
  (dispatching script-driven model calls to the client)
