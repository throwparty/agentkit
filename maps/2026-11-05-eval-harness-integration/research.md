# Evaluation frameworks and harness adapters — research

Status: **research complete, decision pending.** Written 2026-11-05. All
popularity figures are a snapshot of that date and are evidence of attention,
not quality. Primary sources are linked; claims that need a spike are marked
**[verify]**.

Scope: external benchmarking tools and evaluation frameworks, and what each one
requires from a harness (Tackle) in order to drive tests through it. This is the
lead-in to an ADR covering the adapters.

Two readings of the brief are assumed and should be corrected if wrong:

- "Sui Bench" = **SWE-bench**.
- "Gittix WeBench" = **get SWE-bench** running.

## 0. What we are trying to learn

There are four different questions, and they do not share a runner or a metric.
Conflating them is the main way an "eval framework" decision goes wrong.

| ID | Question | Unit of analysis | Design | Primary metrics |
| --- | --- | --- | --- | --- |
| **G1** | Does a Tackle prompt improve outcomes? | Prompt version | Paired: same model, tasks, harness, with vs without the prompt (or prompt A vs B) | Paired pass-rate delta, trigger precision/recall, cost and tokens per task, rubric score |
| **G2** | Does the harness perform well? | Harness build + config | Absolute benchmark run, plus ablations against a fixed model and prompt set | Resolved rate, crash/timeout rate, variance across trials, tokens and wall-clock per task, permission violations |
| **G3** | Does the harness obey the protocols it claims? | Harness build | Conformance suite | Pass/fail per requirement (`acp-tck`, MCP conformance) |
| **G4** | Which model works best with Tackle? | Model | Matrix over models, fixed harness and prompts | G1/G2 metrics grouped by model |

### Absolute vs paired

A public leaderboard score measures **model + harness + prompts + scaffold**
together. It cannot tell you which of those moved. "Harness" here means
everything that is not the model: the turn loop, tool exposure, prompt
assembly, context management and compaction, permission pipeline, MCP client,
and editor surface. To attribute a change:

- **G2 capability** is absolute: run SWE-bench or Terminal-Bench and report the
  resolved rate. Useful for "is the harness good enough", useless for "did this
  change help".
- **G1 prompt efficacy** must be paired: the same task, model, and harness, with
  one variable changed. Report the delta and its variance, not the absolute
  score.
- **G2 harness contribution** is an ablation: hold model and prompts constant,
  vary one harness feature (compaction, MCP, editor tools, permission mode,
  prompt set) or compare two harness builds. This is the only way to claim a
  harness change caused a difference.

Pin the model snapshot for the duration of a comparison. A model upgrade
invalidates every baseline.

### Non-goals

- Ranking Tackle on a public leaderboard.
- Training or RL rollouts.
- Replacing unit, integration, transcript, or `acp-tck` tests.
- Judging a prompt without a task and a verifier.

## 1. Taxonomy

The market mixes four kinds of thing. Only the first drives a harness.

| Kind | What it owns | Examples | What it needs from Tackle |
| --- | --- | --- | --- |
| **Runner** | Launch, sandbox, model routing, trajectory, scoring loop | Harbor, BenchFlow, Inspect AI, Evals.si, Promptfoo (weakly) | An agent integration: ACP entry, CLI, or SDK hook |
| **Benchmark** | Tasks, environments, verifiers, datasets | SWE-bench, Terminal-Bench, MCPMark, SkillsBench | Nothing directly; it runs through a runner |
| **Tracker / scorer** | Traces, experiments, dashboards, judges | Braintrust, LangSmith, Langfuse, Phoenix, Opik, Weave, MLflow, DeepEval | Telemetry (OTel) or an SDK hook, not an agent adapter |
| **Conformance kit** | Requirement-level protocol tests | `acp-tck`, MCP conformance | An ACP/MCP server; already exists |

A benchmark without a runner is a dataset. A tracker without a runner is a
dashboard. Neither is an integration target for Tackle.

## 2. The harness contract

Every runner, whatever its vocabulary, asks the same questions of the harness.
This is the checklist to fill in per framework.

