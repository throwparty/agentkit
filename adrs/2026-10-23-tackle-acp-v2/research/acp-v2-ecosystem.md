# ACP v2 ecosystem: current state

Reference resource for [../spec.toon](../spec.toon). Records which clients and agent
harnesses speak which protocol version today, how each handles the ACP v2 removal of the
client filesystem and terminal surface, and what has changed in the prior-art harnesses
since the original review.

**Last verified: 2026-10-23.** Protocol versions move; treat every row as a dated claim.
Evidence is primary wherever possible (source, `package.json` pins, traffic logs, PR/issue
numbers with dates). Where a claim rests on a vendor doc rather than code, it is marked
**[doc]**; where it could not be established, **[unverified]**.

## Method

Two planes, because ACP is a two-sided protocol and they fail differently:

- **Clients** (editors/IDEs) *consume* tackle's ACP server. Their negotiated version and
  advertised capabilities decide which half of tackle they can even talk to.
- **Agent harnesses** *are* tackle's peers. Their v2 implementations are prior art, and
  their advertised capabilities are what clients test against.

Load-bearing fact up front: **the stable ACP protocol version is still 1.** v2 has been a
Draft since 2026-07-20, its schema is still `v2.0.0-alpha.x`, and the protocol authors'
guidance is that "v1-only peers will remain common for some time" and that v2 must not ship
by default.

## Summary — clients

| Client | Protocol | v2 status | Client fs/terminal today |
|---|---|---|---|
| Zed | 1 / 2 | v2 behind `acp-beta`, v1 fallback | Advertised in v1; forwards project MCP servers |
| VS Code (ACP Client) | 1 | None | `fs` + `terminal` handlers advertised |
| Neovim (CodeCompanion, avante, agentic, hermes) | 1 | None | `fs` present; terminal varies |
| Emacs (`agent-shell.el`) | 1 | None | `fs` only |

## Summary — agent harnesses

| Agent | Protocol | v2 status |
|---|---|---|
| Codex CLI (`codex-acp`) | 1 / 2 | **Dual v1 + unstable v2, negotiated per connection** |
| Goose | 1 | v1 only, deliberately; v2 message IDs backported to v1 |
| OpenCode | 1 | Full v2 draft PR, closed by automation, unmerged |
| Pi (`pi-acp`) | 1 | v1 adapter; first-party support proposed only |
| Poolside (`pool`) | 1 | v1 |
| Qwen Code | 1 | v1 |
| Claude Agent (`claude-agent-acp`) | 1 | v1 |

## Clients

### Zed

