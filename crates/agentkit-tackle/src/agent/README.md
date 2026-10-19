# agent

The agent loop and the core orchestration: running a prompt turn, direct
invocation, tool-call execution, nested agents, fork, compaction, seed,
and titling. The model seam itself lives in [`crate::providers`]; the RPC
surface that drives this lives in [`crate::acp_v1`].

## What lives here

```
mod.rs          the loop's outcome types: TurnStop, TurnOutcome
runtime.rs      TurnRuntime — the connection-provided collaborators a
                turn runs against (pool, sink, requester, executor,
                tracker, notice sinks, client-tool catalogue)
sink.rs         TurnSink — the loop's outbound event interface
turn.rs         run_turn — one prompt → streamed model requests →
                persisted assistant messages, until the model finishes or
                the cap trips
prompt.rs       run_prompt_turn — resolve actor/model/tools/context, run
                the loop, report usage/compaction/titling, release lease
direct.rs       run_direct_invocation — a user-typed /!<tool> {json}
tool_call.rs    run_model_tool_call: announce, permission-gate, execute,
                report; run_agent_invocation; policy_script_sources
nested.rs       agent.<actor> invocation: a nested turn loop in an
                ephemeral fork, depth-limited to one
fork.rs         session/fork orchestration and fork_access
compaction.rs   compaction turns and the post-turn firing
titling.rs      the title_trigger firing (session titling)
seed.rs         first-run seed and missing-credential checks
script_loop.rs  SessionAccess over the store with the model dispatcher
config_options.rs  session config options (model/actor selectors)
retry.rs        retry policy and provider-error classification
```

## Dependencies

- depends on: `providers`, `store`, `invokables`, `permissions`, `scripts`,
  `mcp`, `state`, `content`
- depended on by: `acp_v1` (the RPC surface drives it)