| ID | Requirement | Why it matters |
| --- | --- | --- |
| H1 | **Launch and packaging** | Can the runner install/start the harness in its environment (binary, npx, uvx, OCI, source)? |
| H2 | **Wire protocol** | ACP stdio, JSONL events, HTTP, or an in-process SDK |
| H3 | **Workspace root** | Does the harness accept a per-run cwd from the runner? |
| H4 | **File access** | Read/write through the runner (ACP `fs/*`) or directly in the sandbox |
| H5 | **Command execution** | Terminal/exec surface: create, output, wait, kill, release |
| H6 | **Model routing** | Endpoint base URL, key, model selection, optional proxy/gateway |
| H7 | **Prompt injection** | Task instruction, prompt/persona files, system prompt, parameters |
| H8 | **Permission policy** | How tool approvals are answered; auto-approve, deny, scripted |
| H9 | **Trajectory** | Streaming events with tool calls, usage, cost, stable IDs |
| H10 | **Cancellation and timeouts** | `session/cancel`, process kill, per-task deadline |
| H11 | **Scoring hook** | Workspace-visible artefacts, verifier script, reward/score |
| H12 | **Isolation and state** | Per-run HOME/config/db, network policy, no cross-run state |
| H13 | **Concurrency** | Multiple instances without state or port collisions |
| H14 | **Result contract** | Machine-readable results, stable schema, artefacts on disk |

Tackle's current position against this checklist:

| Requirement | Tackle today | Consequence |
| --- | --- | --- |
| H1 | Rust binary; no ACP registry entry; no headless subcommand | Needs a distribution entry and a launch wrapper |
| H2 | ACP v1 over stdio and HTTP; no JSONL/headless mode | Native fit for ACP runners; shim for the rest |
| H3 | Uses `session/new` `cwd` (`src/acp/v1/sessions.rs`) | Works |
| H4 | Client tools `client.fs.*`; runner must advertise | Works with runners that implement ACP fs |
| H5 | Client tools `client.terminal.*`; runner must implement | Works with runners that implement ACP terminal |
| H6 | OpenAI-compatible endpoints in config; switchboard supported | Wrapper must translate runner env into config |
| H7 | `.agentkit/tackle/prompts/` with parameters and `model_invokable`; personas/actors | Runner must stage files or the wrapper must copy them |
| H8 | `session/request_permission`; grant store and policy scripts | Runner must answer; auto-approve for most evals |
| H9 | `session/update` tool calls, `usage_update` with cost; SQLite session store | Good; prompt invocation observability needs a check |
| H10 | `session/cancel`; process kill | Works |
| H11 | Editor tools and MCP act on the workspace; no built-in verifier | Runner's verifier is the scoring hook |
| H12 | `--config-dir` and `--db-path` per process; sessions in platform data dir | Wrapper must isolate HOME/config/db per run |
| H13 | One process per ACP connection; SQLite per db path | Parallel-safe if isolated |
| H14 | ACP events + `sessions.db`; no standard result bundle | Runner owns the result contract |

## 3. Framework-by-framework

### 3.1 Harbor

Harbor is the framework from the Terminal-Bench team. It has three agent
shapes: installed CLI agents (`BaseInstalledAgent`), external agents whose loop
runs in Harbor (`BaseAgent`), and generic **ACP agents** from the ACP Registry.
The ACP route is the relevant one: Harbor installs the agent inside the task
container, launches it over stdio, implements the ACP client (fs, terminal,
permissions), and records the exchange.

**Contract**

| Requirement | Harbor |
| --- | --- |
| Launch | `acp:<id>` from the registry, or `acp` + `registry_entry_path` for a local `agent.json`; installed in the task container |
| Protocol | ACP v1 JSON-RPC over stdio |
| Workspace | Task container cwd; ACP `fs/*` and `terminal/*` mapped to the container |
| Model | `-m provider/model`; ACP agents get `HARBOR_ACP_REQUESTED_MODEL`; provider credentials are **not** passed automatically, use `--agent-env` |
| Prompts | `instruction.md`; skills are per-agent and not generic to ACP |
| Permissions | `permission_mode=allow|deny` (ACP agent kwarg) |
| Trajectory | `acp.txt`, `acp-events.jsonl`, `acp-summary.json`, ATIF `trajectory.json` |
| Scoring | `tests/test.sh` writes `/logs/verifier/reward.txt` or `reward.json` |
| Isolation | Docker, Modal, Daytona, Kubernetes; task environments are self-contained |
| Packaging | ACP registry distribution: binary per target with `archive`, `sha256`, `cmd`, `args`, `env`; or npx/uvx |

