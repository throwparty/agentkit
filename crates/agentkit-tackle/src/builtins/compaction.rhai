// compaction.rhai — behaviour script (shipped default), serving both
// compaction events. Define a [scripts.compaction] entry with the same
// name to override this wholesale.

// post_turn: automatic compaction — when utilisation crosses the
// threshold, summarise old context in an ephemeral fork, keeping the
// most recent turns verbatim.
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

// compaction_requested: manual /compact — summarise everything so far
// from the intercepted turn onward, keeping the command turn verbatim.
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

    record_compaction(result.final_message, event.turn_id);   // elide everything before the intercepted turn
    log(`compacted: ${event.usage.used} → ${result.usage.used} tokens`);
}
