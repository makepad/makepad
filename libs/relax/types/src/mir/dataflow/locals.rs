//! Local-only analyses from rustc_mir_dataflow/src/impls/{borrowed_locals,
//! liveness,storage_liveness}.rs. No type metadata or move-path approximations.
use super::*;
use crate::mir::visit::{Visitor, PlaceContext, MutatingUseContext as W, NonMutatingUseContext as R};
use PlaceContext::{MutatingUse as Write, NonMutatingUse as Read, NonUse};

/// rustc_middle/src/mir/statement.rs:505; any dereference makes the place indirect.
fn is_indirect(place: &Place) -> bool {
    for elem in &place.projection {
        if matches!(elem, PlaceElem::Deref) { return true; }
    }
    false
}

/// rustc_mir_dataflow/src/impls/borrowed_locals.rs:21,64. Fake borrows are
/// excluded, so this is a limited alias analysis, not borrowck's borrow tracking.
pub struct MaybeBorrowedLocals;

impl Analysis for MaybeBorrowedLocals {
    fn bottom_value(&self, body: &Body) -> BitSet { BitSet::new_empty(body.local_decls.len()) }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, stmt: &Statement, at: Location) {
        BorrowTransfer(state).visit_statement(stmt, at);
    }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, at: Location) -> TerminatorEdges<'a> {
        BorrowTransfer(state).visit_terminator(term, at);
        TerminatorEdges::for_terminator(term)
    }
}

struct BorrowTransfer<'a>(&'a mut BitSet);
impl Visitor for BorrowTransfer<'_> {
    fn visit_statement(&mut self, stmt: &Statement, at: Location) {
        self.super_statement(stmt, at);
        if let StatementKind::StorageDead(local) = stmt.kind { self.0.remove(local.0 as usize); }
    }
    fn visit_rvalue(&mut self, rv: &Rvalue, at: Location) {
        self.super_rvalue(rv, at);
        match rv {
            Rvalue::RawPtr(_, place) | Rvalue::Ref(_, BorrowKind::Shared | BorrowKind::Mut { .. }, place) => {
                if !is_indirect(place) { self.0.insert(place.local.0 as usize); }
            }
            Rvalue::Ref(_, BorrowKind::Fake(_), _) | Rvalue::Use(_) | Rvalue::Repeat(..)
            | Rvalue::ThreadLocalRef(_) | Rvalue::Cast(..) | Rvalue::BinaryOp(..)
            | Rvalue::UnaryOp(..) | Rvalue::Discriminant(_) | Rvalue::Aggregate(..)
            | Rvalue::CopyForDeref(_) => {}
        }
    }
    fn visit_terminator(&mut self, term: &Terminator, at: Location) {
        self.super_terminator(term, at);
        // Drop glue can expose &mut self through a raw pointer (rustc borrowed_locals.rs:106).
        if let TerminatorKind::Drop { place, .. } = &term.kind {
            if !is_indirect(place) { self.0.insert(place.local.0 as usize); }
        }
    }
}

/// rustc_mir_dataflow/src/impls/liveness.rs:34,96,163. Tracks direct uses, not
/// accesses via existing references; pair with MaybeBorrowedLocals when needed.
pub struct MaybeLiveLocals;
impl Analysis for MaybeLiveLocals {
    fn direction(&self) -> Direction { Direction::Backward }
    fn bottom_value(&self, body: &Body) -> BitSet { BitSet::new_empty(body.local_decls.len()) }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, stmt: &Statement, at: Location) {
        LiveTransfer(state).visit_statement(stmt, at);
    }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, at: Location) -> TerminatorEdges<'a> {
        LiveTransfer(state).visit_terminator(term, at);
        TerminatorEdges::for_terminator(term)
    }
    fn apply_call_return_effect(&self, state: &mut BitSet, _block: BasicBlock, places: CallReturnPlaces<'_>) {
        match places {
            CallReturnPlaces::Call(place) => {
                if place.projection.is_empty() { state.remove(place.local.0 as usize); }
            }
            CallReturnPlaces::Yield(place) => {
                apply_def_use(state, place, Write(W::Yield));
                LiveTransfer(state).projection(place, Write(W::Yield), Location::default());
            }
        }
    }
}