**Tackle gaps**

- No ACP registry entry. The registry schema is a good fit: a binary target per
  platform with a `cmd` that can be the wrapper.
- Tackle's config is TOML, not env. The wrapper must write the endpoint,
  defaults, MCP servers, and prompt files from the runner's env.
- Provider credentials must be forwarded explicitly (`--agent-env`); the ACP
  runner does not proxy model calls.
- The task container's glibc may be older than the build host; ship a
  compatible or static binary.
- Prompt injection is not a Harbor concept. The wrapper or task setup must copy
  `.agentkit/tackle/prompts/` into the workspace.
- Per-run state: use `--config-dir` and `--db-path` under the workspace or a
  per-run temp dir.

**Fit:** best overall for G2. ACP-native, datasets for SWE-bench Verified/Pro,
Terminal-Bench, SWE-rebench; active; Apache-2.0.

### 3.2 BenchFlow

BenchFlow is a universal environment framework and the harness that runs
SkillsBench 1.1. Its unit is a task directory plus a sandbox; its agent unit is
an ACP-over-stdio process. It has explicit **with-skill / no-skill** modes,
multi-turn scenes, a LiteLLM provider gateway, and artefact-rich rollouts.

**Contract**

| Requirement | BenchFlow |
| --- | --- |
| Launch | Agent manifest with `install_cmd` and `launch_cmd`, run in the sandbox; `protocol=acp`; ACP Registry tier as well |
| Protocol | ACP stdio; `trajectory/acp_trajectory.jsonl`; gateway trace in `trajectory/llm_trajectory.jsonl` |
| Workspace | Sandbox cwd; ACP client provides fs/terminal **[verify]** |
| Model | `-m provider/model`; LiteLLM gateway; `BENCHFLOW_PROVIDER_BASE_URL`, `BENCHFLOW_PROVIDER_API_KEY`, `LLM_BASE_URL`, `LLM_API_KEY`; manifest `env_mapping`, `acp_model_format`, `supports_acp_set_model` |
| Prompts | Task instruction; `--skill-mode with-skill|no-skill`; `--skills-dir`; manifest `skill_paths`, `home_dirs` |
| Permissions | ACP permission requests; approval policy |
| Trajectory | `acp_trajectory.jsonl`, `llm_trajectory.jsonl`, `result.json` |
| Scoring | Task verifier (`/verifier` or `tests/`), reward in `rewards.reward` |
| Isolation | Docker, Daytona, Modal |
| Packaging | `install_cmd` + `launch_cmd`; anything needing config-file emission belongs in the shim, not the manifest |

**Tackle gaps**

- The manifest is deliberately data-only: config-file emission, provider
  translation, and prompt staging must live in a shim.
- `skill_paths` expects Agent Skills directories, not Tackle prompts. A
  translation from `SKILL.md` to `.agentkit/tackle/prompts/*.md` is needed, or a
  plugin package. For **Tackle prompts**, the cleaner mapping is to stage
  `.agentkit/tackle/prompts/` directly and use the with/without arm to add or
  remove it; `skill_paths` is the wrong abstraction.
- Prompt-invocation counting: BenchFlow derives `n_skill_invocations` from
  `tool_call` events whose `kind` is `skill`. Tackle's model-invokable prompts
  need to surface a recognisable kind/name for that count, or analysis must read
  the tool name from the trajectory **[verify]**.
- Same binary/glibc and state-isolation concerns as Harbor.

**Fit:** best for G1. The with/without condition is built in, the gateway
handles provider routing, and the task format is Harbor-compatible. Lower
adoption than Harbor; treat it as a bet with a fallback.

### 3.3 Inspect AI

Inspect is the UK AI Security Institute / Meridian Labs framework. Its unit is
a Python `Task` (dataset + solver + scorer); it has sandboxes, a large eval
catalogue, a viewer, and human-intervention support. Its ACP support is a
**server** for attaching a human operator to a running eval, not a client for
driving an external ACP agent.

