//! Bitset dataflow, following rustc_mir_dataflow/src/framework/mod.rs:99,244.
//! The object-safe Analysis fixes the domain to BitSet and direction to an enum,
//! avoiding per-analysis engine monomorphization. Effects must be monotone under join.

pub mod bitset;
mod cursor;
mod direction;
mod initialized;
mod locals;
mod visitor;

pub use bitset::{BitMatrix, BitSet};
pub use cursor::ResultsCursor;
pub use direction::{CallReturnPlaces, Direction, SwitchTargetIndex, TerminatorEdges};
pub use initialized::{EverInitializedPlaces, MaybeInitializedPlaces, MaybeUninitializedPlaces};
pub use locals::{always_storage_live_locals, MaybeBorrowedLocals, MaybeLiveLocals, MaybeRequiresStorage, MaybeStorageDead, MaybeStorageLive};
pub use visitor::{visit_reachable_results, visit_results, ResultsVisitor};

use super::*;
use direction::{apply_effect, Cfg};
use std::collections::VecDeque;

pub trait Analysis {
    fn direction(&self) -> Direction { Direction::Forward }
    /// rustc_mir_dataflow/src/framework/lattice.rs:140 (MaybeReachable).
    /// Lift the forward domain with Unreachable, stored separately from its bits.
    /// No transfer runs on Unreachable; joining Reachable(empty) still changes it.
    fn bottom_is_unreachable(&self) -> bool { false }
    fn bottom_value(&self, body: &Body) -> BitSet;
    fn initialize_start_block(&self, _body: &Body, _state: &mut BitSet) {}
    fn join(&self, state: &mut BitSet, incoming: &BitSet) -> bool { state.union(incoming) }
    fn apply_early_statement_effect(&self, _state: &mut BitSet, _stmt: &Statement, _at: Location) {}
    fn apply_primary_statement_effect(&self, _state: &mut BitSet, _stmt: &Statement, _at: Location) {}
    fn apply_early_terminator_effect(&self, _state: &mut BitSet, _term: &Terminator, _at: Location) {}
    fn apply_primary_terminator_effect<'a>(&self, _state: &mut BitSet, term: &'a Terminator, _at: Location) -> TerminatorEdges<'a> {
        TerminatorEdges::for_terminator(term)
    }
    fn apply_call_return_effect(&self, _state: &mut BitSet, _block: BasicBlock, _places: CallReturnPlaces<'_>) {}

    /// rustc framework/mod.rs:207,218. Passing the switch directly replaces the generic
    /// SwitchIntData cache; the analysis may precompute its own per-block facts.
    /// Backward switch edge effects are unsupported, as in direction.rs:127.
    fn has_switch_int_edge_effect(&self, _block: BasicBlock, _targets: &SwitchTargets, _discr: &Operand) -> bool { false }
    fn apply_switch_int_edge_effect(&self, _state: &mut BitSet, _block: BasicBlock, _targets: &SwitchTargets, _discr: &Operand, _index: SwitchTargetIndex) {}
}

/// Entry is in analysis order: before the first statement for Forward, after the
/// terminator for Backward. Only reachable blocks may be queried; lifted domains
/// additionally exclude blocks reachable solely through edges the analysis pruned.
/// rustc_mir_dataflow/src/framework/results.rs:14 and framework/mod.rs:244.
pub struct Results<'a> {
    body: &'a Body,
    analysis: &'a dyn Analysis,
    entry_states: Vec<BitSet>,
    reachable: BitSet,
}

impl<'a> Results<'a> {
    pub fn body(&self) -> &'a Body { self.body }
    pub fn analysis(&self) -> &'a dyn Analysis { self.analysis }
    pub fn is_reachable(&self, block: BasicBlock) -> bool { self.reachable.contains(block.0 as usize) }
    pub fn entry_state(&self, block: BasicBlock) -> &BitSet {
        assert!(self.is_reachable(block), "dataflow state requested for unreachable block");
        &self.entry_states[block.0 as usize]
    }
    pub fn cursor(&self) -> ResultsCursor<'_, 'a> { ResultsCursor::new(self) }
}

/// rustc_mir_dataflow/src/framework/mod.rs:244. Seed every reachable block, even
/// when bottom propagates unchanged: its transfer may generate facts from bottom.
pub fn iterate_to_fixpoint<'a>(body: &'a Body, analysis: &'a dyn Analysis) -> Results<'a> {
    assert!(!body.basic_blocks.is_empty());
    let cfg = Cfg::new(body);
    let count = body.basic_blocks.len();
    let bottom = analysis.bottom_value(body);
    let lifted = analysis.bottom_is_unreachable();
    assert!(!lifted || analysis.direction() == Direction::Forward, "lifted backward domain is unsupported");
    let mut results = Results {
        body, analysis, entry_states: vec![bottom.clone(); count],
        reachable: if lifted { BitSet::new_empty(count) } else { cfg.reachable.clone() },
    };
    results.reachable.insert(0);
    analysis.initialize_start_block(body, &mut results.entry_states[0]);
    assert_eq!(results.entry_states[0].domain_size(), bottom.domain_size());
    let backward = analysis.direction() == Direction::Backward;
    if backward {
        assert_eq!(results.entry_states[0], bottom, "backward start state is unsupported");
    }
    let mut queue = VecDeque::new();
    let mut queued = BitSet::new_empty(count);
    for i in 0..cfg.postorder.len() {
        let block = if backward { cfg.postorder[i] } else { cfg.postorder[cfg.postorder.len() - 1 - i] };
        queue.push_back(block);
        queued.insert(block.0 as usize);
    }
    let mut state = bottom.clone();
    let mut edge_state = bottom;
    while let Some(block) = queue.pop_front() {
        let b = block.0 as usize;
        queued.remove(b);
        if lifted && !results.is_reachable(block) { continue; }
        state.clone_from(&results.entry_states[b]);
        let data = &body.basic_blocks[b];
        if backward {
            for step in 0..(data.statements.len() + 1) * 2 {
                apply_effect(body, analysis, &mut state, block, step);
            }
            for i in cfg.pred_offsets[b]..cfg.pred_offsets[b + 1] {
                let edge = &cfg.edges[cfg.predecessors[i]];
                edge_state.clone_from(&state);
                direction::apply_edge_effect(analysis, &mut edge_state, *edge);
                propagate(&mut results, edge.source, &edge_state, &mut queue, &mut queued);
            }
        } else {
            for step in 0..data.statements.len() * 2 + 1 {
                apply_effect(body, analysis, &mut state, block, step);
            }
            let at = Location { block, statement_index: data.statements.len() };
            let edges = analysis.apply_primary_terminator_effect(&mut state, data.terminator.as_ref().unwrap(), at);
            for i in 0..edges.len() {
                let edge = edges.edge(block, i);
                assert!(cfg.reachable.contains(edge.target.0 as usize), "analysis introduced an unreachable edge");
                edge_state.clone_from(&state);
                direction::apply_edge_effect(analysis, &mut edge_state, edge);
                propagate(&mut results, edge.target, &edge_state, &mut queue, &mut queued);
            }
        }
    }
    results
}

fn propagate(results: &mut Results<'_>, target: BasicBlock, state: &BitSet, queue: &mut VecDeque<BasicBlock>, queued: &mut BitSet) {
    let i = target.0 as usize;
    let reached = results.analysis.bottom_is_unreachable() && results.reachable.insert(i);
    let changed = results.analysis.join(&mut results.entry_states[i], state);
    if (reached || changed) && queued.insert(i) { queue.push_back(target); }
}

#[cfg(test)]
mod tests;