enum DefUse { Def, Use, PartialWrite, NonUse }
fn def_use(place: &Place, context: PlaceContext) -> DefUse {
    match context {
        NonUse(_) => DefUse::NonUse,
        Write(W::Call | W::Yield | W::AsmOutput | W::Store) => {
            if is_indirect(place) { DefUse::Use }
            else if place.projection.is_empty() { DefUse::Def }
            else { DefUse::PartialWrite }
        }
        Write(W::SetDiscriminant) => {
            if is_indirect(place) { DefUse::Use } else { DefUse::PartialWrite }
        }
        Write(W::RawBorrow | W::Borrow | W::Drop | W::Retag)
        | Read(R::RawBorrow | R::Copy | R::Inspect | R::Move | R::PlaceMention | R::FakeBorrow | R::SharedBorrow) => DefUse::Use,
        Write(W::Projection) | Read(R::Projection) => unreachable!("projection base must be handled as a place"),
    }
}
fn apply_def_use(state: &mut BitSet, place: &Place, context: PlaceContext) {
    match def_use(place, context) {
        DefUse::Def => { state.remove(place.local.0 as usize); }
        DefUse::Use => { state.insert(place.local.0 as usize); }
        DefUse::PartialWrite | DefUse::NonUse => {}
    }
}

struct LiveTransfer<'a>(&'a mut BitSet);
impl LiveTransfer<'_> {
    fn projection(&mut self, place: &Place, context: PlaceContext, at: Location) {
        let mut i = place.projection.len();
        while i != 0 {
            i -= 1;
            self.visit_projection_elem(&place.projection[i], context, at);
        }
    }
}
impl Visitor for LiveTransfer<'_> {
    fn visit_place(&mut self, place: &Place, context: PlaceContext, at: Location) {
        if context == Write(W::Yield) { return; }
        match def_use(place, context) {
            DefUse::Def => {
                if !matches!(context, Write(W::Call | W::AsmOutput)) { self.0.remove(place.local.0 as usize); }
            }
            DefUse::Use => { self.0.insert(place.local.0 as usize); }
            DefUse::PartialWrite | DefUse::NonUse => {}
        }
        self.projection(place, context, at);
    }
    fn visit_local(&mut self, local: &Local, context: PlaceContext, _at: Location) {
        apply_def_use(self.0, &Place::local(*local), context);
    }
}

/// rustc_mir_dataflow/src/impls/storage_liveness.rs:13. Absence of either storage
/// annotation gives a local fixed storage for the whole body, including unreachable blocks.
pub fn always_storage_live_locals(body: &Body) -> BitSet {
    let mut live = BitSet::new_filled(body.local_decls.len());
    for block in &body.basic_blocks {
        for stmt in &block.statements {
            if let StatementKind::StorageLive(local) | StatementKind::StorageDead(local) = stmt.kind {
                live.remove(local.0 as usize);
            }
        }
    }
    live
}

/// rustc_mir_dataflow/src/impls/storage_liveness.rs:38.
pub struct MaybeStorageLive { always_live: BitSet }
impl MaybeStorageLive {
    pub fn new(body: &Body) -> Self { Self { always_live: always_storage_live_locals(body) } }
}
impl Analysis for MaybeStorageLive {
    fn bottom_value(&self, body: &Body) -> BitSet { BitSet::new_empty(body.local_decls.len()) }
    fn initialize_start_block(&self, body: &Body, state: &mut BitSet) {
        state.union(&self.always_live);
        for i in 1..=body.arg_count as usize { state.insert(i); }
    }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, stmt: &Statement, _at: Location) {
        match stmt.kind {
            StatementKind::StorageLive(local) => { state.insert(local.0 as usize); }
            StatementKind::StorageDead(local) => { state.remove(local.0 as usize); }
            _ => {}
        }
    }
}

/// rustc_mir_dataflow/src/impls/storage_liveness.rs:80. This is an independent
/// may analysis, not the complement of MaybeStorageLive at control-flow joins.
pub struct MaybeStorageDead { always_live: BitSet }
impl MaybeStorageDead {
    pub fn new(body: &Body) -> Self { Self { always_live: always_storage_live_locals(body) } }
}
impl Analysis for MaybeStorageDead {
    fn bottom_value(&self, body: &Body) -> BitSet { BitSet::new_empty(body.local_decls.len()) }
    fn initialize_start_block(&self, body: &Body, state: &mut BitSet) {
        assert_eq!(body.local_decls.len(), self.always_live.domain_size());
        for i in body.arg_count as usize + 1..body.local_decls.len() {
            if !self.always_live.contains(i) { state.insert(i); }
        }
    }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, stmt: &Statement, _at: Location) {
        match stmt.kind {
            StatementKind::StorageLive(local) => { state.remove(local.0 as usize); }
            StatementKind::StorageDead(local) => { state.insert(local.0 as usize); }
            _ => {}
        }
    }
}