**Contract**

| Requirement | Inspect |
| --- | --- |
| Launch | Python `@agent` factory; `sandbox().exec()` to run a CLI; `sandbox_agent_bridge()` for CLI agents that speak OpenAI/Anthropic/Google APIs |
| Protocol | In-process Python agent; the bridge proxies model calls on `localhost:13131`; ACP server for intervention |
| Workspace | Sandbox (`docker`, `k8s`, `modal`, ...); `sandbox().exec`, read/write |
| Model | Inspect model provider; bridge sets `OPENAI_BASE_URL`/`ANTHROPIC_BASE_URL`/`GOOGLE_GEMINI_BASE_URL` for the sandboxed agent |
| Prompts | Dataset sample / task prompt; agent-specific |
| Permissions | Tool approval policies; agent-specific inside the sandbox |
| Trajectory | Inspect transcript; a custom agent must record ACP events itself |
| Scoring | Python scorers, `Score`, `Target`; sandbox state available to scorers |
| Isolation | Docker/K8s/Modal/Proxmox/Vagrant and extensions |
| Packaging | Dockerfile or install command in the sandbox |

**Tackle gaps**

- No ACP client. A custom Inspect agent must implement `initialize`,
  `session/new`, `session/prompt`, `session/update`, `session/cancel`,
  `session/request_permission`, `fs/*`, and `terminal/*`, or reuse a small ACP
  client library.
- The model bridge is useful: configure Tackle's endpoint base URL to
  `http://localhost:13131/v1` so Inspect owns model routing and cost tracking.
- Trajectory mapping: ACP `session/update` to Inspect events is adapter code.
- More work than Harbor, but the strongest custom-scoring and human-in-the-loop
  story.

**Fit:** use when the scorer is the hard part, or when we need intervention,
custom Python metrics, or Inspect's existing eval catalogue.

### 3.4 Evals.si

Evals.si is an eval service with a native ACP target. Its ACP connector runs a
command in the task sandbox, acts as the client, and implements `initialize`,
`session/new`, `session/prompt`, `session/update`, `session/request_permission`,
`fs/read_text_file`, `fs/write_text_file`, and optionally `terminal/*`.

**Contract**

| Requirement | Evals.si |
| --- | --- |
| Launch | `acp: {command, install, env_from, allow_hosts, timeout, approval, terminals}` inside the sandbox |
| Protocol | ACP v1 over stdio |
| Workspace | Task sandbox cwd |
| Model | `env_from` passes credentials; no evidence of a model proxy for ACP targets **[verify]** |
| Prompts | Task instruction |
| Permissions | `approval` policy |
| Trajectory | ACP updates mapped to steps; permission events |
| Scoring | Environment checker plus evaluators; pass@k / pass^k |
| Isolation | Firecracker, bwrap, Landlock, or Kubernetes |
| Packaging | `install` command |

**Tackle gaps:** wrapper for config and credentials; verify protocol coverage
(no auth methods, `session/load`, or modes), sandbox networking, and maturity.
The integration surface is the smallest of any runner because the ACP target is
first-class.

**Fit:** watch. If it is production-grade, it may be the cheapest path to G1 and
G2; if not, it is a distraction.

### 3.5 Promptfoo

Promptfoo is a declarative eval and red-team tool. Its unit is a provider
(`callApi(prompt, context)`) and assertions; it has no sandbox, no workspace,
and no native multi-turn agent loop.

**Contract**

| Requirement | Promptfoo |
| --- | --- |
| Launch | Custom JS/Python provider that spawns a process |
| Protocol | In-process function call |
| Workspace | None; the provider must create one |
| Model | The provider or the harness owns it |
| Prompts | Prompt string |
| Permissions | The wrapper must answer |
| Trajectory | Provider `metadata`; no standard |
| Scoring | Assertions, LLM rubrics, custom graders |
| Isolation | None |
| Packaging | N/A |

**Tackle gaps:** the wrapper would have to do everything (workspace, sandbox,
ACP client, model config, cleanup), at which point Promptfoo is only a
spreadsheet and assertion engine.

