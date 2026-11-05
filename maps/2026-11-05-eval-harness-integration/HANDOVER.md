# Handover: `eval-harness-integration`

The map has moved from landscape research to delivery. Decision: **Harbor is
the single runner**. It has native SWE-bench tasks and ACP; skill lift is a
paired-job script. BenchFlow's `bench skills eval` is better UX, but its
SWE-bench path is an adoption and parity project, so it is not the single stack.

The adapter is wrapper-only: a `tackle-acp` launcher plus an ACP registry entry.
No Tackle product change is required.

## Frontier (next probes)

| Ticket | Question |
| --- | --- |
| **T-005** | How does a Tackle prompt package map to a Harbor skill directory or to a staged project prompt? |
| **T-006** | How do we observe prompt invocation, tool calls, tokens and cost in Harbor's trajectory and results? |
| **T-008** | How do we isolate untrusted prompt packages and workspaces, and who approves project trust? |
| **T-015** | Does a 5-instance SWE-bench Verified overlap agree between Harbor and the official harness? |
| **T-016** | Does the launcher work with Harbor's generic ACP agent, including `skills_dir`, trust approval and model routing? |
| **T-017** | Which instance IDs, model snapshot and provider define the first baseline? |

Blocked behind them: T-007 (result schema), T-011 (CI policy), T-012 (authoring
workflow), T-013 (MVP acceptance).

## What is already settled

- **T-001:** BenchFlow has no native SWE-bench adapter. Harbor-format tasks
  import, but parity is unproven and adoption is parity-gated.
- **T-002:** Harbor can run paired with/without jobs and a small script can
  compute lift. There is no native lift report.
- **T-003:** Tackle is ACP-native; a wrapper-only launcher works. Prompts must
  be staged in the project tree; trust is a runner permission concern; state is
  isolated with `--config-dir` and `--db-path`.
- **T-009:** goreleaser already builds the target matrix; ship the launcher in
  the archive and publish an ACP registry entry.
- **T-010:** start with a 25-instance SWE-bench Verified subset at 3 trials.
- **T-014:** the test patch is carried; the official pass rule is unproven.

## Assumptions

- "Sui Bench" was read as **SWE-bench**; "Gittix WeBench" as **get SWE-bench**.
- "Skills" here means a reusable Tackle prompt package; `.agents/skills` is out
  of scope.
- Stars are a dated snapshot (2026-11-05) and a weak signal.

## Next

Probe the frontier. Then write **ADR A — eval harness adapters** for the
launcher, registry entry, prompt staging, Harbor job configs, and result schema.
**ADR B — SWE-bench bring-up** follows. **ADR C — MCP capability benchmarks** is
optional.
