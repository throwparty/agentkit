# permissions

The fixed-order permission pipeline: the only path to tool execution
aside from user direct invocation.

## What lives here

```
mod.rs       module doc + re-exports
policy.rs    the policy-script source (PolicyScript) and its payload
             (PolicyRequest, Decision, PolicyVerdict)
request.rs   the ask fallback: PermissionRequester, PermissionRequest,
             PermissionChoice, and the presented options
grants.rs    GrantStore — the in-memory, session-scoped standing
             allow/deny records; never persisted
pipeline.rs  PermissionPipeline (the fixed order), Assessment, Verdict,
             and ConsentViaPermission (the trust-consent bridge)
tests.rs     order, veto, fail-closed, headless, and timeout
```

Order: grant deny → policy script → grant allow → fixed ask fallback.
The ask fallback is not configurable and never resolves to allow. There
is no harness-side timeout and no headless auto-reject.

## Dependencies

- depends on: `config::trust` (the consent prompt the pipeline bridges)
- depended on by: `acp::v1` (the requester over the client connection),
  `scripts` (the policy host implements `PolicyScript`)
