//! The in-memory structural mirror of a session's turn DAG, backed by
//! daggy — acyclicity and single-parent semantics enforced by
//! construction. SQLite remains the source of truth; this mirror is
//! rebuildable from rows at any time, and property tests pin the SQL
//! walks to daggy's traversals as an independent oracle.

use super::{assembly_order, Turn, TurnId, TurnKind};
use daggy::{Dag, NodeIndex};
use std::collections::HashMap;

pub struct SessionGraph {
    dag: Dag<TurnId, ()>,
    indices: HashMap<TurnId, NodeIndex>,
    kinds: HashMap<TurnId, TurnKind>,
    first_retained: HashMap<TurnId, Option<TurnId>>,
}

impl SessionGraph {
    /// Builds the graph from turn rows: one node per turn, one edge per
    /// parent link. Cycles are refused by daggy; a malformed chain (a
    /// parent missing from the set) is skipped defensively.
    pub fn build(turns: impl IntoIterator<Item = Turn>) -> Self {
        let turns: Vec<Turn> = turns.into_iter().collect();
        let mut dag = Dag::new();
        let mut indices = HashMap::new();
        let mut kinds = HashMap::new();
        let mut first_retained = HashMap::new();

        // Nodes first, parents second, so add_edge always finds both ends.
        for turn in &turns {
            let index = dag.add_node(turn.id.clone());
            indices.insert(turn.id.clone(), index);
            kinds.insert(turn.id.clone(), turn.kind);
            first_retained.insert(turn.id.clone(), turn.first_retained_turn_id.clone());
        }
        for turn in &turns {
            let Some(parent_id) = &turn.parent_id else {
                continue;
            };
            if let (Some(&parent), Some(&child)) = (indices.get(parent_id), indices.get(&turn.id)) {
                let _ = dag.add_edge(parent, child, ());
            }
        }

        Self {
            dag,
            indices,
            kinds,
            first_retained,
        }
    }

    /// The assembly order for a session with head `head`: the compaction
    /// summaries first (they replace the elided prefix), then the retained
    /// turns oldest to newest. The walk and its truncation state machine
    /// mirror the SQL CTE exactly; property tests pin the two together.
    pub fn assembly_order(&self, head: &TurnId) -> Vec<TurnId> {
        assembly_order(self.context_walk(head))
    }

    /// The raw parent-chain walk from `head`, newest first, with the
    /// compaction truncation state machine applied.
    pub fn context_walk(&self, head: &TurnId) -> Vec<(TurnId, TurnKind)> {
        let mut walk = Vec::new();
        let mut stop_at: Option<TurnId> = None;
        let mut current = Some(head.clone());

        while let Some(id) = current {
            let Some(&index) = self.indices.get(&id) else {
                break; // dangling parent: defensive stop
            };
            let kind = self.kinds[&id];
            walk.push((id.clone(), kind));

            if kind == TurnKind::Compaction {
                match &self.first_retained[&id] {
                    None => break, // full compaction: nothing below the summary
                    Some(retained) => {
                        stop_at = Some(retained.clone());
                        current = self.parent_id(index);
                    }
                }
            } else if stop_at.as_ref() == Some(&id) {
                break; // keep-recent boundary reached
            } else {
                current = self.parent_id(index);
            }
        }

        walk
    }

    fn parent_id(&self, index: NodeIndex) -> Option<TurnId> {
        // daggy re-exports the petgraph version it builds on.
        self.dag
            .graph()
            .neighbors_directed(index, daggy::petgraph::Direction::Incoming)
            .next()
            .and_then(|parent| self.dag.node_weight(parent).cloned())
    }
}
