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

    // The host trims the title before storing (rhai's trim is
    // in-place and returns unit).
    set_session_title(result.final_message);
}
