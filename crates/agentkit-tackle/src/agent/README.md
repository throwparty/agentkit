# agent

The agent loop: running one prompt turn, nested agent invocation, and
retry. The model seam itself lives in [`crate::providers`].

## What lives here

```
mod.rs      the loop's outcome types: TurnStop, TurnOutcome
turn.rs     run_turn — one prompt → streamed model requests → persisted
            assistant messages, until the model finishes or the cap trips
nested.rs   agent.<actor> invocation: a nested turn loop in an ephemeral
            fork, depth-limited to one
retry.rs    retry policy and provider-error classification
```

## Dependencies

- depends on: `providers` (the model trait and types), `store` (turns,
  messages, usage)
- depended on by: `acp_v1` (prompt-turn orchestration), `scripts`
  (script-driven model calls)
