//! Bitset dataflow, following rustc_mir_dataflow/src/framework/mod.rs:99,288.
//! The object-safe Analysis fixes the domain to BitSet and direction to an enum,
//! avoiding per-analysis engine monomorphization. Effects must be monotone under join.

pub mod bitset;
pub mod interval;
mod cursor;
mod direction;
mod initialized;
mod locals;
mod visitor;

pub use bitset::{BitMatrix, BitSet};
pub use interval::IntervalSet;
pub use cursor::ResultsCursor;
pub use direction::{CallReturnPlaces, Direction, SwitchTargetIndex, TerminatorEdges};
pub use initialized::{EverInitializedPlaces, MaybeInitializedPlaces, MaybeUninitializedPlaces};
pub use locals::{always_storage_live_locals, MaybeBorrowedLocals, MaybeLiveLocals, MaybeRequiresStorage, MaybeStorageDead, MaybeStorageLive};
pub use visitor::{visit_reachable_results, visit_results, ResultsVisitor};

use super::*;
use direction::{apply_effect, Cfg};

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
/// rustc_mir_dataflow/src/framework/results.rs:14 and framework/mod.rs:288.
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

/// rustc_mir_dataflow/src/framework/mod.rs:288 (nightly 8d1a764). Seed every reachable block,
/// even when bottom propagates unchanged: its transfer may generate facts from bottom.
///
/// The min-rank traversal of mod.rs:312: a block's rank is its position in the reverse
/// postorder (flipped for a backward analysis), every reachable block starts dirty, and the
/// next block is always the dirty one of least rank. A back edge that changes an earlier block
/// returns there at once, so a loop settles before the blocks after it run, and loop-free code
/// takes a single pass. Each target joins the exit state in place (mod.rs:370); only an edge
/// effect needs a copy of it.
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
    // `cfg.postorder` holds the reachable blocks; a forward rank counts from its end.
    let last = cfg.postorder.len() - 1;
    let mut ranks = vec![usize::MAX; count];
    for (i, block) in cfg.postorder.iter().enumerate() {
        ranks[block.0 as usize] = if backward { i } else { last - i };
    }
    let mut work = Worklist { ranks, dirty: BitSet::new_filled(cfg.postorder.len()), curr: 0, backward };
    let mut state = bottom.clone();
    let mut edge_state = bottom;
    loop {
        let block = cfg.postorder[if backward { work.curr } else { last - work.curr }];
        work.dirty.remove(work.curr);
        let b = block.0 as usize;
        if !lifted || results.is_reachable(block) {
            state.clone_from(&results.entry_states[b]);
            let data = &body.basic_blocks[b];
            if backward {
                for step in 0..(data.statements.len() + 1) * 2 {
                    apply_effect(body, analysis, &mut state, block, step);
                }
                // rustc direction.rs:80: only a call's return edge carries an effect backward.
                for i in cfg.pred_offsets[b]..cfg.pred_offsets[b + 1] {
                    let edge = cfg.edges[cfg.predecessors[i]];
                    if direction::has_edge_effect(analysis, edge) {
                        edge_state.clone_from(&state);
                        direction::apply_edge_effect(analysis, &mut edge_state, edge);
                        work.propagate(&mut results, edge.source, &edge_state);
                    } else {
                        work.propagate(&mut results, edge.source, &state);
                    }
                }
            } else {
                for step in 0..data.statements.len() * 2 + 1 {
                    apply_effect(body, analysis, &mut state, block, step);
                }
                let at = Location { block, statement_index: data.statements.len() };
                // rustc direction.rs:173. The cleanup edge leaves before the call's return
                // effect; only a switch with edge effects copies the state, once per value.
                match analysis.apply_primary_terminator_effect(&mut state, data.terminator.as_ref().unwrap(), at) {
                    TerminatorEdges::None => {}
                    TerminatorEdges::Single(target) => work.propagate(&mut results, target, &state),
                    TerminatorEdges::Double(target, unwind) => {
                        work.propagate(&mut results, target, &state);
                        work.propagate(&mut results, unwind, &state);
                    }
                    TerminatorEdges::AssignOnReturn { return_, cleanup, place } => {
                        if let Some(cleanup) = cleanup { work.propagate(&mut results, cleanup, &state); }
                        if let Some(target) = return_ {
                            analysis.apply_call_return_effect(&mut state, block, place);
                            work.propagate(&mut results, target, &state);
                        }
                    }
                    TerminatorEdges::SwitchInt { targets, discr } => {
                        if analysis.has_switch_int_edge_effect(block, targets, discr) {
                            for (i, &target) in targets.targets.iter().enumerate() {
                                edge_state.clone_from(&state);
                                analysis.apply_switch_int_edge_effect(&mut edge_state, block, targets, discr, SwitchTargetIndex::Normal(i));
                                work.propagate(&mut results, target, &edge_state);
                            }
                            analysis.apply_switch_int_edge_effect(&mut state, block, targets, discr, SwitchTargetIndex::Otherwise);
                        } else {
                            for &target in &targets.targets { work.propagate(&mut results, target, &state); }
                        }
                        work.propagate(&mut results, targets.otherwise, &state);
                    }
                }
            }
        }
        match work.dirty.first_set_at_or_after(work.curr) {
            Some(rank) => work.curr = rank,
            None => break,
        }
    }
    results
}

/// The dirty blocks by rank, and the least dirty rank: rustc framework/mod.rs:346.
struct Worklist {
    ranks: Vec<usize>,
    dirty: BitSet,
    curr: usize,
    backward: bool,
}

impl Worklist {
    fn propagate(&mut self, results: &mut Results<'_>, target: BasicBlock, state: &BitSet) {
        let i = target.0 as usize;
        let rank = self.ranks[i];
        // A backward analysis meets the unreachable predecessors of reachable blocks and skips
        // them (mod.rs:361); a forward one reaches only reachable blocks.
        if rank == usize::MAX {
            assert!(self.backward, "analysis introduced an unreachable edge");
            return;
        }
        let reached = results.analysis.bottom_is_unreachable() && results.reachable.insert(i);
        let changed = results.analysis.join(&mut results.entry_states[i], state);
        if reached || changed {
            self.dirty.insert(rank);
            self.curr = self.curr.min(rank);
        }
    }
}

#[cfg(test)]
mod tests;
