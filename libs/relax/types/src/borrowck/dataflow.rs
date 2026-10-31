// ported from N:rustc_borrowck/src/dataflow.rs @8d1a764
// NLL and Polonius-next loan scopes. M0 lacks Reborrow
// and semantic inline-asm operands (the existing framework rejects InlineAsm).
use super::borrow_set::{BorrowIndex, BorrowSet};
use super::place_ext::ignore_borrow;
use super::places_conflict::{places_conflict, PlaceConflictBias};
use crate::fxhash::HashMap;
use crate::mir::dataflow::{iterate_to_fixpoint, Analysis, BitSet, EverInitializedPlaces, MaybeUninitializedPlaces, Results, ResultsCursor};
use crate::mir::move_paths::MoveData;
use crate::mir::visit::Visitor;
use crate::mir::*;
use crate::tcx::Tcx;

/// C0a supplies solved NLL region values, indexed by BorrowData::region.
/// Points include each block's terminator at statements.len(); regions must
/// already be renumbered. This is membership, not source-level lexical scope.
pub trait RegionScope {
    fn contains_point(&self, region: Region, location: Location) -> bool;
    /// None selects NLL region membership; Some selects Polonius loan liveness.
    fn is_loan_live_at(&self, _loan: BorrowIndex, _location: Location) -> Option<bool> { None }
}

type ScopeKills = HashMap<(BasicBlock, usize), Vec<BorrowIndex>>;

struct OutOfScopePrecomputer<'a> {
    body: &'a Body,
    regioncx: &'a dyn RegionScope,
    visited: BitSet,
    visit_stack: Vec<Location>,
    borrows_out_of_scope_at_location: ScopeKills,
}
impl OutOfScopePrecomputer<'_> {
    fn compute(body: &Body, regioncx: &dyn RegionScope, borrow_set: &BorrowSet) -> ScopeKills {
        let mut prec = OutOfScopePrecomputer {
            body, regioncx, visited: BitSet::new_empty(body.basic_blocks.len()),
            visit_stack: Vec::new(), borrows_out_of_scope_at_location: HashMap::default(),
        };
        for (index, data) in borrow_set.iter_enumerated() {
            prec.precompute_borrows_out_of_scope(index, data.region, data.reserve_location);
        }
        prec.borrows_out_of_scope_at_location
    }
    fn precompute_borrows_out_of_scope(&mut self, index: BorrowIndex, region: Region, first: Location) {
        // Do not mark the issuing block visited: a backedge must scan its prefix
        // before the reservation too. All subsequent visits start at statement 0.
        self.visit_stack.push(first);
        while let Some(start) = self.visit_stack.pop() {
            let block = &self.body.basic_blocks[start.block.0 as usize];
            let kill = (start.statement_index..=block.statements.len()).find(|&statement_index| {
                let at = Location { block: start.block, statement_index };
                match self.regioncx.is_loan_live_at(index, at) {
                    // Nightly's issuance exception also applies when a backedge
                    // revisits the issuing block: early kills precede generation.
                    Some(live) => at != first && !live,
                    None => !self.regioncx.contains_point(region, at),
                }
            });
            if let Some(statement_index) = kill {
                self.borrows_out_of_scope_at_location.entry((start.block, statement_index)).or_default().push(index);
                continue; // The loan is dead on this path; do not visit successors.
            }
            self.visit_terminator(block.terminator.as_ref().unwrap(), start);
        }
        self.visited.clear();
    }
}
impl Visitor for OutOfScopePrecomputer<'_> {
    fn visit_target(&mut self, target: &BasicBlock, _: bool, _: Location) {
        if self.visited.insert(target.0 as usize) {
            self.visit_stack.push(Location { block: *target, statement_index: 0 });
        }
    }
}