**Fit:** useful for fast prompt-text A/B and red-team assertions, not for
harness-level eval. Do not make it a primary adapter.

### 3.6 SWE-bench (follow-on)

SWE-bench is a **benchmark**, not a runner. The official contract is minimal and
separate from the agent:

1. Run the agent on a repository checked out at the instance's base commit,
   with the issue text as the prompt.
2. Capture the resulting patch (usually `git diff`).
3. Write predictions JSONL: `instance_id`, `model_name_or_path`, `model_patch`.
4. Run `swebench.harness.run_evaluation` with the dataset and predictions.

The evaluation harness builds per-instance Docker images, applies the patch, runs
the repository tests, and reports resolved/unresolved/error. Resource
requirements are significant: ~120 GB disk, 16 GB RAM, 8+ cores; SWE-bench Lite
takes ~30–50 minutes with `env` caching. It caches by `run_id` + `instance_id`
and does **not** consider patch content, so every new patch needs a new
`run_id`.

**What Tackle needs:** a per-instance runner that clones the repo at the base
commit, runs Tackle with the issue as the instruction, captures the diff, and
writes predictions. This is a separate adapter from the ACP runners: no ACP
client is involved, and the official harness owns scoring. It measures G2
capability end-to-end, not harness isolation.

**Cheaper path:** Harbor already publishes SWE-bench Verified and SWE-bench Pro
datasets as Harbor tasks, so the same Tackle ACP adapter can run them without a
separate predictions pipeline. The official harness remains the cross-check.

**Recommendation:** a separate ADR after the ACP adapter lands. Start with
SWE-bench Verified on a small instance subset, then scale.

### 3.7 MCP benchmarks (follow-on)

Tackle's outside world is MCP. Two benchmarks are closer to its design than
SWE-bench:

- **MCP-Universe** (Salesforce, Apache-2.0): six domains, eleven MCP servers;
  now also evaluates MCPMark.
- **MCPMark** (Apache-2.0): 127 realistic multi-step MCP tasks, isolated
  sandboxes, deterministic verifiers.

Neither is an ACP runner today; both have their own agent harness. They are
candidates for a later adapter if MCP capability matters more than repo editing.
Worth noting because a harness that is "MCP-only for the outside world" is
exactly what these measure.

### 3.8 Vivaria

METR's Vivaria is a serious eval platform with the METR Task Standard, but METR
is **transitioning to Inspect** and ramping down new feature development. Do not
adopt for new work. Recorded here because it appears in comparisons and would
otherwise be re-researched.

### 3.9 Trackers and scorers

Braintrust, LangSmith, Langfuse, Arize Phoenix, Opik, W&B Weave, MLflow,
DeepEval, Pydantic Evals, and Google ADK Evaluation are **not runners**. They
record traces, run judges, and compare experiments. They integrate through an
SDK or OTel, not an agent adapter.

For Tackle, the useful hook is its existing OTel telemetry plus a result exporter.
If we later want a hosted experiment UI, pick one tracker and feed it the
runner's artefacts; do not let it become the harness integration.

### 3.10 Niche and emerging

- **ACPX**: a headless ACP client with persistent sessions and crash recovery,
  used by BenchFlow and referenced by Harbor's simulated-user trials. A useful
  reference implementation for our own shim, not a framework.
- **AgentV**: eval framework with an `evals.json` adapter and provider model;
  no evidence of a native ACP target **[verify]**.
- **`@ai-sdk/harness-acp`**: a generic ACP harness adapter for the Vercel AI
  SDK. A library for building clients, not a benchmark runner.
- **`acp-tck`**: conformance only; already integrated under
  `adrs/2026-10-24-tackle-acp-tck`.
- **`lm-evaluation-harness` and OpenAI Evals**: model-level; no agent harness.

## 4. Viability criteria

Stars are a lagging indicator and easy to game. Use hard gates first, then a
weighted score for the survivors.

### 4.1 Hard gates

A tool is only a candidate if all of these hold:

