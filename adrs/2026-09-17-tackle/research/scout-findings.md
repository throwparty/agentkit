# Scout findings — security review (2026-09-17)

Review of the tackle design prior to specification. Findings ordered by severity; folded into `spec.toon` requirements.

1. **CRITICAL — Project hooks trusted on load.** Hooks are executable code with pipeline authority; cloning a repo containing `.agentkit/tackle/hooks/*.rhai` and opening it in a client is immediate harness compromise. The `.litterbox.toml` precedent does not transfer (litterbox config provisions contained sandboxes; tackle hooks run unsandboxed with pipeline authority). → Explicit trust gating for project hooks, personas, and actors: first-use consent, TOFU hash pinning keyed by repo remote + file hash, re-consent on hash change; hooks load once at process start, never hot-reloaded.
2. **HIGH — Project-overridable model endpoint and credential source.** A cloned repo could point `base_url` at an attacker server (credential exfiltration) or name a sensitive env var. → Endpoint, wire format, and credential source restricted to user config; never forwarded into MCP server environments.
3. **HIGH — Grant store write path conflated with MCP elicitations.** Elicitation `allow_always` responses must never write grants; elicitations surfaced with explicit origin attribution, distinct from tool permission prompts.
4. **HIGH — `hook_initiated` calls consuming session grants.** Behaviour hooks must not inherit the user's `allow_always` grants; they run under a hook-declared pre-granted permission profile and never prompt.
5. **MEDIUM — Grant store before scripts short-circuits policy.** Scripts receive a `previously_granted` flag and act as a veto layer; deny records remain authoritative.
6. **MEDIUM — Rhai hardening.** Operation/call-level/expression-depth/string-size limits, time limits (~1s policy, ~10s behaviour), script size cap, no modules, fresh engine per invocation, argument payload caps; script error/timeout ⇒ fail-closed (`ask`), surfaced.
7. **MEDIUM — stdio hygiene.** Rhai `on_print`/`on_debug` redirected to stderr (never stdout — JSON-RPC corruption); `catch_unwind` on host functions; `history_search` row/byte caps.
8. **MEDIUM — `!` bypass trigger scope.** Recognised only on the user-typed input path; never parsed from model output, seeds, hook payloads, or `send_prompt` content; no host function replicates direct invocation; invocations audited.
9. **MEDIUM — Compaction as context laundering.** Immutable pinned prefix (system prompt + standing instructions) that compaction can never elide; summaries marked untrusted; records attributed to the producing script.
10. **MEDIUM — Cost abuse budgets.** Max live ephemeral forks per session, max `send_prompt` per hook/turn/session, enforcing cost cap at the parent, `await_completion` timeout, no nested hook-driven prompts beyond depth 1, hook-driven model calls surfaced in the client.
11. **MEDIUM — Seed messages as injection channel.** Seeds are harness-authored static content; dynamic content enters as untrusted user/tool-role content with delimiters; authority level of every history kind documented.
12. **MEDIUM — Session DB exposure.** DB stored outside the repo (user data dir), `0600`; `history_search` DAG-scoped with caps.
13. **LOW — Residual hardening.** Structural engine separation (policy engines lack act functions by construction); credential/argument redaction in logs; MCP tool result size caps; bounded frontmatter parsing.

**Verdict**: sound to specify once trust gating, endpoint/credential restriction, grant/elicitation separation, hook-scoped pre-grants, and fail-closed Rhai limits are applied.
