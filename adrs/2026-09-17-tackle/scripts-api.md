# Tackle script API

The interface exposed to Rhai scripts. Scripts are registered in config (`[scripts.<name>]`: `events`, `file`, optional `enabled`); the event determines the script's kind — `pre_tool_use` runs in a policy engine, all other events in behaviour engines.

## Conventions

- **Values are plain object maps** (`#{ "key": value }`) and arrays — JSON-shaped. No custom types are registered; everything is serializable and inspectable.
- **One Rhai function per event**, named after the event (`fn pre_tool_use(request) { ... }`). An absent function is a no-op for that event, so one file may serve several events.
- **Errors and limits**: engines enforce operation, call-level, expression-depth, collection-size, script-size (256 KiB), and time limits (~1s policy, ~10s behaviour); no module imports; a fresh engine per invocation. The time limit bounds **script evaluation only** — it does not tick while a host call is blocked (Rhai's progress callback runs between operations), so `await_completion` waits are governed by their own timeout parameter. A policy script error or timeout fails closed to `ask`; a behaviour script error degrades gracefully (logged, harness continues).
- **Event suppression**: events fire in every session, ephemeral ones included — the payload carries the session kind, and scripts decide how to handle their own re-entrancy (the default compaction script guards against re-running inside its own ephemeral forks).

## Event payloads

All payloads include `session` and (where a session is active) `actor`:

```rhai
session = #{ id, cwd, title, ephemeral, actor, agent }   // ephemeral: bool — scripts guard their own re-entrancy with it
actor   = #{ name, persona, model }                // model is endpoint-qualified
```

### `pre_tool_use(request)` — policy; must return a verdict

```rhai
request = #{
    invokable: "mcp.litterbox.exec",   // namespaced: mcp.<server>.<tool> | prompt.<name> | agent.<actor>
    kind: "mcp",                       // mcp | prompt | agent
    arguments: #{ command: "cargo test", cwd: "/workspace" },   // structured, named parameters
    interactive: true,                 // whether the session is currently user-attended
    previously_granted: false,         // the grant store held an allow record for this invokable
    turn: #{ command: "deploy" },      // the /command that initiated the turn, if any
    session: session, actor: actor,
}
```

Return: `#{ decision: "allow" | "deny" | "ask", reason: "..." }` — `reason` is optional and surfaces in the permission prompt; `allow`/`deny` are final, `ask` falls through to the fixed fallback. A malformed or missing return fails closed to `ask`.

### `post_turn(event)` — behaviour

```rhai
event = #{ turn_id, stop_reason, request_count,
           recent_turn_ids: [...],            // the session's last turns, oldest first
           usage: #{ used, size, cost_usd }, session, actor }
```

### `session_forked(event)` — behaviour

```rhai
event = #{ session,                       // the NEW session
           source_session_id, fork_point_turn_id }
```

### `compaction_requested(event)` — behaviour

Fires **instead of a model turn** in the main session: compaction-tagged prompts are intercepted, the summarisation runs in the ephemeral fork, and the harness announces the result in-band. The intercepted main-session turn stores the user message (the command) plus the harness's announcement agent message; `turn_id` is that turn.

```rhai
event = #{ turn_id, usage: #{ used, size, cost_usd }, session, actor }
```

### `title_trigger(event)` — behaviour

Fires after every completed turn while the session has no title; the script decides what to do.

```rhai
event = #{ turns_since_title, session, actor }
```

## Host functions

Available to **all** scripts:

| Function | Returns | Notes |
|---|---|---|
| `history_search(query)` | array of `#{ role, content, turn_id, created_at }` | Read-only search over the current session's messages; row- and byte-capped. |
| `context_usage()` | `#{ used, size }` | Current context utilisation for the session. |
| `log(message)` | `()` | Structured line to stderr, prefixed with the script name. |

Available to **behaviour** scripts only (policy engines physically lack them):

| Function | Returns | Notes |
|---|---|---|
| `fork_session(opts)` | `#{ session_id, agent }` | `opts = #{ ephemeral: bool, actor: "name" }`. Forks the current session; ephemeral forks share the DAG, are never revert points, and attribute usage to the parent. An unknown actor is a script error (graceful degradation). |
| `send_prompt(session_id, text)` | `()` | Drives a prompt through that session's normal agent loop — model-mediated, pipeline-gated. Scripts never invoke tools directly. |
| `await_completion(session_id, timeout_secs)` | `#{ stop_reason, final_message, usage }` | Blocks until the session's turn completes or the timeout expires (error). `final_message` is the text of the final assistant message. |
| `insert_seed(text)` | `()` | Inserts a harness-authored seed message into the forked session (session_forked scripts only). |
| `record_compaction(summary, first_retained_turn_id)` | `()` | Appends a compaction turn. `first_retained_turn_id` = the oldest turn kept verbatim (keep-recent compaction); pass `()` to elide everything before the summary. The harness clamps the value to be newer than any older compaction turn on the chain. Reversible by deleting the turn. |
| `set_session_title(title)` | `()` | Emits ACP `session_info_update`. |

**Never exposed**: tool invocation (scripts drive the model instead), the grant store (read or write), filesystem, network.

## Complete example scripts

### `pre_tool_use` — policy

```rhai
// deny-secrets.rhai — policy script for pre_tool_use.
// allow/deny are final; ask falls through to the fixed ask fallback.

const ALWAYS_DENY = [
    "mcp.litterbox.exec",              // shell execution: denied outright
];

fn pre_tool_use(request) {
    // 1. Blanket rules by invokable name.
    if ALWAYS_DENY.contains(request.invokable) {
        return #{ decision: "deny", reason: "shell execution is denied by policy" };
    }

    // 2. Argument inspection: deny access to files known to hold secrets.
    if request.arguments.contains("path") {
        let path = request.arguments.path;
        if path.contains(".npmrc") || path.contains(".netrc") || path.contains(".aws") {
            return #{ decision: "deny", reason: `${path} is a credential file, denied by policy` };
        }
    }

    // 3. Shell-shaped arguments: deny password-manager access.
    if request.arguments.contains("command") {
        let cmd = request.arguments.command;
        if cmd.contains("op ") || cmd.contains("1password") || cmd.contains("pass ") {
            return #{ decision: "deny", reason: "password manager access is denied by policy" };
        }
    }

    // 4. Everything else: fall through to the fixed ask fallback.
    #{ decision: "ask" }
}
```

### `session_forked` — behaviour

```rhai
// provision-sandbox.rhai — behaviour script for session_forked.
// Seeds the fork with a user-facing note, then has the model provision
// an environment through the normal permission pipeline.

fn session_forked(event) {
    insert_seed(`Forked from session ${event.source_session_id}. ` +
        `This session works in its own copy of the workspace; ` +
        `I'll set up a matching environment now.`);

    // Fire-and-forget: the provisioning turn runs in the new session,
    // visible to the user there. No await — the user just forked and is
    // present to answer any permission prompt the model triggers.
    send_prompt(event.session.id,
        `A new session was forked from ${event.source_session_id} at turn ` +
        `${event.fork_point_turn_id}. Call mcp.litterbox.sandbox-create to ` +
        `provision a sandbox for this session, then report the sandbox id.`);
}
```

### `post_turn` — behaviour (shipped default: automatic compaction)

```rhai
// compaction.rhai — behaviour script for post_turn (shipped default).
// When utilisation crosses 85%, summarise old context in an ephemeral fork,
// keeping the most recent turns verbatim.

