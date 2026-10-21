//! The lint `tail_expr_drop_order` (rustc_mir_transform lint_tail_expr_drop_order.rs): values a block's
//! tail expression leaves in temporaries are dropped after the block's locals up to edition 2021 and
//! before them from edition 2024. The MIR builder marks the places whose drop moves
//! (`BackwardIncompatibleDropHint`); this pass finds the locals dropped between such a hint and the
//! real drop of its place and reports the reordering. Runs on the promoted MIR of a checked body, in
//! `makepad check` only.
use crate::lint::{self, Lint};
use crate::mir::dataflow::{iterate_to_fixpoint, BitSet, MaybeInitializedPlaces};
use crate::mir::move_paths::{LookupResult, MoveData};
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::significant_drop::SigDrop;
use crate::tcx::{Tcx, TypingEnv};
use crate::types::TyId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub const TAIL_EXPR_DROP_ORDER: Lint = lint::lint("tail_expr_drop_order");

fn place_has_common_prefix(left: &Place, right: &Place) -> bool {
    left.local == right.local && left.projection.iter().zip(right.projection.iter()).all(|(l, r)| l == r)
}

/// Cache entry of `drop` at a `BasicBlock`.
#[derive(Clone, Copy)]
enum DropAtBlock {
    /// nothing is known yet
    Unknown,
    /// the `drop` here has no effect
    None,
    /// the `drop` here will invoke a destructor
    Some(usize),
}

struct DropsReachable<'a, 'r> {
    body: &'a Body,
    place: &'a Place,
    drop_span: &'a mut Option<Span>,
    move_data: &'a MoveData,
    results: &'a crate::mir::dataflow::Results<'r>,
    block_drop_value_info: &'a mut Vec<DropAtBlock>,
    collected_drops: &'a mut BitSet,
    visited: HashMap<BasicBlock, Rc<RefCell<BitSet>>>,
}

impl DropsReachable<'_, '_> {
    fn visit(&mut self, block: BasicBlock) {
        let size = self.move_data.move_paths.len();
        let data = &self.body.basic_blocks[block.0 as usize];
        let Some(terminator) = &data.terminator else { return };
        // The drops observed at `block` so far; an occupied entry says `block` was visited.
        let dropped_local_here = Rc::clone(self.visited.entry(block).or_insert_with(|| Rc::new(RefCell::new(BitSet::new_empty(size)))));
        match self.block_drop_value_info[block.0 as usize] {
            DropAtBlock::Some(dropped) => {
                dropped_local_here.borrow_mut().insert(dropped);
            }
            DropAtBlock::Unknown => {
                if let TerminatorKind::Drop { place, .. } = &terminator.kind {
                    if let LookupResult::Exact(idx) | LookupResult::Parent(Some(idx)) = self.move_data.rev_lookup.find(place) {
                        // A `drop` terminator at this stage does not mean the drop happens: drop
                        // elaboration decides, so ask whether the place may be initialized here.
                        let maybe = if self.results.is_reachable(block) {
                            let mut cursor = self.results.cursor();
                            cursor.seek_before_primary_effect(Location { block, statement_index: data.statements.len() });
                            cursor.get().contains(idx)
                        } else {
                            false
                        };
                        if maybe {
                            self.block_drop_value_info[block.0 as usize] = DropAtBlock::Some(idx);
                            dropped_local_here.borrow_mut().insert(idx);
                        } else {
                            self.block_drop_value_info[block.0 as usize] = DropAtBlock::None;
                        }
                    }
                }
            }
            DropAtBlock::None => {}
        }
        for succ in crate::mir::transform::simplify::successors(&terminator.kind) {
            let target = &self.body.basic_blocks[succ.0 as usize];
            if target.is_cleanup {
                continue;
            }
            // as long as a new block or new dropped places arrive, go on with `succ`
            let dropped_local_there = match self.visited.get(&succ) {
                Some(there) => {
                    let there = Rc::clone(there);
                    if succ == block || !there.borrow_mut().union(&dropped_local_here.borrow()) {
                        continue;
                    }
                    there
                }
                None => {
                    let fresh = Rc::new(RefCell::new(dropped_local_here.borrow().clone()));
                    self.visited.insert(succ, Rc::clone(&fresh));
                    fresh
                }
            };
            if let Some(Terminator { kind: TerminatorKind::Drop { place: dropped_place, .. }, source_info }) = &target.terminator {
                if place_has_common_prefix(dropped_place, self.place) {
                    // the current drop of `place` is reached: check the observed drops in
                    self.collected_drops.union(&dropped_local_there.borrow());
                    if self.drop_span.is_none() {
                        *self.drop_span = Some(source_info.span);
                    }
                    // a simple path from a future drop point to the current one: stop here
                    continue;
                }
            }
            self.visit(succ);
        }
    }
}

/// A moved place at `idx` that is a part of a BID: its drop counts as the drop of the BID itself.
fn place_descendent_of_bids(mut idx: usize, move_data: &MoveData, bids: &[&Place]) -> bool {
    loop {
        let path = &move_data.move_paths[idx];
        if bids.iter().any(|b| **b == path.place) {
            return true;
        }
        match path.parent {
            Some(parent) => idx = parent,
            None => return false,
        }
    }
}