/// rustc_mir_dataflow/src/impls/storage_liveness.rs:109. Whether each local requires storage at a
/// location, i.e. whether its storage can go away without being observed.
pub struct MaybeRequiresStorage<'r, 'a> { borrowed_locals: std::cell::RefCell<ResultsCursor<'r, 'a>> }
impl<'r, 'a> MaybeRequiresStorage<'r, 'a> {
    pub fn new(borrowed_locals: ResultsCursor<'r, 'a>) -> Self { Self { borrowed_locals: std::cell::RefCell::new(borrowed_locals) } }
    /// Kill locals that are fully moved and have not been borrowed.
    fn check_for_move(&self, state: &mut BitSet, at: Location) {
        let mut borrowed = self.borrowed_locals.borrow_mut();
        let body = borrowed.body();
        let mut visitor = MoveVisitor { state, borrowed_locals: &mut borrowed };
        let block = &body.basic_blocks[at.block.0 as usize];
        match block.statements.get(at.statement_index) {
            Some(stmt) => visitor.visit_statement(stmt, at),
            None => visitor.visit_terminator(block.terminator.as_ref().expect("terminator"), at),
        }
    }
}
struct MoveVisitor<'s, 'b, 'r, 'a> { borrowed_locals: &'b mut ResultsCursor<'r, 'a>, state: &'s mut BitSet }
impl Visitor for MoveVisitor<'_, '_, '_, '_> {
    fn visit_local(&mut self, local: &Local, context: PlaceContext, at: Location) {
        if context == Read(R::Move) {
            self.borrowed_locals.seek_before_primary_effect(at);
            if !self.borrowed_locals.get().contains(local.0 as usize) { self.state.remove(local.0 as usize); }
        }
    }
}
impl Analysis for MaybeRequiresStorage<'_, '_> {
    fn bottom_value(&self, body: &Body) -> BitSet { BitSet::new_empty(body.local_decls.len()) }
    fn initialize_start_block(&self, body: &Body, state: &mut BitSet) {
        // The resume argument is live on function entry (the `self` argument is not cared about).
        for arg in 2..=body.arg_count as usize { state.insert(arg); }
    }
    fn apply_early_statement_effect(&self, state: &mut BitSet, stmt: &Statement, at: Location) {
        // If a place is borrowed in a statement, it needs storage for that statement.
        BorrowTransfer(state).visit_statement(stmt, at);
        match &stmt.kind {
            StatementKind::StorageDead(l) => { state.remove(l.0 as usize); }
            // If a place is assigned to in a statement, it needs storage for that statement.
            StatementKind::Assign(place, _) | StatementKind::SetDiscriminant { place, .. } => { state.insert(place.local.0 as usize); }
            _ => {}
        }
    }
    fn apply_primary_statement_effect(&self, state: &mut BitSet, _stmt: &Statement, at: Location) {
        // If we move from a place then it only stops needing storage *after* that statement.
        self.check_for_move(state, at);
    }
    fn apply_early_terminator_effect(&self, state: &mut BitSet, term: &Terminator, at: Location) {
        // If a place is borrowed in a terminator, it needs storage for that terminator.
        BorrowTransfer(state).visit_terminator(term, at);
        // (a `Yield`'s resume argument is not generated: it is written only when the coroutine is
        // resumed, so it needs storage after the yield, not before)
        if let TerminatorKind::Call { destination, .. } = &term.kind { state.insert(destination.local.0 as usize); }
    }
    fn apply_primary_terminator_effect<'a>(&self, state: &mut BitSet, term: &'a Terminator, at: Location) -> TerminatorEdges<'a> {
        // A call's destination requires storage for the call and after it returns successfully, but not
        // after a panic: kill it here and generate it again in the call return effect.
        if let TerminatorKind::Call { destination, .. } = &term.kind { state.remove(destination.local.0 as usize); }
        self.check_for_move(state, at);
        TerminatorEdges::for_terminator(term)
    }
    fn apply_call_return_effect(&self, state: &mut BitSet, _block: BasicBlock, places: CallReturnPlaces<'_>) {
        match places { CallReturnPlaces::Call(place) | CallReturnPlaces::Yield(place) => { state.insert(place.local.0 as usize); } }
    }
}