| Gate | Test |
| --- | --- |
| **HG1 Drives a harness** | Can launch and drive an external agent, not just call a model |
| **HG2 Workspace control** | Sandbox/workspace with file and command access, or a documented way to provide it |
| **HG3 Open and self-hostable** | Permissive licence (Apache-2.0/MIT); no mandatory SaaS for core runs |
| **HG4 Active** | Commit within 90 days and a release within 6 months |
| **HG5 Extensible without forking** | Agent/plugin API or protocol; upstream accepts adapters |
| **HG6 Machine-readable output** | Stable JSON/JSONL results and artefacts |
| **HG7 Deterministic scoring hook** | A verifier or scorer that can run on the workspace/artefacts |

### 4.2 Weighted score

| Criterion | Weight | What it measures |
| --- | --- | --- |
| Adoption and governance | 20% | Stars, forks, contributors, organisational backing, benchmark ownership |
| Harness fit | 25% | ACP, workspace, permissions, multi-turn, model routing |
| Benchmark ecosystem | 15% | SWE-bench, Terminal-Bench, MCP, Terminal-style datasets |
| Operability | 15% | Local/offline, Docker, concurrency, resumability, trace quality |
| Extensibility | 10% | Plugin/agent API, docs, upstream responsiveness |
| Maintenance risk | 10% | Release cadence, bus factor, deprecation signals |
| Cost | 5% | OSS, compute footprint, hosted dependency |

Scores are 1–5; weighted score out of 5. These are assessments, not
measurements.

### 4.3 Scorecard

| Tool | HG | Adoption | Fit | Ecosystem | Operability | Extensibility | Risk | Cost | Weighted | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **Harbor** | pass | 5 | 5 | 5 | 5 | 4 | 4 | 4 | **4.75** | Primary G2 |
| **BenchFlow** | pass | 3 | 5 | 4 | 4 | 4 | 3 | 5 | **4.00** | Primary G1 |
| **Inspect AI** | pass | 4 | 3 | 5 | 4 | 5 | 5 | 5 | **4.15** | Secondary |
| **Evals.si** | pass | 1 | 5 | 3 | 3 | 3 | 2 | 4 | **3.05** | Watch |
| **Promptfoo** | fail (HG2) | 5 | 1 | 2 | 3 | 3 | 5 | 5 | — | Complementary |
| **Vivaria** | pass | 2 | 4 | 2 | 3 | 3 | 1 | 4 | **2.75** | Do not adopt |
| **SWE-bench harness** | n/a (benchmark) | 4 | 5 | 5 | 2 | 3 | 5 | 2 | — | Follow-on ADR |
| **MCP-Universe / MCPMark** | n/a (benchmark) | 3 | 3 | 3 | 3 | 3 | 3 | 4 | — | Follow-on |

### 4.4 Popularity snapshot (2026-11-05)

| Repository | Stars | Forks | Open issues | Last push | Licence |
| --- | --- | --- | --- | --- | --- |
| harbor-framework/harbor | 6,821 | 2,142 | 1,208 | 2026-11-05 | Apache-2.0 |
| benchflow-ai/benchflow | 359 | 73 | 148 | 2026-10-30 | Apache-2.0 |
| UKGovernmentBEIS/inspect_ai | 3,183 | 896 | 492 | 2026-11-05 | MIT |
| promptfoo/promptfoo | 26,870 | 2,658 | 923 | 2026-11-05 | MIT |
| METR/vivaria | 144 | 41 | 227 | 2026-10-07 | MIT |
| SWE-bench/SWE-bench | 6,149 | 1,047 | 62 | 2026-09-18 | MIT |
| harbor-framework/terminal-bench | 1,013 | 617 | 489 | 2026-11-04 | Apache-2.0 |
| SalesforceAIResearch/MCP-Universe | 604 | 90 | 36 | 2026-10-08 | Apache-2.0 |
| eval-sys/MCPMark | 472 | 50 | 27 | 2026-06-12 | Apache-2.0 |
| agentclientprotocol/agent-client-protocol | 4,580 | 473 | 35 | 2026-11-04 | Apache-2.0 |
| agentclientprotocol/registry | 447 | 384 | 275 | 2026-11-05 | Apache-2.0 |

Caveats:

- Promptfoo and the trackers have high stars but fail the harness gates; stars
  alone would mis-rank them.
