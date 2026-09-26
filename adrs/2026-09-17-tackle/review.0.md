# Review — spec/plan/tasks consistency (2026-09-18)

Review of `spec.toon`, `plan.toon`, `tasks.toon`, and the supporting artifacts (`schema.sql`, `config.example.toml`, `scripts-api.md`, `architecture.mmd`, `research/`) for cross-document inconsistencies, ambiguities, and implementation blockers.

Renders are current (`spec.md`, `tasks.md` match their `.toon` sources); all `adrs/…` cross-references resolve; all task `refs` slugs trace to spec slugs.

## Blockers

1. **`schema.sql` migration will fail.** Line 70: `CREATE INDEX idx_turns_session ON turns(session_id, position)` — the `turns` table has no `position` column (the storage ADR's `prompt_turns` had one; the rename dropped it but kept the index).
2. **Script permission profiles are vestigial but load-bearing.** `plan.toon` permissions component (and Scout F4, Quest F21) says script-initiated invocations "run under the permission profile declared alongside the script registration" and "never prompt". But the final design says scripts can't invoke tools at all — `scripts-api.md` and `config.example.toml` are explicit that script-driven tool calls go through the normal pipeline (the provision-sandbox example even expects the user to answer prompts). No permission-profile field exists in `config.example.toml`, no requirement specifies one, and no task implements it. Resolve before M7/M8: either delete the profile concept from the plan or specify it.
3. **Image content can't be persisted.** FR-023 accepts image blocks, FR-003 replays "original content", but `messages.content` is text-only with no content-type or media column. Images are silently unstoreable, so replay can't be faithful.
4. **`agent.*` invokables have no defined behaviour.** FR-009 names the namespace; the grants table, scripts-api `kind` enum, and registry tasks all carry it — but no requirement says what invoking an agent does, and sub-agents are a declared non-goal. Unimplementable as written.

## Inconsistencies between documents

5. **Host-function lists disagree.** FR-016 lists `history_search` as a behaviour host function; `scripts-api.md` exposes it to *all* scripts (and FR-015 gives it to policy scripts). FR-016 also omits `insert_seed`, `context_usage`, and `log`, which `scripts-api.md` and T-026 include.
6. **`pre_tool_use` payload mismatch.** FR-015 requires an "interactive indicator" in the payload; `scripts-api.md`'s payload (`invokable, kind, arguments, previously_granted, turn, session, actor`) has no such field.
7. **Pipeline order ambiguity.** FR-014 reads "grant store lookup → policy script → ask", implying grants short-circuit scripts. The plan is more precise (deny records → script veto → allow honoured → ask). FR-014 should state the plan's order, otherwise a script veto over an existing allow grant is ambiguous.
8. **`first_retained_turn_id` in assembly.** T-007 says the CTE walks "with first_retained_turn_id resume"; the schema's CTE and FR-018 stop the walk at the compaction turn and never use the field. Its assembly role (if any) is undefined.
9. **Configurable things with no config keys.** `config.example.toml` is "the schema of record" (T-003), yet FR-005's model-request cap (Quest F9 explicitly wants it as a visible config option) and FR-017's include-ephemeral-in-`session/list` option appear nowhere in it.
10. **Trust-gating scope.** FR-022 and T-005 gate project scripts, personas, and actors — prompts are excluded with no rationale, though a project prompt is the same injection channel. Relatedly, T-004 discovers "personas, actors, and prompts" but not scripts, so script discovery/loading has no task home.
11. **Attribution placement.** FR-024 says *messages* carry actor/agent-instance attribution in metadata; `schema.sql` puts it on `turns.metadata`.
12. **Cost source.** FR-007: cost from "agentkit-models pricing or endpoint-reported values"; T-016 only implements the former.
13. **Process model.** The cli component says "one process per client connection by design"; deployment says HTTP mode serves multiple clients in one process. Both can't be the invariant.
14. **"FK-enforced acyclicity"** (plan storage) — a self-referential FK doesn't enforce acyclicity; the schema comment's "FK + pre-existing parent" (application discipline) is the accurate claim.
15. **"Ephemeral turns"** (FR-003, T-012) — no such turn kind exists (`interaction|seed|compaction`); ephemerality is a session kind. CONTEXT.md defines harness turns as seed/compaction; the spec's phrasing diverges from its own terminology.
16. **rmcp version.** Plan pins 3.3; the workspace (litterbox) is on 3.1.2. Fine if an upgrade is intended, but unstated.
17. **Grants cleanup.** `schema.sql` says grant "rows die with the session", but delete is soft, there's no `ON DELETE CASCADE`, and no task schedules cleanup.

## Ambiguities to settle before implementation

18. **`await_completion` vs engine time limit.** Examples block up to 120s; behaviour engines enforce ~10s via `on_progress`. Either the limit kills every legitimate compaction run or it doesn't tick during host calls (making it porous). The interaction needs defining.
19. **Do behaviour events fire inside script-driven/ephemeral sessions?** The compaction `post_turn` script would re-trigger inside its own ephemeral fork until the budget refuses it (EC-011). Self-limiting but noisy; suppression rules are unstated.
20. **MCP duplicate "rename rule"** (FR-013) — renamed to what, and what does the model see? Unspecified anywhere.
21. **`/compact` mechanics** — ordering of prompt expansion, the `compaction_requested` event, and compaction-turn insertion; what's stored in the turn vs the ephemeral fork.
22. **Fork titling at creation** (FR-019, T-027) — mechanism unspecified (title_trigger? parent title copy?).
23. **EC-009 for v1 clients** — "the user sees a notice", but notices are an unstable capability; the stable fallback for this specific notice is unstated.
24. **EC-003 "naming the enabling configuration"** — compaction is enabled by script registration, not a config key; nothing concrete to name.
25. **Post-replay usage snapshot** (FR-003/FR-007) — usage isn't persisted in any table; where the snapshot comes from is undefined.
26. **Two in-memory structures** — the `SessionStore` in-memory backend and the `daggy::Dag` mirror coexist in the plan; their relationship (same thing?) is unclear.
27. **Disabling a built-in script** — `[scripts.compaction]` with `enabled = false` and no `file`? The config comment implies it; the schema doesn't define it.
28. **Milestone independence vs dependencies.** T-015's tool-call tests need the registry (T-021) and pipeline (T-022), which are M6/M7; T-027's `/fork` fallback needs T-021/T-023; T-028's `/compact` needs T-023. Either add the dependency edges or state the stub strategy that keeps milestones independently testable.
29. **NFR-001 (thin-core)** is the only spec slug no task references.

## Summary

Items 1–4 need resolution before tasks are actionable; 5–17 are doc fixes; 18–29 are decisions to record in the spec or plan rather than discover mid-milestone.
