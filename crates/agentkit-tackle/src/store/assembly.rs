//! Context-assembly ordering: the ordering step of the compaction truncation state machine, shared by the in-memory backend, the daggy mirror, and the tests.

use super::model::{TurnId, TurnKind};

/// Orders a parent-chain walk (newest first) for assembly: compaction
/// summaries first — they replace the elided prefix — then the retained
/// turns oldest to newest.
pub(crate) fn assembly_order(walk: Vec<(TurnId, TurnKind)>) -> Vec<TurnId> {
    let mut summaries = Vec::new();
    let mut retained = Vec::new();
    for (id, kind) in walk {
        if kind == TurnKind::Compaction {
            summaries.push(id);
        } else {
            retained.push(id);
        }
    }
    summaries.reverse();
    retained.reverse();
    summaries.extend(retained);
    summaries
}