/// The core of the lint (rustc `run_lint`). `owner` is the item the body belongs to, `closure` whether
/// the body is a closure's.
pub fn run_lint(prog: &Program, tcx: &Tcx, env: TypingEnv, owner: DefId, closure: bool, body: &Body) {
    if !lint::enabled() {
        return;
    }
    let krate = prog.def(owner).krate;
    if prog.crates[krate as usize].edition >= 2024 || !prog.crates[krate as usize].validate {
        return;
    }
    let sig = SigDrop { prog, tcx, env };
    // The places with a backwards-incompatible drop (BID), per block.
    let mut bid_per_block: Vec<(BasicBlock, Vec<&Place>)> = Vec::new();
    let mut bid_places: Vec<&Place> = Vec::new();
    let mut components: HashMap<TyId, Vec<TyId>> = HashMap::new();
    let components_of = |ty: TyId, components: &mut HashMap<TyId, Vec<TyId>>| -> bool {
        components.entry(ty).or_insert_with(|| sig.extract_component_with_significant_dtor(ty)).is_empty()
    };
    for (b, data) in body.basic_blocks.iter().enumerate() {
        for stmt in &data.statements {
            if let StatementKind::BackwardIncompatibleDropHint { place, .. } = &stmt.kind {
                let Some(pty) = place.ty(body, tcx) else { continue };
                if components_of(pty.ty, &mut components) {
                    continue;
                }
                let block = BasicBlock(b as u32);
                match bid_per_block.iter_mut().find(|(bb, _)| *bb == block) {
                    Some((_, v)) => v.push(place),
                    None => bid_per_block.push((block, vec![place])),
                }
                bid_places.push(place);
            }
        }
    }
    if bid_per_block.is_empty() {
        return;
    }
    let names = collect_user_names(body);
    // "maybe initialized": only a drop of a place that may be initialized counts
    let move_data = MoveData::gather_moves(body, prog, tcx, env, &|_| true);
    let analysis = MaybeInitializedPlaces::new(prog, tcx, body, &move_data);
    let results = iterate_to_fixpoint(body, &analysis);
    let mut block_drop_value_info = vec![DropAtBlock::Unknown; body.basic_blocks.len()];
    for (block, candidates) in &bid_per_block {
        // the locals dropped between the BID points and their real drops
        let mut all_locals_dropped = BitSet::new_empty(move_data.move_paths.len());
        let mut drop_span: Option<Span> = None;
        for &place in candidates {
            let mut collected_drops = BitSet::new_empty(move_data.move_paths.len());
            DropsReachable {
                body,
                place,
                drop_span: &mut drop_span,
                move_data: &move_data,
                results: &results,
                block_drop_value_info: &mut block_drop_value_info,
                collected_drops: &mut collected_drops,
                visited: HashMap::new(),
            }
            .visit(*block);
            all_locals_dropped.union(&collected_drops);
        }
        // exclude some local bindings
        {
            let mut to_exclude = BitSet::new_empty(all_locals_dropped.domain_size());
            for path_idx in all_locals_dropped.iter() {
                let move_path = &move_data.move_paths[path_idx];
                let dropped_local = move_path.place.local;
                // a) the return value `_0` will eventually be used
                if dropped_local == Local(0) {
                    to_exclude.insert(path_idx);
                    continue;
                }
                // b) a closure's captures are still dropped last
                if closure && dropped_local == Local(1) {
                    to_exclude.insert(path_idx);
                    continue;
                }
                // c) projections into the BID locals are considered dropped now
                if place_descendent_of_bids(path_idx, &move_data, &bid_places) {
                    to_exclude.insert(path_idx);
                    continue;
                }
                // d) the collected local has no custom destructor that passes the ecosystem filter
                let Some(observer_ty) = move_path.place.ty(body, tcx) else { continue };
                if components_of(observer_ty.ty, &mut components) {
                    to_exclude.insert(path_idx);
                    continue;
                }
            }
            // all BIDs into one local: leave that local out, to focus on the others
            let first = candidates[0].local;
            if candidates.iter().all(|p| p.local == first) {
                for path_idx in all_locals_dropped.iter() {
                    if move_data.move_paths[path_idx].place.local == first {
                        to_exclude.insert(path_idx);
                    }
                }
            }
            all_locals_dropped.subtract(&to_exclude);
        }
        if all_locals_dropped.is_empty() {
            continue;
        }
        let place = candidates[0];
        let decl = &body.local_decls[place.local.0 as usize];
        let span = decl.source_info.span;
        let _ = (&names, drop_span);
        if lint::external(prog, span.file, span.lo) {
            continue;
        }
        lint::emit(prog, owner, &TAIL_EXPR_DROP_ORDER, span.file, span.lo, "relative drop order changing in Rust 2024");
    }
}

/// Binding names, where the body's debug info has them.
fn collect_user_names(body: &Body) -> HashMap<Local, String> {
    let mut names = HashMap::new();
    for info in &body.var_debug_info {
        if info.place.projection.is_empty() || matches!(info.place.projection.first(), Some(PlaceElem::Deref)) {
            names.entry(info.place.local).or_insert_with(|| info.name.clone());
        }
    }
    names
}