pub struct Borrows<'a> {
    tcx: &'a Tcx,
    body: &'a Body,
    borrow_set: &'a BorrowSet,
    borrows_out_of_scope_at_location: ScopeKills,
}
impl<'a> Borrows<'a> {
    pub fn new(tcx: &'a Tcx, body: &'a Body, regioncx: &dyn RegionScope, borrow_set: &'a BorrowSet) -> Self {
        Self { tcx, body, borrow_set, borrows_out_of_scope_at_location: OutOfScopePrecomputer::compute(body, regioncx, borrow_set) }
    }
    fn kill_loans_out_of_scope_at_location(&self, state: &mut BitSet, location: Location) {
        if let Some(indices) = self.borrows_out_of_scope_at_location.get(&(location.block, location.statement_index)) {
            for &index in indices { state.remove(index); }
        }
    }
    fn kill_borrows_on_place(&self, state: &mut BitSet, place: &Place) {
        let Some(indices) = self.borrow_set.borrows_on_local(place.local) else { return; };
        if place.projection.is_empty() {
            if !matches!(self.body.local_decls[place.local.0 as usize].local_info, LocalInfo::StaticRef { .. }) {
                for &index in indices { state.remove(index); }
            }
            return;
        }
        for &index in indices {
            if places_conflict(self.tcx, self.body, &self.borrow_set[index].borrowed_place, place, PlaceConflictBias::NoOverlap) {
                state.remove(index);
            }
        }
    }
}
impl Analysis for Borrows<'_> {
    fn bottom_value(&self, _: &Body) -> BitSet { BitSet::new_empty(self.borrow_set.len()) }
    fn apply_early_statement_effect(&self, state: &mut BitSet, _: &Statement, at: Location) {
        self.kill_loans_out_of_scope_at_location(state, at);
    }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, stmt: &Statement, at: Location) {
        match &stmt.kind {
            StatementKind::Assign(lhs, rhs) => {
                if let Rvalue::Ref(_, _, place) = rhs {
                    if ignore_borrow(place, self.tcx, self.body, self.borrow_set.locals_state_at_exit()) { return; }
                    for &index in self.borrow_set.borrows_at_location(&at).expect("missing borrow at reservation") {
                        state.insert(index);
                    }
                }
                // Generation precedes overwrite kills, but follows early NLL kills.
                self.kill_borrows_on_place(state, lhs);
            }
            StatementKind::StorageDead(local) => self.kill_borrows_on_place(state, &Place::local(*local)),
            StatementKind::FakeRead(..) | StatementKind::SetDiscriminant { .. }
            | StatementKind::StorageLive(..) | StatementKind::PlaceMention(..)
            | StatementKind::AscribeUserType(..) | StatementKind::Intrinsic(..)
            | StatementKind::ConstEvalCounter | StatementKind::Nop | StatementKind::BackwardIncompatibleDropHint { .. } => {}
        }
    }
    fn apply_early_terminator_effect(&self, state: &mut BitSet, _: &Terminator, at: Location) {
        self.kill_loans_out_of_scope_at_location(state, at);
    }
    // Ordinary terminators and call-return edges do not kill borrows in nightly.
    // The default primary effect supplies CFG edges and rejects unsupported asm.
}

/// Solve the three domains separately, as nightly does, using M4a's engine.
pub struct Borrowck<'a> {
    pub borrows: Borrows<'a>,
    pub uninits: MaybeUninitializedPlaces<'a>,
    pub ever_inits: EverInitializedPlaces<'a>,
}
impl<'a> Borrowck<'a> {
    pub fn new(prog: &'a crate::program::Program, tcx: &'a Tcx, body: &'a Body, regions: &dyn RegionScope, borrow_set: &'a BorrowSet, moves: &'a MoveData) -> Self {
        Self {
            borrows: Borrows::new(tcx, body, regions, borrow_set),
            uninits: MaybeUninitializedPlaces::new(prog, tcx, body, moves),
            ever_inits: EverInitializedPlaces::new(body, moves),
        }
    }
    pub fn iterate_to_fixpoint(&self) -> BorrowckResults<'_> {
        let body = self.borrows.body;
        BorrowckResults {
            borrows: iterate_to_fixpoint(body, &self.borrows),
            uninits: iterate_to_fixpoint(body, &self.uninits),
            ever_inits: iterate_to_fixpoint(body, &self.ever_inits),
        }
    }
}
pub struct BorrowckResults<'a> {
    pub borrows: Results<'a>,
    pub uninits: Results<'a>,
    pub ever_inits: Results<'a>,
}
impl BorrowckResults<'_> {
    pub fn cursor(&self) -> BorrowckCursor<'_, '_> {
        BorrowckCursor { borrows: self.borrows.cursor(), uninits: self.uninits.cursor(), ever_inits: self.ever_inits.cursor() }
    }
}

/// A borrowed view of the transient state; no extra bitset copies per point.
/// uninits indexes MovePaths; ever_inits indexes Init records, not MovePaths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BorrowckDomain<'a> {
    pub borrows: &'a BitSet,
    pub uninits: &'a BitSet,
    pub ever_inits: &'a BitSet,
}
pub struct BorrowckCursor<'r, 'a> {
    borrows: ResultsCursor<'r, 'a>,
    uninits: ResultsCursor<'r, 'a>,
    ever_inits: ResultsCursor<'r, 'a>,
}
impl BorrowckCursor<'_, '_> {
    pub fn seek_before_primary_effect(&mut self, at: Location) {
        self.borrows.seek_before_primary_effect(at);
        self.uninits.seek_before_primary_effect(at);
        self.ever_inits.seek_before_primary_effect(at);
    }
    pub fn seek_after_primary_effect(&mut self, at: Location) {
        self.borrows.seek_after_primary_effect(at);
        self.uninits.seek_after_primary_effect(at);
        self.ever_inits.seek_after_primary_effect(at);
    }
    pub fn get(&self) -> BorrowckDomain<'_> {
        BorrowckDomain { borrows: self.borrows.get(), uninits: self.uninits.get(), ever_inits: self.ever_inits.get() }
    }
}
