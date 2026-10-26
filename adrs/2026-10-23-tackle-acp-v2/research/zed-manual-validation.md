# Manual Zed validation: ACP v1 and v2

Runbook for [T-020](../tasks.toon). Fill in the result tables as you go. This is the
human-in-the-loop check that the automated suites cannot make: that a real client renders
what the wire says.

Record findings here, then copy any client-specific behaviour into
[`docs/docs/dev/tackle/clients/zed.mdx`](../../../docs/docs/dev/tackle/clients/zed.mdx).

---

## Environment

Fill this in before starting; a result is only meaningful against a dated build.

| Field | Value |
| --- | --- |
| Date | 2026-10-24 |
| Tester | Luke Carrier <luke@throw.party> |
| OS / arch | NixOS 26.11 (Zokor) |
| Zed version (from `zed: about`, or `zed --system-specs`) | v1.27.0+dev.9f305aaf88370e8ca39c702242186e3d8f97ecca (Zed Dev 9f305aaf88370e8ca39c702242186e3d8f97ecca) (Taylor's Version) |
| Tackle commit (`jj log -r @- --no-graph -T commit_id`) | bfc3b886e7111ffec031cadb177a9223706a3f21 |
| Model endpoint / model | Switchboard / |
| Credential setup (`tackle auth`, `auth = "none"`, …) | |
| `acp-beta` state | on / off |
| `RUST_LOG` for the agent | `info` (recommended) |

### Build tackle

```sh
nix develop ./nix --command cargo build -p agentkit-tackle --bin agentkit-tackle
```

The binary is `target/debug/agentkit-tackle`.

### Configure the agent in Zed

Craft an isolated config from your system config, plus the fixture stanza, without touching
the system file:

```sh
adrs/2026-10-23-tackle-acp-v2/research/zed-validation-config.sh
```

It copies the system user config to `target/zed-validation-config/config.toml`, appends
`[mcp_servers.testfs]`, and prints the directory and dedicated session database to pass to
`--config-dir` and `--db-path`. The separate database keeps the validation run out of your
production session list. It is idempotent and never writes to the source config. Override the
source with `TACKLE_SOURCE_CONFIG=/path/to/config.toml`, or pass a destination directory as
the first argument.

Use `zed: open settings file` (or the External Agents page) and add an entry under
`agent_servers`, substituting the absolute path to the binary and to the generated directory:

```json
{
  "agent_servers": {
    "Tackle": {
      "type": "custom",
      "command": "/absolute/path/to/target/debug/agentkit-tackle",
      "args": ["stdio", "--config-dir", "/absolute/path/to/target/zed-validation-config", "--db-path", "/absolute/path/to/target/zed-validation.db"],
      "env": { "RUST_LOG": "info" }
    }
  }
}
```

`--config-dir` takes a directory, not a file: Tackle reads `config.toml` inside it and uses it
as the user layer for this agent process only. Zed picks up settings changes without a
restart. The agent appears in the agent panel.

### Select the protocol version

Tackle serves both versions and lets the client choose. Zed decides:

- **ACP v2** — enable the `acp-beta` feature flag. On a staff/debug build it is on by
  default; otherwise set `"feature_flags": { "acp-beta": "on" }` in `settings.json`
  (honoured only when feature-flag overrides are enabled), or have Zed's server announce it.
- **ACP v1** — set `"feature_flags": { "acp-beta": "off" }` explicitly. On staff/debug
  builds the flag is on by default, so deleting an override is not enough. Zed falls back
  to v1 when v2 negotiation fails.

**Confirm which version was selected.** Tackle logs one line per connection at `info`:

```text
ACP protocol negotiated protocol="v1"
ACP protocol negotiated protocol="v2"
```

Find it in Zed's log (`zed: open log`) or run the agent with stderr captured. Record the
observed line in each section below. Do not assume the flag took effect.

### Tooling for the tool rows

v1 needs nothing extra: Zed advertises `fs` and `terminal`, so tackle exposes `client.fs.*`
and `client.terminal.*` to the model.

v2 has no client filesystem or terminal surface. For the tool rows, the repo ships a
**test-only** read-only filesystem MCP fixture, `tackle-mcp-fs`, alongside
`tackle-mcp-echo`. It is never registered automatically and is not part of the shipped tool
surface: you must configure it explicitly for this run, and remove it afterwards.

```sh
nix develop ./nix --command cargo build -p agentkit-tackle --bin tackle-mcp-fs
```

The script above appends this stanza to the run's config. To add it by hand, put it in
`target/zed-validation-config/config.toml` (the directory passed to `--config-dir`):

```toml
[mcp_servers.testfs]
transport = "stdio"
command = "/absolute/path/to/target/debug/tackle-mcp-fs"
args = ["--root", "/absolute/path/to/project"]
```

Its tools are `mcp.testfs.read_file`, `mcp.testfs.list_directory`, and `mcp.testfs.grep`.
They are read-only and confined to `--root`; paths outside it are rejected. Model calls still
pass through the permission pipeline, so the approve/reject rows exercise the v2
`requires_action` transition.

This fixture exists for validation only. Do not treat it as production tooling; write, edit,
and command execution are deliberately absent.

### Optional: capture the wire

For a wire-level record, run the agent under a tee:

```sh
RUST_LOG=info target/debug/agentkit-tackle stdio 2> >(tee /tmp/tackle-v2.log >&2)
```

and point the Zed `command` at a wrapper script that does the same. The conformance reports
in `target/conformance/report-{1,2}.json` remain the authoritative requirement-level record;
this run is about rendering and UX.

---

## ACP v1

Observed negotiation log line: `__________________________`

| # | Step | Expected | Result | Notes / evidence |
| --- | --- | --- | --- | --- |
| 1 | Open a new Tackle thread in a project | Agent connects, a session opens, the first-run seed is visible | ✅ pass ☐ fail | |
| 2 | Prompt: "Write a two-sentence description of this repository." | Text streams token by token; the turn ends cleanly | ✅ pass ☐ fail | |
| 3 | Ask it to read a file in the project | A tool call appears; the permission prompt shows allow/reject; approving returns file content | ✅ pass ☐ fail | |
| 4 | Reject a permission request | The turn continues without the tool result; no crash | ✅ pass ☐ fail | |
| 5 | Start a long response and press stop | Turn ends with a cancelled state; partial output retained | ✅ pass ☐ fail | |
| 6 | Switch the model via the config selector | Selector updates; the next turn uses the new model | ✅ pass ☐ fail | |
| 7 | Close the thread and reopen/resume it | History replays; the session is listed; config selectors restored | ✅ pass ☐ fail | |
| 8 | Run `/compact` | Compaction summary appears; context/usage reflects the drop | ✅ pass ☐ fail | |
| 9 | Run `/fork` | A new forked thread appears seeded from the parent | ✅ pass ☐ fail | |
| 10 | Trigger a background model call (e.g. titling) | A notice appears (v1 sends it only when the client advertised the unstable notice capability) | ✅ pass ☐ fail ☐ n/a | |
| 11 | Delete a session from the history panel | Session disappears and does not reopen | ✅ pass ☐ fail | |

v1 divergence notes:

```text

```

---

## ACP v2

Observed negotiation log line:

```text
2026-10-24T23:43:15+01:00 WARN  [agent_servers::acp::transport] agent stderr: 2026-10-24T22:43:15.059874Z  INFO connection{name="tackle"}: agentkit_tackle::acp::v2::initialize: ACP protocol negotiated protocol="v2"
```

| # | Step | Expected | Result | Notes / evidence |
| --- | --- | --- | --- | --- |
| 1 | Open a new Tackle thread | Agent connects over v2, a session opens, the seed is visible | ✅ pass ☐ fail | |
| 2 | Prompt: "Write a two-sentence description of this repository." | The user message is accepted immediately (insertion response), then the turn streams and ends | ✅ pass ☐ fail | |
| 3 | Watch the working/idle indicator during a turn | `running` while working, `idle` at the end; no updates after the terminal idle | ✅ pass ☐ fail | |
| 4 | Ask it to read `README.md` with `mcp.testfs.read_file` | A `tool_call_update` card appears; the permission prompt shows allow/reject; approving returns the file content | ✅ pass ☐ fail | |
| 5 | Approve a permission request | The prompt blocks with a requires-action state, then resumes to running | ✅ pass ☐ fail | |
| 6 | Reject a permission request | Turn continues without the tool result; no crash | ✅ pass ☐ fail | |
| 7 | Start a long response and press stop | Idle arrives with `stopReason: cancelled`; partial output retained | ✅ pass ☐ fail | |
| 8 | Switch the model via the config selector | Selector updates; the next turn uses the new model; the response carries the full option list | ✅ pass ☐ fail | |
| 9 | Close and resume the thread | Replayed history appears with the same message ids; current state reported before the response | ✅ pass ☐ fail | |
| 10 | Run `/compact` | Compaction update is accepted with no capability gate; summary appears | ✅ pass ☐ fail | |
| 11 | Run `/fork` | Zed does not call `session/fork`; `/fork` reports the new session id in the parent turn. The fork is stored and can be opened from history/import; no thread appears automatically | ✅ pass ☐ fail | |
| 12 | Trigger a background model call | A notice appears with no client capability advertised | ✅ pass ☐ fail | |
| 13 | Tool-card rendering across a multi-step tool turn | One card per `toolCallId`; updates patch in place, no duplicate cards | ✅ pass ☐ fail | |
| 14 | Tool inventory | The model sees the explicitly configured `mcp.testfs.*`; no `client.fs.*`/`client.terminal.*` exists under v2 | ✅ pass ☐ fail | |
| 15 | Read a path outside the fixture root | `tackle-mcp-fs` rejects it; the turn continues without a crash | ✅ pass ☐ fail | |

v2 divergence notes:

```text
None
```

---

## Cross-version checks

| # | Check | Expected | Result | Notes |
| --- | --- | --- | --- | --- |
| 1 | Run v1, then v2, against the same session store | Sessions are listed under both; a v1 session resumes under v2 and vice versa | ☐ pass ☐ fail | |
| 2 | Confirm no v2-only update is rendered on a v1 connection | No state-update UI, no keyed-upsert behaviour under v1 | ☐ pass ☐ fail | |
| 3 | Compare a tool turn between versions | Same logical sequence; v2 is a keyed upsert, v1 is create-then-update | ☐ pass ☐ fail | |

## Findings to carry back

List anything that differs from the TCK-verified behaviour, then either fix it or document
it in the Zed client page.

| Finding | Version | Severity | Action (fix / document / follow-up) | Issue link |
| --- | --- | --- | --- | --- |
| | | | | |
| | | | | |
| | | | | |

## Sign-off

- [x] Both versions negotiated and recorded above.
- [x] Every applicable row filled in; `n/a` rows explained.
- [x] Divergences recorded and triaged.
- [x] Client page updated with any new Zed-specific behaviour.
- [x] T-020 marked complete in `tasks.toon`.