- BenchFlow's low star count understates its relevance: it is the SkillsBench
  1.1 harness and has first-class ACP, with/without conditions, and a LiteLLM
  gateway.
- Harbor's high open-issue count reflects activity and a large surface, not
  necessarily quality.
- Star counts move; treat this as a dated snapshot.

## 5. Recommendation

### 5.1 Runners

1. **Harbor for G2 (harness performance).** ACP-native, broad sandbox support,
   SWE-bench Verified/Pro and Terminal-Bench datasets, ATIF trajectories, active,
   Apache-2.0. The adapter is an ACP registry entry plus a launch wrapper.
2. **BenchFlow for G1 (prompt efficacy).** The with/without condition is native,
   the gateway handles model routing, the task format is Harbor-compatible, and
   the trajectory records tool calls and usage. Lower adoption, so keep Harbor as
   the fallback if BenchFlow's ACP client has gaps.
3. **Inspect as the scoring escape hatch.** Use it when the hard part is a custom
   Python scorer, human intervention, or an existing Inspect eval. Do not start
   here; the ACP client is ours to write.
4. **Evals.si as a watch item.** If its ACP target is production-grade, it is the
   cheapest integration. Verify maturity before committing.
5. **Do not adopt Vivaria.** It is transitioning to Inspect.
6. **Do not use Promptfoo as a harness runner.** It is a prompt/A-B and
   red-team tool. Use it only if the question is about prompt text in isolation.

### 5.2 What to build

One adapter, several manifests. The adapter is a **Tackle ACP launcher**:

- Reads provider/model env from the runner (`BENCHFLOW_PROVIDER_*`,
  `LLM_*`, `OPENAI_*`, `ANTHROPIC_*`, `TACKLE_MODEL`).
- Writes Tackle config: endpoint base URL and key, default model, MCP servers,
  scripts, compaction, permission policy.
- Stages `.agentkit/tackle/prompts/` into the workspace (with-prompt arm) or
  omits it (without-prompt arm).
- Sets `--config-dir` and `--db-path` under the run workspace.
- `exec`s `tackle stdio`.

Then publish:

- An **ACP registry `agent.json`** with binary targets for linux x86_64 and
  aarch64 (and darwin for local runs), `sha256`, and `cmd` pointing at the
  launcher. This single artefact unlocks Harbor, BenchFlow, Evals.si, and any
  other ACP client.
- A **BenchFlow manifest** (`contract_version`, `install_cmd`, `launch_cmd`,
  `protocol=acp`, `env_mapping`, `acp_model_format`, `default_model`,
  `requires_env`) for the BenchFlow-specific paths.
- A **prompt staging convention**: `.agentkit/tackle/prompts/<name>/` or
  `<name>.md`, invoked as `/name args` for user-invokable prompts, or left
  `model_invokable` for discovery. The with/without arm is the presence or
  absence of that directory.

What not to build: a runner, a sandbox, a dataset registry, a result UI, or a
judge. Those are the parts the frameworks already own.

### 5.3 Experiment protocol (applies to both goals)

- Fix the model snapshot, the task set, and the harness build.
- Run each cell at least 3 times; report mean and variance.
- Pair every prompt variant with a no-prompt baseline and, where possible, a
  previous prompt version.
- Score with a deterministic verifier first; use a judge only for qualities the
  verifier cannot see.
- Record tokens, cost, wall-clock, tool calls, and prompt invocations per task.
- Keep the raw trajectory and workspace diff. A score without a trace is not
  debuggable.
- Baseline-diff in CI: new failures are errors; newly passing baseline entries
  are drift to update.

### 5.4 Prompt efficacy specifics

Tackle prompts have two delivery modes, and they need different cases.

| Mode | Frontmatter | How the case invokes it | What to measure |
| --- | --- | --- | --- |
| **Command** | `user_invokable: true` | The case prompt is `/name args`; Tackle intercepts the first token and expands the body | Output quality vs a no-prompt baseline; parameter handling; failure on missing or extra arguments |
| **Discovery** | `model_invokable: true` | The case prompt is the task; the prompt is exposed as a tool and the model decides | Trigger precision and recall; outcome delta when it triggers; behaviour on irrelevant tasks where it should not trigger |

