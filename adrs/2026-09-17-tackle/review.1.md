# Review — spec/plan/tasks consistency, pass 2 (2026-09-18)

Follow-up to `review.0.md`. All 29 of its items were re-checked against the current sources: most are fixed (`messages.content` now stores JSON-serialised ACP content blocks; FR-026 defines `agent.*` invocation; FR-014 states the full pipeline order; `[turns]`/`[sessions]` config keys exist; the CTE uses `first_retained_turn_id`; the profile concept is deleted from the permissions component; milestone dependencies are explicit; NFR-001 is referenced). Items 2, 10, 15, and 21 of review.0 are only partially fixed and reappear below. Renders (`spec.md`, `plan.md`, `tasks.md`) are current with their `.toon` sources; all `adrs/…` cross-references resolve; all task `refs` slugs trace to spec slugs except where noted.

Findings below are the residuals plus new issues introduced by the revisions. The full-compaction CTE claim was verified by executing `schema.sql` against a test chain.

## Blockers

1. **Full compaction is broken in `schema.sql`.** The context-assembly CTE sets `stop_at = COALESCE(first_retained_turn_id, walk.id)` when leaving a compaction turn, but the `WHERE` clause checks the row being expanded — one generation too late. With `first_retained_turn_id = NULL` (full compaction) the walk never terminates at the compaction turn and returns the entire history. Verified by execution: a five-turn chain with a full-compaction head returns all six turns. The shipped `/compact` default (`compact-now.rhai`) uses exactly this path (`record_compaction(summary, ())`).
2. **Summary ordering contradicts FR-018.** The CTE orders strictly by `depth DESC` (oldest first), so the summary message — living in the newest DAG node — lands *after* the retained turns. FR-018 and the schema's own comment say assembly "emits the summary and resumes from `first_retained_turn_id`" (summary first). Needs an assembly-level ordering key or a CTE change; T-007's property tests would certify the wrong order, since they only assert SQL ≡ daggy.
3. **FR-026 (`agent.<actor>`) has no task and no milestone.** It was added to fix review.0 item 4, but it appears in no milestone's FR list and no task's `refs` — it is now the only spec slug no task traces to. Also unspecified: how the nested loop interacts with the model-request cap, recursion depth when an agent invokes `agent.<actor>` (scripts have a depth-one rule; model-invoked agents have none), and whether `agent.*` is user-invokable (what does `/!agent.adrian` do?).
4. **ACP HTTP transport (FR-001) is never implemented.** FR-001 requires stdio *and* HTTP; `axum` and `agent-client-protocol-http` sit in the technologies table and the `acp` component description, but T-009 is stdio-only and no task, milestone exit criterion, or test covers HTTP mode.

## Inconsistencies between documents

5. **Permission-profile leftovers in `plan.toon`** (residual of review.0 item 2): the M7 approach text still says "script permission profiles" and the dataFlow fork paragraph still says the `session_forked` script may "invoke a tool under its declared permission profile" — both contradict the permissions component ("the earlier permission-profile concept is deleted"), FR-014, and `scripts-api.md`.
6. **Direct-invocation syntax is two things at once.** FR-011 and T-024 say `/!name` (and advertise a command named `!`); AC-006, the plan's invokables component and dataFlow, and EC-007/EC-008 say `!name`. The interception rule ("exact first-token match") and the first-session seed docs cannot be written until one is canonical.
7. **Session payload lacks the kind.** FR-016 requires "the session kind in the payload" and the shipped `compaction.rhai` reads `event.session.ephemeral`, but `scripts-api.md` documents the session map as `#{ id, cwd, title, actor, agent }` — no kind field.
8. **Shipped titling script contradicts the re-entrancy rule it exemplifies.** `titling.rhai` has no ephemeral guard (the compaction script does), yet events fire in every session per FR-016 — titling forks recursively inside its own ephemeral forks until budgets trip. It also calls `fork_session(#{ actor: "summariser" })`, but no summariser actor/persona is shipped (builtins: default persona+actor, two scripts, the `/fork` prompt), and behaviour for a missing actor is undefined.
9. **T-005 trust-hashes "scripts, personas, and actors"** — FR-022 also trust-gates prompts (residual of review.0 item 10).
10. **Collision handling is specified three different ways.** FR-009: cross-server name collisions are a configuration error at load. FR-013: duplicate *server* names resolve by precedence, client wins. Plan mcp component: "duplicate-name precedence and rename rules" — the rename rule is defined nowhere. Underlying question unresolved: does the model see namespaced (`mcp.<server>.<tool>`) or bare tool names? If namespaced, FR-009's collision clause is dead text; if bare, renames need defining.
11. **T-012 criterion "ephemeral turns never replayed"** — no such turn kind exists (`interaction|seed|compaction`); ephemerality is a session kind. FR-003 phrases it correctly (residual of review.0 item 15).
12. **Consent is stubbed forever.** T-005 wires the TOFU consent flow "behind a stubbed prompt interface" and no task ever replaces the stub, but FR-022 requires consent "via a permission prompt" and AC-010 requires explicit consent — the AC cannot pass as scheduled.
13. **T-031 implements unspecified behaviour.** configOptions, model discovery, and mid-session model switch with auto-compaction exist only in plan/tasks; no requirement describes them, so its `refs` point at broad slugs that do not cover the behaviour.
14. **`[sessions] include_ephemeral` is in the config schema of record but in no task** — T-011 implements `session/list` without it (FR-017 requires the option).

## Ambiguities to settle before implementation

15. **Grant-store keying is undefined** — by invokable only, or actor+invokable? Quest F22 implies actor+tool with re-evaluation on actor switch; FR-014 and T-022 never say. Affects pipeline lookup and the `previously_granted` flag.
16. **Headless behaviour is tested but never specified.** T-022 tests "headless auto-reject"; no requirement says what the `interactive` indicator does to the ask fallback.
17. **`/compact` mechanics still thin** (review.0 item 21, partially addressed): what, if anything, is stored in the main session for the intercepted prompt; which turn hosts the in-band announcement; what `turn_id` means in the `compaction_requested` payload.
18. **Ephemeral fork lifecycle is undefined.** The "max live ephemeral forks" budget implies forks stop being live at some point; nothing says when (after `await_completion`? GC? never).
19. **`session/close` vs `session/delete`** — both appear to set `active=0`; what distinguishes them, and does `session/list` surface closed sessions?
20. **"Ancestor edits are copy-on-write" (FR-024)** — no requirement, ACP method, or task describes what performs an ancestor edit; `copiedFrom` metadata has no producer.
21. **Truncation policy is "defined" but never defined** — FR-011 defers to "a defined truncation policy" and T-024 tests one, but the threshold and marker are unspecified (same for the MCP tool-result size cap from Scout F13).
22. **NFR-003's "context assembly is bounded by the turn cap"** — the model-request cap bounds requests per turn, not context; DAG depth is unbounded without compaction. The claim as written is unclear.

## Summary

Items 1–4 need resolution before the affected tasks are actionable: 1–2 are code-level bugs in the schema of record (the CTE fails for full compaction and orders the summary last), 3–4 are FRs with no schedule. Items 5–14 are doc fixes with named sources; 15–22 are decisions to record in `spec.toon` or `plan.toon` rather than discover mid-milestone.