const THRESHOLD = 0.85;
const KEEP_RECENT = 4;                     // turns kept verbatim

fn post_turn(event) {
    if event.session.ephemeral { return (); }   // never re-enter our own forks
    let ratio = event.usage.used.to_float() / event.usage.size.to_float();
    if ratio < THRESHOLD {
        return ();
    }

    log(`context at ${(ratio * 100.0)}% — compacting`);

    let fork = fork_session(#{ ephemeral: true, actor: "summariser" });
    send_prompt(fork.session_id,
        "Summarise this conversation in under 500 words. Preserve: the task state, " +
        "decisions made and why, open questions, and anything the user asked to remember.");
    let result = await_completion(fork.session_id, 120);

    if result.stop_reason != "end_turn" {
        log(`summarisation failed: ${result.stop_reason}`);
        return ();                         // degrade gracefully; try again next turn
    }

    // Keep the most recent turns verbatim; elide the rest behind the summary.
    let turns = event.recent_turn_ids;
    let keep = if turns.len() < KEEP_RECENT { turns.len() } else { KEEP_RECENT };
    if keep > 0 {
        record_compaction(result.final_message, turns[turns.len() - keep]);
    } else {
        record_compaction(result.final_message, ());
    }
}
```

### `compaction_requested` — behaviour (shipped default: manual /compact)

```rhai
// compact-now.rhai — behaviour script for compaction_requested.
// Runs when the user invokes a compaction-tagged prompt (/compact):
// summarise everything so far, keep nothing verbatim.

fn compaction_requested(event) {
    log(`manual compaction requested at ${event.usage.used} tokens`);

    let fork = fork_session(#{ ephemeral: true, actor: "summariser" });
    send_prompt(fork.session_id,
        "Summarise this conversation in under 500 words. Preserve: the task state, " +
        "decisions made and why, open questions, and anything the user asked to remember.");
    let result = await_completion(fork.session_id, 120);

    if result.stop_reason != "end_turn" {
        log(`compaction failed: ${result.stop_reason}`);
        return ();                         // context untouched; the harness reports the failure
    }

    record_compaction(result.final_message, ());   // elide everything before the summary
    log(`compacted: ${event.usage.used} → ${result.usage.used} tokens`);
}
```

### `title_trigger` — behaviour (shipped default: automatic titling)

```rhai
// titling.rhai — behaviour script for title_trigger (shipped default).
// Generates a short session title from the conversation so far.

fn title_trigger(event) {
    if event.session.ephemeral { return (); }   // never re-enter our own forks
    let fork = fork_session(#{ ephemeral: true, actor: "summariser" });
    send_prompt(fork.session_id,
        "Write a title for this session: at most six words, no quotes, no period. " +
        "Reply with the title and nothing else.");
    let result = await_completion(fork.session_id, 60);

    if result.stop_reason != "end_turn" {
        return ();                         // titling failures are silent
    }

    set_session_title(result.final_message.trim());
}
```

The shipped scripts use the built-in `summariser` actor — a no-tools actor on the default persona's model, shipped alongside the default persona and actor for exactly this purpose.