A prompt package can carry resources (per the definitions ADR), so the eval
workspace must contain the whole package under `.agentkit/tackle/prompts/`,
not just the entry markdown. The with-prompt arm stages it; the without-prompt
arm omits it. Prompt A vs prompt B is two directories and the same task set.

For command prompts, a "without" arm is not the same task minus the command: it
is the same user intent expressed without the prompt, so the baseline measures
the prompt's value over the model's unaided attempt. For discovery prompts, the
negative set matters as much as the positive set: a prompt that fires on every
task inflates its own apparent value and pollutes unrelated work.

## 6. Open questions and evidence gaps

| # | Question | Why it matters | How to resolve |
| --- | --- | --- | --- |
| Q1 | Does BenchFlow's ACP client advertise and implement `fs/*` and `terminal/*`? | Tackle's editor tools are client-provided | Run a Tackle smoke task in BenchFlow |
| Q2 | Does BenchFlow route ACP-agent model calls through LiteLLM, and under which env names? | The wrapper must emit the right endpoint | Inspect a rollout's `llm_trajectory.jsonl` and env |
| Q3 | Does Harbor's generic ACP agent expose a model base URL/key, or only `HARBOR_ACP_REQUESTED_MODEL`? | Tackle needs an endpoint, not just a name | Read the ACP agent source; smoke test |
| Q4 | Does Tackle emit a distinguishable `tool_call` kind/name for `model_invokable` prompts? | Prompt invocation counting and trigger precision | Inspect `acp-events.jsonl` for a prompt tool call |
| Q5 | Does Tackle support ACP `session/set_model` / model switching? | BenchFlow's `supports_acp_set_model` | Check schema and ACP v1 implementation |
| Q6 | Can a single Tackle binary run in the task containers (glibc, arch)? | ACP registry packaging | Build static/musl or pin a compatible target |
| Q7 | Do Harbor's SWE-bench datasets match the official harness's instance set and scoring? | Comparability with published results | Run a small overlap set through both |
| Q8 | Is Evals.si production-grade and self-hostable? | Potential cheapest integration | Trial account or source review |

## 7. Follow-on ADRs

| ADR | Scope | Depends on |
| --- | --- | --- |
| **A. Eval harness adapters** | ACP registry entry, launcher, prompt staging, Harbor and BenchFlow manifests, result contract | This research |
| **B. SWE-bench bring-up** | Dataset choice (Verified vs Pro), Harbor dataset vs official harness, image cache, CI budget, baseline | A |
| **C. MCP capability benchmarks** | MCP-Universe/MCPMark adapter, if MCP capability is a priority | A |
| **D. Prompt-efficacy protocol** | Case format, with/without arms, trigger precision, statistical reporting | A (can fold into A) |

ADR A should be written first. B and C are deliberately separate: they are
benchmarks with their own resource envelopes and scoring, not runner adapters.

## 8. Sources

Primary:

- Harbor: <https://docs.harborframework.com/agents/acp>, `.../agents/custom-agents`,
  `.../tasks`, `.../datasets`; ACP registry PR #1464.
- BenchFlow: <https://docs.benchflow.ai/running-benchmarks>,
  <https://docs.benchflow.ai/task-authoring>, `docs/external-agents.md`,
  `benchflow-ai/agents` manifest schema.
- Inspect: <https://inspect.aisi.org.uk/agents.html>, `.../agent-bridge.html`,
  `.../sandboxing.html`, `.../intervention.html`.
- Evals.si: <https://evalsi.dev/docs/guides/agent-runs/>.
- Promptfoo: <https://www.promptfoo.dev/docs/providers/custom-api/>.
- SWE-bench: <https://www.swebench.com/SWE-bench/reference/harness/>,
  `.../guides/evaluation/`.
- ACP registry schema: `agentclientprotocol/registry/agent.schema.json`.
- Vivaria: <https://github.com/METR/vivaria>.

Secondary (used for context, not load-bearing):

- SkillsBench 1.1 (BenchFlow harness), `skillsbench.ai`, arXiv 2602.12670.
- MCP-Universe and MCPMark repositories and papers.
- Terminal-Bench 2.0 and Harbor Hub dataset listings.