The reference ACP client and the only *editor* with v2 support — behind its `acp-beta`
feature flag, **falling back to v1 when v2 negotiation fails**, and not default-on for
release users. Details are kept current in [Tackle's Zed client page](../../../docs/docs/dev/tackle/clients/zed.mdx).

Relevant wiring, from Zed `main`:

- A v2 client ingress exists (`crates/agent_servers/src/acp.rs`): `V2Builder` handlers for
  `session/update`, `terminal_update`/`terminal_output_chunk`, `request_permission`, and
  elicitation, alongside the v1 path.
- It forwards the project's configured MCP servers to the agent
  (`mcp_configurations_for_project`), and its docs state that "Zed-configured MCP servers
  may be forwarded to External Agents over ACP". That is MCP *forwarding*, not a
  client-capability MCP server — see [the client-tooling question](#the-client-tooling-question).

### Visual Studio Code

No native ACP support; only extensions. The most widely used, `formulahendry/vscode-acp`
("ACP Client"), is v1: it pins a v1-era `@agentclientprotocol/sdk` (`0.14.1` → `0.21.1` in
`0.2.0`, 2026-05-16) and ships a `FileSystemHandler` and `TerminalHandler` advertising
`fs` and `terminal`. A second extension, **ACP Pro** (`duclvz.acp-pro`), is a distinct
closed-source product (the `duclvz/vscode-acp` repo is a *fork* of formulahendry's, not the
same thing); its protocol version is **[unverified]** and it is excluded per the
open-source rule below.

### Neovim

No native support; community plugins, all v1 on direct evidence:

- `olimorris/codecompanion.nvim` — documents "ACP Protocol Version 1"; advertises `fs`,
  **not** `terminal`; passes the user's MCP servers.
- `carlos-algms/agentic.nvim` — its own debug log (issue `#63`) shows `protocolVersion = 1`,
  `fs` read/write, `terminal = false`; passes MCP servers.
- `yetone/avante.nvim` — keys off `agent_capabilities.loadSession`, implements
  `on_read_file`/`on_write_file`, consumes v1 `plan`/`toolCallId`/`rawInput`.
- `Ruddickmg/hermes.nvim` — Rust bridge pinned to the Rust SDK `agent-client-protocol`
  `1.0.1` (Renovate `#118`, 2026-07-03); v1 capability shape; `fs_*_access`/`terminal_access`.

### Emacs

`xenodium/agent-shell.el`, on `xenodium/acp.el`: sends `protocolVersion: 1` with
`clientCapabilities.fs`; tracks v1/RFD-shaped surfaces (fork, resume, steering). v1.

### Deliberately excluded

**Proprietary clients are out of scope.** It is for closed-source vendors to seek spec
correctness, not for tackle to reverse-engineer them. JetBrains is the notable case: its ACP
client is part of a closed-source IDE, so there is no source or SDK pin to read, and it was
dropped. (Incidentally, the ACP-org's own `codex-acp` documents an "AIR (JetBrains)" v2
client contract — so at least one proprietary v2 client exists — but we do not chase it.)

**Closed-source extensions are out of scope**, per the same rule: ACP Pro stays out, and
**Poolside Assistant**'s source could not be confirmed open within this pass, so its client
is excluded. **All other claims in this document are open-source-verifiable.**

## Agent harnesses

### Codex CLI (`codex-acp`) — the shipped v1/v2 template

The most important change since the original review: Codex's ACP adapter is now first-party
(`agentclientprotocol/codex-acp`, moved from `@zed-industries/codex-acp`) and **speaks both
ACP v1 and unstable v2 from one process**, with the version negotiated on the first
`initialize` and fixed for the connection (`readme-dev.md`, "ACP v2 support"). Its v2 chain
registers `initialize`, `session/new|list|close|delete|resume` (with `replayFrom`),
`session/set_config_option`, `session/cancel`, `auth/login|logout`, `session/prompt` (plus
`$/cancel_request`), `session/fork`, and `providers/list|set|disable`.

This matters for three reasons:

1. **It is the architecture our plan proposes, shipped.** One process, per-connection v1/v2
   selection, both surfaces live — a stronger reference than OpenCode's closed PR.
2. **It exercises draft surfaces we had scoped narrowly**, including `session/fork` and
   `providers/*`, and the `_session/steering` and `_session/goal` unstable extensions.
3. **It conformance-tests.** Release `v2.0.0` cites "resolve ACP v1 conformance failures
   found by acp-tck" — there is an ACP Test Compatibility Kit (see [testing tooling](#testing-tooling)).

It also accepts **client-provided MCP servers over command-based stdio config and HTTP
transport**, and documents a comprehensive "AIR v2 client contract" for JetBrains.

### Goose

**v1 only, and deliberately so.** PR `#12580` (merged 2026-10-01) fixed a real bug: `goose
acp` was echoing the client's offered `protocolVersion` (including `2`) into an
`InitializeResponse` built from v1 schema types, so v2-offering clients got a self-inconsistent
body and failed with `missing field 'info'`. The fix stamps `ProtocolVersion::LATEST` (v1)
instead. The PR states plainly that Goose "implements only v1" and that "the v2 schema is an
unstable draft". Its `on_initialize` still reads `client_capabilities.fs` and `.terminal`
and uses `session/set_mode`.

Forward progress on v2-*aligned* work without v2: PR `#10716` (2026-07-27) added stable
message identity "aligned with the ACP v2 message ID specification", populating
`ContentChunk.message_id` while staying on v1.

### OpenCode

Unchanged from the earlier pass: PR `anomalyco/opencode#44524` implemented the full v2 draft
checklist behind `OPENCODE_EXPERIMENTAL_ACP_V2` with an `AgentProtocolRouter` and v1
fallback, then was **closed by an automated cleanup bot** (2026-09-23), not merged. OpenCode
ships v1. Feature request `#44877` remains open. Codex's shipped adapter now supersedes this
as the reference.

### Pi

Pi's maintainer declined native ACP when asked (`earendil-works/pi#175`,
2025-12-12), offering "a new mode" as the only acceptable shape. The result is the community
adapter `svkozak/pi-acp`, **v1**, listed in the ACP registry and working with Zed. Its
limitations are instructive:

- **No ACP filesystem (`fs/*`) or terminal (`terminal/*`) delegation** — pi reads, writes,
  and executes locally.
- MCP servers from `session/new` are accepted and stored but **not wired through**.

A first-party ACP mode is now proposed in Discussion `#4444` (2026-05-12), whose own
motivation lists exactly the limits above and proposes to "prefer client FS/terminal when
the client advertises those capabilities" — i.e. Pi is *leaning in* to the v1 client surface
just as v2 removes it.

### Poolside (`pool`)

`poolsideai/pool` is open source and states it "runs as an ACP server inside editors and as
an ACP client driving other agents". v1: `session/list`, `session/load`,
`session/set_config_option`, slash commands, and a mid-turn steering capability named
`poolside/session_steer` (a custom capability, not standard v2 steering). No v2 evidence.

### Qwen Code

v1. The subject of the JetBrains incompatibility issue (`QwenLM/qwen-code#1502`), whose
maintainer confirms "we haven't added support for [ACP v2] yet"; a follow-up added
`session/set_model`, still v1.

### Claude Agent

`@agentclientprotocol/claude-agent-acp` (renamed from `@zed-industries/claude-code-acp`).
Observed traffic (`agent-shell` issue `#516`) shows `protocolVersion: 1` and
`agentInfo.name: @agentclientprotocol/claude-agent-acp`, version `0.24.2`. v1. Its
org-move mirrors Codex's, but no v2 support is documented for it yet **[unverified]**.

## The client-tooling question

The load-bearing unknown from the previous pass, now with an answer — and the answer is
"not yet".

The v2 RFD that removes the surface ("v2 Client Filesystem and Terminal Execution Surface")
is explicit about the replacement: *"If clients want to offer specialized tooling in these
areas, they can already expose a special MCP server to the agent to provide access."* It
also gives the rationale — the surface "has not been widely adopted by both Clients and
Agents, and many Agents are moving toward their own sandboxing and execution configuration
instead".

Both halves of that are visible in the wild:

- **Agents are already abandoning the client surface.** `pi-acp` declines `fs/*` and
  `terminal/*` and executes locally; `eve`'s ACP mode states it "does not give the agent
  access to the editor's host filesystem or terminal" and does not support client
  filesystem or terminal methods. Codex's adapter ships its own file/terminal handling.
- **No client is observed exposing its editor capabilities as an MCP server.** Clients
  *do* forward MCP servers — Zed forwards the project's configured servers, CodeCompanion
  and agentic.nvim pass the user's, JetBrains defaults `use_custom_mcp` to true, Codex
  accepts client-provided MCP servers — but that is forwarding the *user's* tools, not
  re-exposing `fs`/`terminal` as an MCP server.

So the replacement is a **protocol affordance, not an observed client behaviour**. When a
v1 client moves to v2, its `client.*` tools do not automatically reappear as MCP tools
unless that client also ships an editor-capabilities MCP server, which none is known to do.
This is the substantive risk the ADR must state plainly, and the thing FR-019's per-client
review exists to track.

## Testing tooling

- **`acp-tck`** — the ACP Test Compatibility Kit, referenced by the protocol's testing page
  and by `codex-acp`'s release notes ("resolve ACP v1 conformance failures found by
  acp-tck"). Worth adopting or borrowing from for tackle's conformance suite rather than
  hand-rolling every fixture.
- **`acp-bridge`** — a Rust bridge with an explicit `ProtocolVersion` (`V1`, `V2`,
  `LATEST = V2`), useful as a cross-check that a v1/v2 boundary is a solved shape.

## Harness progress since the original review (2026-09-17)

The original tackle ADR reviewed Pi, Codex CLI, OpenCode, and Goose recipes in
`../../2026-09-17-tackle/research/harness-review.md`. Spot-checking those findings against
today, the **non-ACP findings hold** (Pi has no built-in permissions; Codex's approval
profiles, hooks, and multi-agent roles are unchanged in kind; OpenCode's agent/command/skill
discovery is unchanged; Goose's recipe model is unchanged). The **ACP-relevant progress**:

| Harness | Original review | Today |
|---|---|---|
| Pi | Not covered as an ACP agent | Community `pi-acp` adapter (v1); first-party mode proposed |
| Codex CLI | Permissions/hooks/multi-agent only | First-party `codex-acp`, dual v1/v2, acp-tck conformance |
| OpenCode | Agent/command/skill discovery | v2 PR closed unmerged; still v1 |
| Goose | Recipe model | ACP made primary client protocol; v1 only, v2 message IDs backported |

One original-review assumption is now directly testable and worth restating: the session-fork
RFD the review cited has become v2's **draft `session/fork`**, and Codex's adapter registers
it — so the fork surface tackle implements for v2 parity is independently exercised.

## A naming collision to avoid

- There is an unrelated `agent-control-protocol/acp` (form-filling UI agents) that bumped
  its own major to "v2" in March 2026.
- **Adapter versions are not protocol versions.** `codex-acp` is at `v2.2.2` while speaking
  *unstable protocol v2*; the Rust SDK is at `3.5.0` while the stable wire protocol is `1`.
  Negotiate from `protocolVersion`, never from a package number.

## What this means for tackle

1. **v1 remains the surface essentially every deployed client uses.** v2 today is Zed behind
   a flag. The ADR's "v2 additive, v1 preserved" posture is the only one that serves users.
2. **The client-tooling replacement is aspirational.** See above — this is the sharpest
   finding and should be stated as a known loss, not papered over.
3. **Codex's adapter, not OpenCode's PR, is the architectural reference.** It is shipped,
   dual-version, and conformance-tested; it validates the plan's per-connection router and
   its decision to treat `session/fork` as a best-effort draft surface.
4. **Adopt `acp-tck` where possible** instead of bespoke conformance fixtures.
5. **Draft volatility is real.** Alpha schema churn and `unstable_protocol_v2` mean the v2
   surface can move; staying on the pinned SDK and marking draft surfaces is the right hedge.

## Open questions

- **Which v2 client will first expose editor capabilities as an MCP server**, and in what
  shape? None observed. The primary FR-019 question.
- **Does the Claude Agent adapter follow Codex to dual v1/v2?** **[unverified]**.
- **What does `acp-tck` actually cover**, and can tackle run it directly? To check at plan
  time.

### Scope discipline

This document is a *representative* reference, not a census — the ACP client list runs past
a hundred entries and most are v1 by default. Entries are chosen for decision value: clients
that will realistically drive tackle, and agents whose v2 work is prior art. Proprietary and
otherwise-unverifiable clients are explicitly excluded. The long tail is where FR-019 picks
up if a specific client ever matters.
