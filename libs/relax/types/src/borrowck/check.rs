// ported from N:rustc_borrowck/src/lib.rs @8d1a764 (MirBorrowckCtxt)
use super::borrow_set::{BorrowData, BorrowSet, LocalsStateAtExit};
use super::borrowck_errors::{BorrowckError, ErrorKind};
use super::dataflow::{Borrowck, BorrowckDomain};
use super::mir_nll::MirRegionResult;
use super::path_utils::{each_borrow_involving_path, is_active};
use super::places_conflict::{borrow_conflicts_with_place, AccessDepth, ArtificialField, PlaceConflictBias};
use super::prefixes::{prefixes, PrefixSet};
use crate::mir::dataflow::BitSet;
use crate::mir::move_paths::{InitKind, LookupResult, MoveData};
use crate::mir::visit::Visitor;
use crate::mir::*;
use crate::program::Program;
use crate::tcx::Tcx;
use crate::types::TyKind;
use std::ops::ControlFlow;
use AccessDepth::{Deep, Shallow};
use ErrorKind::*;

#[derive(Debug)]
pub struct MirBorrowckResult {
    pub errors: Vec<BorrowckError>,
    pub used_mut: BitSet,
    /// Missing MIR contracts are a stopped check, never an acceptance verdict.
    pub unsupported: Vec<&'static str>,
    pub region_errors: Vec<super::region_infer::RegionErrorKind>,
    pub tainted_by_errors: bool,
}
impl MirBorrowckResult {
    pub fn accepted(&self) -> bool { self.errors.is_empty() && self.unsupported.is_empty() && self.region_errors.is_empty() && !self.tainted_by_errors }
}

/// Check one typed, renumbered analysis MIR body against its solved regions.
/// MoveData is the same body-owned, Copy-filtered arena used by MIR type checking.
/// C-NLL5 supplies it and the region input; this entry never invokes BIR borrowck.
pub fn mir_borrowck(tcx: &Tcx, prog: &Program, body: &Body, regions: &mut MirRegionResult, moves: &MoveData) -> MirBorrowckResult {
    let nll = check_with_scopes(prog, tcx, body, regions, moves);
    if !nll.unsupported.is_empty() { return nll; }
    let verify = nll.accepted() && verify_polonius_sample();
    if nll.accepted() && !verify { return nll; }
    *regions = compute_polonius_regions(tcx, prog, body, regions, moves);
    let polonius = check_with_scopes(prog, tcx, body, regions, moves);
    if verify { assert!(polonius.accepted(), "Polonius must accept NLL-accepted MIR: {polonius:?}"); }
    polonius
}

// Off by default. A rate of N checks every Nth NLL-accepted body (1 = all).
// Atomic process-local counting needs neither TLS nor a crate-mode input.
fn verify_polonius_sample() -> bool {
    static RATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let rate = *RATE.get_or_init(|| std::env::var("MAKEPAD_RUN_POLONIUS_VERIFY").ok().and_then(|s| s.parse().ok()).unwrap_or(0));
    rate != 0 && COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % rate == 0
}
#[cfg(test)]
pub(super) fn check_polonius(tcx: &Tcx, prog: &Program, body: &Body, regions: &MirRegionResult, moves: &MoveData) -> MirBorrowckResult {
    check_with_scopes(prog, tcx, body, &compute_polonius_regions(tcx, prog, body, regions, moves), moves)
}
fn compute_polonius_regions(tcx: &Tcx, prog: &Program, body: &Body, regions: &MirRegionResult, moves: &MoveData) -> MirRegionResult {
    let invalidated = matches!(body.source.kind, BodyKind::Fn | BodyKind::Closure(_) | BodyKind::Shim(_));
    let borrows = BorrowSet::build(tcx, body, invalidated, moves);
    let liveness = regions.polonius.compute_loan_liveness(tcx, prog, body, moves, &regions.locations, &borrows, &regions.regioncx);
    let regioncx = regions.regioncx.with_liveness(tcx, prog, liveness, regions.locations.num_points(), &regions.type_tests, regions.nested);
    MirRegionResult { regioncx, locations: regions.locations.clone(), type_tests: regions.type_tests.clone(),
        polonius: regions.polonius.clone(), nested: regions.nested, tainted_by_errors: regions.tainted_by_errors }
}

pub(super) fn check_with_scopes(prog: &Program, tcx: &Tcx, body: &Body, regions: &MirRegionResult, moves: &MoveData) -> MirBorrowckResult {
    assert!(regions.locations.matches(body), "region locations belong to another MIR layout");
    let mut result = MirBorrowckResult { errors: vec![], used_mut: BitSet::new_empty(body.local_decls.len()),
        unsupported: vec![], region_errors: regions.regioncx.errors.clone(), tainted_by_errors: regions.tainted_by_errors };
    check_supported(tcx, body, &mut result.unsupported);
    if !result.unsupported.is_empty() { return result; }
    let invalidated = matches!(body.source.kind, BodyKind::Fn | BodyKind::Closure(_) | BodyKind::Shim(_));
    let borrows = BorrowSet::build(tcx, body, invalidated, moves);
    let analysis = Borrowck::new(prog, tcx, body, regions, &borrows, moves);
    let results = analysis.iterate_to_fixpoint();
    let mut cursor = results.cursor();
    let mut cx = MirBorrowckCtxt { tcx, body, moves, borrows: &borrows, cfg: CheckCfg::new(body), result,
        access_errors: vec![], reservation_errors: vec![], uninitialized_errors: BitSet::new_empty(body.local_decls.len()),
        move_errors: vec![], buffered_move_errors: vec![], buffered_mut_errors: vec![] };
    for b in cx.cfg.reverse_postorder.clone() {
        let block = &body.basic_blocks[b];
        let block_id = BasicBlock(b as u32);
        if !results.borrows.is_reachable(block_id) { continue; }
        for (statement_index, stmt) in block.statements.iter().enumerate() {
            let at = Location { block: block_id, statement_index };
            cursor.seek_before_primary_effect(at); // after the early NLL kills
            cx.visit_statement(stmt, at, cursor.get());
        }
        let at = Location { block: block_id, statement_index: block.statements.len() };
        let term = block.terminator.as_ref().expect("unfinished MIR block");
        cursor.seek_before_primary_effect(at);
        cx.visit_terminator(term, at, cursor.get());
        cursor.seek_after_primary_effect(at);
        cx.visit_after_primary_terminator_effect(term, at, cursor.get());
    }
    // lib.rs:567 report_move_errors, then diagnostics/mod.rs:112 emit_errors: the
    // de-duplicated use-after-move errors are buffered until the walk is done.
    let move_errors = std::mem::take(&mut cx.move_errors);
    report_move_errors(body, Some(moves), move_errors, &mut cx.result.errors);
    for (_, _, error) in std::mem::take(&mut cx.buffered_move_errors) {
        if !cx.result.errors.contains(&error) { cx.result.errors.push(error); }
    }
    super::used_muts::gather_used_muts(body, moves, &mut cx.result.used_mut);
    check_promoted_moves(tcx, body, &mut cx.result);
    cx.result
}


fn check_supported(tcx: &Tcx, body: &Body, unsupported: &mut Vec<&'static str>) {
    struct Check<'a> { tcx: &'a Tcx, body: &'a Body, stops: &'a mut Vec<&'static str> }
    impl Check<'_> { fn stop(&mut self, s: &'static str) { if !self.stops.contains(&s) { self.stops.push(s); } } }
    impl Visitor for Check<'_> {
        fn visit_place(&mut self, place: &Place, context: crate::mir::visit::PlaceContext, at: Location) {
            let mut ty = PlaceTy::from_ty(self.body.local_decls[place.local.0 as usize].ty);
            for elem in &place.projection {
                if matches!(elem, PlaceElem::Subtype(_)) { self.stop("Subtype must be removed before borrowck"); return; }
                if matches!(elem, PlaceElem::Field(..)) && matches!(self.tcx.tys.kind(ty.ty), TyKind::Closure(..)) {
                    self.stop("closure field access needs MIR capture metadata"); return;
                }
                let Some(next) = ty.projection_ty(self.tcx, elem) else { self.stop("MIR projected type is unavailable"); return; };
                ty = next;
            }
            self.super_place(place, context, at);
        }
        fn visit_rvalue(&mut self, rv: &Rvalue, at: Location) {
            match rv {
                Rvalue::Aggregate(kind, _) if matches!(**kind, AggregateKind::Closure(..) | AggregateKind::Coroutine(..)) =>
                    self.stop("closure aggregate needs nested used-mut capture metadata"),
                Rvalue::CopyForDeref(_) | Rvalue::ShallowInitBox(..) => self.stop("runtime rvalue in borrowck MIR"),
                _ => {}
            }
            self.super_rvalue(rv, at);
        }
        fn visit_statement(&mut self, stmt: &Statement, at: Location) {
            if matches!(stmt.kind, StatementKind::Nop | StatementKind::SetDiscriminant { .. }
                | StatementKind::Intrinsic(NonDivergingIntrinsic::CopyNonOverlapping { .. })) {
                self.stop("statement is not allowed in nightly borrowck MIR phase");
            }
            self.super_statement(stmt, at);
        }
        fn visit_terminator(&mut self, term: &Terminator, at: Location) {
            if matches!(term.kind, TerminatorKind::InlineAsm(_)) { self.stop("MIR has no inline-asm operands"); }
            if matches!(term.kind, TerminatorKind::Yield { .. }) { self.stop("Yield needs coroutine movability metadata"); }
            self.super_terminator(term, at);
        }
    }
    let mut check = Check { tcx, body, stops: unsupported };
    for (b, block) in body.basic_blocks.iter().enumerate() { check.visit_basic_block_data(BasicBlock(b as u32), block); }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadKind { Copy, Borrow(BorrowKind) }
#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteKind { MutableBorrow(BorrowKind), Mutate, Move, StorageDeadOrDrop, Replace }
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadOrWrite { Read(ReadKind), Write(WriteKind), Reservation(WriteKind), Activation(WriteKind, usize) }
use ReadOrWrite::*;

struct MirBorrowckCtxt<'a> {
    tcx: &'a Tcx, body: &'a Body, moves: &'a MoveData, borrows: &'a BorrowSet,
    cfg: CheckCfg, result: MirBorrowckResult,
    access_errors: Vec<(Place, Span)>, reservation_errors: Vec<Place>, uninitialized_errors: BitSet,
    /// lib.rs:779 move_errors: illegal moves, grouped and reported after the walk.
    move_errors: Vec<MoveError>,
    /// diagnostics/mod.rs:143 buffered_move_errors: one E0382 per set of move-outs, in
    /// first-insertion order; a later error for the same set replaces the earlier one.
    buffered_move_errors: Vec<(Vec<usize>, Place, BorrowckError)>,
    /// diagnostics/mod.rs:164 buffered_mut_errors: (binding span, index into errors, count).
    buffered_mut_errors: Vec<(Span, usize, usize)>,
}
impl MirBorrowckCtxt<'_> {
    fn error(&mut self, kind: ErrorKind, span: Span) {
        let error = BorrowckError { kind, span };
        if !self.result.errors.contains(&error) { self.result.errors.push(error); }
    }
    fn span_at(&self, at: Location) -> Span {
        let block = &self.body.basic_blocks[at.block.0 as usize];
        block.statements.get(at.statement_index).map_or_else(|| block.terminator.as_ref().unwrap().source_info.span, |s| s.source_info.span)
    }
    fn ty(&self, place: &Place) -> crate::types::TyId {
        place.projection.iter().fold(PlaceTy::from_ty(self.body.local_decls[place.local.0 as usize].ty),
            |ty, elem| ty.projection_ty(self.tcx, elem).expect("invalid MIR place")).ty
    }
    fn visit_statement(&mut self, stmt: &Statement, at: Location, state: BorrowckDomain<'_>) {
        let span = stmt.source_info.span;
        self.check_activations(at, span, state);
        match &stmt.kind {
            StatementKind::Assign(place, value) => {
                self.consume_rvalue(value, at, span, state);
                self.mutate_place(place, Shallow(None), at, span, state);
            }
            StatementKind::FakeRead(_, place) => self.check_if_path_or_subpath_is_moved(place, at, span, state),
            StatementKind::StorageDead(local) => self.access_place(&Place::local(*local), Shallow(None),
                Write(WriteKind::StorageDeadOrDrop), true, at, span, state),
            StatementKind::Intrinsic(NonDivergingIntrinsic::Assume(op)) => self.consume_operand(op, at, span, state),
            StatementKind::AscribeUserType(..) | StatementKind::PlaceMention(..)
            | StatementKind::ConstEvalCounter | StatementKind::StorageLive(..) => {}
            _ => unreachable!("preflight rejected invalid phase"),
        }
    }
    fn visit_terminator(&mut self, term: &Terminator, at: Location, state: BorrowckDomain<'_>) {
        let span = term.source_info.span;
        self.check_activations(at, span, state);
        match &term.kind {
            TerminatorKind::SwitchInt { discr, .. } => self.consume_operand(discr, at, span, state),
            TerminatorKind::Drop { place, replace, .. } => self.access_place(place, AccessDepth::Drop,
                Write(if *replace { WriteKind::Replace } else { WriteKind::StorageDeadOrDrop }), true, at, span, state),
            TerminatorKind::Call { func, args, destination, arg_spans, .. } => {
                self.consume_operand(func, at, span, state);
                for (i, arg) in args.iter().enumerate() { self.consume_operand(arg, at, arg_spans.get(i).copied().unwrap_or(span), state); }
                self.mutate_place(destination, Deep, at, span, state);
            }
            TerminatorKind::TailCall { func, args, arg_spans, .. } => {
                self.consume_operand(func, at, span, state);
                for (i, arg) in args.iter().enumerate() { self.consume_operand(arg, at, arg_spans.get(i).copied().unwrap_or(span), state); }
            }
            TerminatorKind::Assert { cond, msg, .. } => {
                self.consume_operand(cond, at, span, state);
                if let AssertKind::BoundsCheck { len, index } = msg {
                    self.consume_operand(len, at, span, state);
                    self.consume_operand(index, at, span, state);
                }
            }
            TerminatorKind::Yield { value, resume_arg, .. } => {
                self.consume_operand(value, at, span, state);
                self.mutate_place(resume_arg, Deep, at, span, state);
            }
            TerminatorKind::InlineAsm(_) => unreachable!("preflight rejected asm"),
            _ => {}
        }
    }
    fn visit_after_primary_terminator_effect(&mut self, term: &Terminator, _: Location, state: BorrowckDomain<'_>) {
        if matches!(term.kind, TerminatorKind::Return | TerminatorKind::UnwindResume | TerminatorKind::TailCall { .. } | TerminatorKind::CoroutineDrop)
            && matches!(self.borrows.locals_state_at_exit(), LocalsStateAtExit::AllAreInvalidated) {
            for i in state.borrows.iter() { self.check_for_invalidation_at_exit(&self.borrows[i], term.source_info.span); }
        }
        // STOP: movable-coroutine Yield checking requires arch's movability metadata.
    }
    fn consume_rvalue(&mut self, value: &Rvalue, at: Location, span: Span, state: BorrowckDomain<'_>) {
        match value {
            Rvalue::Ref(_, kind, place) => {
                let (depth, access) = match kind {
                    BorrowKind::Fake(FakeBorrowKind::Shallow) => (Shallow(Some(ArtificialField::FakeBorrow)), Read(ReadKind::Borrow(*kind))),
                    BorrowKind::Shared | BorrowKind::Fake(FakeBorrowKind::Deep) => (Deep, Read(ReadKind::Borrow(*kind))),
                    BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow } => (Deep, Reservation(WriteKind::MutableBorrow(*kind))),
                    BorrowKind::Mut { .. } => (Deep, Write(WriteKind::MutableBorrow(*kind))),
                };
                self.access_place(place, depth, access, false, at, span, state);
                self.check_if_path_or_subpath_is_moved(place, at, span, state);
            }
            Rvalue::RawPtr(kind, place) => {
                let (depth, access) = match kind {
                    RawPtrKind::Mut => (Deep, Write(WriteKind::MutableBorrow(BorrowKind::Mut { kind: MutBorrowKind::Default }))),
                    RawPtrKind::Const => (Deep, Read(ReadKind::Borrow(BorrowKind::Shared))),
                    RawPtrKind::FakeForPtrMetadata => (Shallow(Some(ArtificialField::ArrayLength)), Read(ReadKind::Copy)),
                };
                self.access_place(place, depth, access, false, at, span, state);
                self.check_if_path_or_subpath_is_moved(place, at, span, state);
            }
            Rvalue::Use(op) | Rvalue::Repeat(op, _) | Rvalue::UnaryOp(_, op) | Rvalue::Cast(_, op, _) => self.consume_operand(op, at, span, state),
            Rvalue::BinaryOp(_, ops) => {
                self.consume_operand(&ops.0, at, span, state); self.consume_operand(&ops.1, at, span, state);
            }
            Rvalue::Discriminant(place) | Rvalue::Len(place) => {
                let field = if matches!(value, Rvalue::Len(_)) { Some(ArtificialField::ArrayLength) } else { None };
                self.access_place(place, Shallow(field), Read(ReadKind::Copy), false, at, span, state);
                self.check_if_path_or_subpath_is_moved(place, at, span, state);
            }
            Rvalue::Aggregate(_, ops) => for op in ops { self.consume_operand(op, at, span, state); },
            Rvalue::ThreadLocalRef(_) | Rvalue::NullaryOp(..) => {}
            Rvalue::CopyForDeref(_) | Rvalue::ShallowInitBox(..) => unreachable!("preflight rejected runtime rvalue"),
        }
    }
    fn consume_operand(&mut self, op: &Operand, at: Location, span: Span, state: BorrowckDomain<'_>) {
        let (place, access, allow) = match op {
            Operand::Constant(_) => return,
            Operand::Copy(place) => (place, Read(ReadKind::Copy), false),
            Operand::Move(place) => { self.check_movable_place(place, at); (place, Write(WriteKind::Move), true) }
        };
        self.access_place(place, Deep, access, allow, at, span, state);
        self.check_if_path_or_subpath_is_moved(place, at, span, state);
    }
    fn mutate_place(&mut self, place: &Place, depth: AccessDepth, at: Location, span: Span, state: BorrowckDomain<'_>) {
        self.check_if_assigned_path_is_moved(place, at, span, state);
        self.access_place(place, depth, Write(WriteKind::Mutate), false, at, span, state);
    }
    fn access_place(&mut self, place: &Place, depth: AccessDepth, access: ReadOrWrite, allow: bool, at: Location, span: Span, state: BorrowckDomain<'_>) {
        if matches!(access, Activation(..)) && self.reservation_errors.contains(place) { return; }
        if self.access_errors.iter().any(|(p, s)| p == place && *s == span) {
            if access == Write(WriteKind::Mutate) {
                if let Ok((root, allow)) = self.is_mutable(place, allow) { self.add_used_mut(&root, allow, state); }
            }
            return;
        }
        let permission = self.check_access_permissions(place, access, allow, span, state);
        let conflict = self.check_access_for_conflict(place, depth, access, at, span, state);
        if permission || conflict { self.access_errors.push((place.clone(), span)); }
    }
    fn check_access_for_conflict(&mut self, place: &Place, depth: AccessDepth, access: ReadOrWrite, at: Location, span: Span, state: BorrowckDomain<'_>) -> bool {
        let mut conflict = None;
        each_borrow_involving_path(self.tcx, self.body, (depth, place), self.borrows,
            |i| state.borrows.contains(i), |i, borrow| {
                match (access, borrow.kind) {
                    (Activation(_, activating), _) if i == activating => return ControlFlow::Continue(()),
                    (Read(_), BorrowKind::Shared | BorrowKind::Fake(_))
                    | (Read(ReadKind::Borrow(BorrowKind::Fake(FakeBorrowKind::Shallow))), BorrowKind::Mut { .. })
                    | (Reservation(_), BorrowKind::Shared | BorrowKind::Fake(_))
                    | (Write(WriteKind::Move), BorrowKind::Fake(FakeBorrowKind::Shallow)) => return ControlFlow::Continue(()),
                    (Read(_), BorrowKind::Mut { .. }) if !is_active(|a, b| self.cfg.dominates(a, b), borrow, at) => return ControlFlow::Continue(()),
                    _ => {}
                }
                conflict = Some(i); ControlFlow::Break(())
            });
        let Some(i) = conflict else { return false; };
        let borrow = &self.borrows[i];
        // diagnostics/conflict_errors.rs:1701,1766: E0505 and E0503 take their primary span
        // from move_spans at the location (the statement, or the whole terminator), not
        // from the operand; the access-error key above keeps the operand span.
        let mut span = span;
        let kind = match access {
            Read(ReadKind::Copy) => { span = self.span_at(at); UseWhileMutablyBorrowed }
            Read(ReadKind::Borrow(kind)) => self.conflicting_borrow_kind(kind, borrow),
            Write(kind) | Reservation(kind) | Activation(kind, _) => match kind {
                WriteKind::MutableBorrow(kind) => self.conflicting_borrow_kind(kind, borrow),
                WriteKind::Move => { span = self.span_at(at); MoveWhileBorrowed }
                WriteKind::Mutate | WriteKind::Replace => if matches!(borrow.kind, BorrowKind::Fake(_)) && self.immutable_section(&borrow.assigned_place) { MutateInGuard } else { AssignWhileBorrowed },
                WriteKind::StorageDeadOrDrop => { self.lifetime_error(borrow, place, span); return true; }
            },
        };
        if matches!(access, Reservation(_)) { self.reservation_errors.push(place.clone()); }
        self.error(kind, span);
        true
    }
    fn immutable_section(&self, place: &Place) -> bool {
        let cause = self.body.basic_blocks.iter().flat_map(|b| &b.statements).filter_map(|s| match &s.kind {
            StatementKind::FakeRead(cause, p) if p == place => Some(*cause), _ => None,
        }).last();
        matches!(cause, Some(FakeReadCause::ForMatchGuard | FakeReadCause::ForIndex))
    }
    fn conflicting_borrow_kind(&self, new: BorrowKind, old: &BorrowData) -> ErrorKind {
        use BorrowKind::Mut;
        use MutBorrowKind::ClosureCapture;
        match (new, old.kind) {
            (Mut { kind: ClosureCapture }, Mut { kind: ClosureCapture }) => TwoClosures,
            (Mut { .. }, BorrowKind::Fake(FakeBorrowKind::Shallow)) => {
                if self.immutable_section(&old.assigned_place) { MutateInGuard } else { ConflictingBorrow }
            },
            (Mut { kind: ClosureCapture }, _) => ClosureRequiresUnique,
            (_, Mut { kind: ClosureCapture }) => AlreadyUniquelyBorrowed,
            (Mut { .. }, Mut { .. }) => MultipleMutableBorrows,
            _ => ConflictingBorrow,
        }
    }
    fn has_destructor_prefix(&self, place: &Place) -> bool {
        for (i, elem) in place.projection.iter().enumerate() {
            if matches!(elem, PlaceElem::Field(..) | PlaceElem::OpaqueCast(_) | PlaceElem::Downcast(..)) {
                let base = Place { local: place.local, projection: place.projection[..i].to_vec() };
                if matches!(self.tcx.tys.kind(self.ty(&base)), TyKind::Adt(def, ..) if self.tcx.adt_destructor(*def).is_some()) { return true; }
            }
        }
        false
    }
    fn check_activations(&mut self, at: Location, span: Span, state: BorrowckDomain<'_>) {
        for &index in self.borrows.activations_at_location(&at) {
            let borrow = &self.borrows[index];
            assert!(matches!(borrow.kind, BorrowKind::Mut { .. }));
            self.access_place(&borrow.borrowed_place, Deep, Activation(WriteKind::MutableBorrow(borrow.kind), index), false, at, span, state);
        }
    }
    fn lifetime_error(&mut self, borrow: &BorrowData, dropped: &Place, _drop_span: Span) {
        let borrow_span = self.span_at(borrow.reserve_location);
        let key = (Place::local(borrow.borrowed_place.local),borrow_span);
        if self.access_errors.contains(&key) { return; }
        self.access_errors.push(key);
        let decl = &self.body.local_decls[borrow.borrowed_place.local.0 as usize];
        let kind = if matches!(decl.local_info, LocalInfo::StaticRef { is_thread_local: true, .. }) { ThreadLocalEscapes }
            else if !dropped.projection.starts_with(&borrow.borrowed_place.projection) && self.has_destructor_prefix(&borrow.borrowed_place) { BorrowAcrossDestructor }
            else if decl.kind == LocalKind::Temp { TemporaryDropped } else { DoesNotLiveLongEnough };
        let span = if kind == TemporaryDropped { decl.source_info.span } else { self.span_at(borrow.reserve_location) };
        self.error(kind, span);
    }
    fn check_for_invalidation_at_exit(&mut self, borrow: &BorrowData, span: Span) {
        let mut root = Place::local(borrow.borrowed_place.local);
        let thread_local = matches!(self.body.local_decls[root.local.0 as usize].local_info, LocalInfo::StaticRef { is_thread_local: true, .. });
        if thread_local { root.projection.push(PlaceElem::Deref); }
        if borrow_conflicts_with_place(self.tcx, self.body, &borrow.borrowed_place, borrow.kind, &root,
            if thread_local { Deep } else { Shallow(None) }, PlaceConflictBias::Overlap) { self.lifetime_error(borrow, &root, span); }
    }
    fn first_reaching_init(&self, local: Local, state: BorrowckDomain<'_>) -> Option<usize> {
        let path = self.moves.rev_lookup.find_local(local)?;
        self.moves.init_path_map[path].iter().copied().find(|&i| state.ever_inits.contains(i))
    }
    fn add_used_mut(&mut self, root: &Place, allow: bool, state: BorrowckDomain<'_>) {
        if root.projection.is_empty() && !allow && self.first_reaching_init(root.local, state).is_some() {
            self.result.used_mut.insert(root.local.0 as usize);
        }
        // Upvar roots are stopped before checking, until MIR owns capture metadata.
    }
    fn check_access_permissions(&mut self, place: &Place, access: ReadOrWrite, allow: bool, span: Span, state: BorrowckDomain<'_>) -> bool {
        let (borrow, allow) = match access {
            Write(WriteKind::MutableBorrow(BorrowKind::Mut { kind })) | Reservation(WriteKind::MutableBorrow(BorrowKind::Mut { kind })) =>
                (true, allow || kind == MutBorrowKind::ClosureCapture),
            Write(WriteKind::Mutate) | Reservation(WriteKind::Mutate) => (false, allow),
            _ => return false, // Activation permissions were checked at reservation.
        };
        let the_place_err = match self.is_mutable(place, allow) {
            Ok((root, allow)) => { self.add_used_mut(&root, allow, state); return false; }
            Err(the_place_err) => the_place_err,
        };
        if self.first_reaching_init(place.local, state).is_none() { return false; }
        if !borrow {
            self.error(if place.projection.is_empty() { Reassigned } else { AssignImmutable }, span);
        } else {
            self.report_mutable_borrow_error(&the_place_err, span);
        }
        true
    }
    // diagnostics/mutability_errors.rs:226-250,659: a second mutable borrow of the same
    // immutable binding joins the first error, whose primary span becomes the binding.
    fn report_mutable_borrow_error(&mut self, the_place_err: &Place, span: Span) {
        let decl = &self.body.local_decls[the_place_err.local.0 as usize];
        let can_be_made_mutable = matches!(decl.local_info, LocalInfo::User { can_be_made_mutable: true, .. });
        if !the_place_err.projection.is_empty() || !can_be_made_mutable {
            self.result.errors.push(BorrowckError { kind: BorrowImmutable, span });
            return;
        }
        let binding_span = decl.source_info.span;
        if let Some(entry) = self.buffered_mut_errors.iter_mut().find(|(s, ..)| *s == binding_span) {
            entry.2 += 1;
            if entry.2 == 2 { self.result.errors[entry.1].span = binding_span; }
            return;
        }
        self.buffered_mut_errors.push((binding_span, self.result.errors.len(), 1));
        self.result.errors.push(BorrowckError { kind: BorrowImmutable, span });
    }
    /// lib.rs:2567: Ok(root place, its mutation mode) or Err(the place that is not mutable).
    fn is_mutable(&self, place: &Place, allow: bool) -> Result<(Place, bool), Place> {
        let Some((elem, base)) = last_projection(place) else {
            return if allow || self.body.local_decls[place.local.0 as usize].mutability == Mutability::Mut {
                Ok((place.clone(), allow))
            } else { Err(place.clone()) };
        };
        if elem == PlaceElem::Deref {
            match self.tcx.tys.kind(self.ty(&base)) {
                TyKind::Ref(false, ..) | TyKind::Ptr(false, _) => Err(place.clone()),
                TyKind::Ref(true, ..) => self.is_mutable(&base, true),
                TyKind::Ptr(true, _) => Ok((place.clone(), allow)),
                TyKind::Adt(def, ..) if self.tcx.adts[def].is_box() => self.is_mutable(&base, allow),
                _ => panic!("invalid MIR dereference"),
            }
        } else { self.is_mutable(&base, allow) }
    }
    fn check_movable_place(&mut self, place: &Place, at: Location) {
        if let Some(kind) = illegal_move(self.tcx, self.body, place) { self.move_errors.push(MoveError { place: place.clone(), location: at, kind }); }
    }
    fn exact_path(&self, place: &Place) -> Option<usize> {
        match self.moves.rev_lookup.find(place) { LookupResult::Exact(i) => Some(i), LookupResult::Parent(_) => None }
    }
    fn check_if_full_path_is_moved(&mut self, place: &Place, at: Location, span: Span, state: BorrowckDomain<'_>) {
        let path = match self.moves.rev_lookup.find(place) {
            LookupResult::Exact(i) | LookupResult::Parent(Some(i)) => i,
            LookupResult::Parent(None) => panic!("missing move path for MIR local"),
        };
        if state.uninits.contains(path) { self.report_use_of_moved_or_uninitialized(place, path, at, span); }
    }
    fn uninit_descendant(&self, path: usize, state: BorrowckDomain<'_>) -> Option<usize> {
        find_in_move_path_or_its_descendants(self.moves, path, |i| state.uninits.contains(i))
    }
    fn check_if_path_or_subpath_is_moved(&mut self, place: &Place, at: Location, span: Span, state: BorrowckDomain<'_>) {
        self.check_if_full_path_is_moved(place, at, span, state);
        if let Some((PlaceElem::Subslice { from, to, from_end: false }, base)) = last_projection(place) {
            if matches!(self.tcx.tys.kind(self.ty(&base)), TyKind::Array(..)) {
                if let Some(path) = self.exact_path(&base) {
                    let mut child = self.moves.move_paths[path].first_child;
                    while let Some(i) = child {
                        if let Some(PlaceElem::ConstantIndex { offset, from_end: false, .. }) = self.moves.move_paths[i].place.projection.last() {
                            if (from..to).contains(offset) {
                                if let Some(i) = self.uninit_descendant(i, state) { self.report_use_of_moved_or_uninitialized(&base, i, at, span); return; }
                            }
                        }
                        child = self.moves.move_paths[i].next_sibling;
                    }
                }
                return;
            }
        }
        if let Some(path) = self.exact_path(place) {
            if let Some(i) = self.uninit_descendant(path, state) { self.report_use_of_moved_or_uninitialized(place, i, at, span); }
        }
    }
    fn check_if_assigned_path_is_moved(&mut self, place: &Place, at: Location, span: Span, state: BorrowckDomain<'_>) {
        for i in (0..place.projection.len()).rev() {
            let base = Place { local: place.local, projection: place.projection[..i].to_vec() };
            match place.projection[i] {
                PlaceElem::Deref => { self.check_if_full_path_is_moved(&base, at, span, state); break; }
                PlaceElem::Subslice { .. } => panic!("assignment to MIR subslice"),
                PlaceElem::Field(..) => {
                    let ty = self.tcx.tys.kind(self.ty(&base));
                    if matches!(ty, TyKind::Adt(def, ..) if self.tcx.adt_destructor(*def).is_some()) {
                        self.check_if_path_or_subpath_is_moved(&base, at, span, state); break;
                    }
                    if matches!(ty, TyKind::Adt(..) | TyKind::Tuple(_)) {
                        let mut shortest = None;
                        for prefix in prefixes(&base, PrefixSet::Shallow) {
                            if let Some(path) = self.exact_path(&prefix) { if state.uninits.contains(path) { shortest = Some(path); } }
                        }
                        if let Some(path) = shortest {
                            if matches!(ty, TyKind::Adt(def, ..) if self.tcx.adts[def].is_union())
                                && self.moves.path_map[path].iter().any(|&i| self.cfg.is_predecessor(self.body, self.moves.moves[i].source, at)) { continue; }
                            self.report_use_of_moved_or_uninitialized(&base, path, at, span);
                            self.result.used_mut.insert(base.local.0 as usize);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // ported from N:rustc_borrowck/src/diagnostics/conflict_errors.rs @8d1a764
    // report_use_of_moved_or_uninitialized:74-350, reduced to code + primary span. E0381
    // is reported once per root local; E0382 once per set of move-outs, where a use of a
    // prefix of the place already reported for that set is suppressed and any other use
    // replaces the buffered error (buffer_move_error's IndexMap insert).
    fn report_use_of_moved_or_uninitialized(&mut self, used_place: &Place, mpi: usize, at: Location, span: Span) {
        let move_out_indices = self.get_moved_indexes(at, mpi);
        if move_out_indices.is_empty() {
            if self.uninitialized_errors.insert(used_place.local.0 as usize) {
                self.error(Uninitialized, span);
            }
            return;
        }
        let error = BorrowckError { kind: Moved, span };
        if let Some(entry) = self.buffered_move_errors.iter_mut().find(|(key, ..)| *key == move_out_indices) {
            if is_prefix_of(used_place, &entry.1) { return; }
            entry.1 = used_place.clone();
            entry.2 = error;
            return;
        }
        self.buffered_move_errors.push((move_out_indices, used_place.clone(), error));
    }
    // ported from N:rustc_borrowck/src/diagnostics/conflict_errors.rs @8d1a764
    // get_moved_indexes:3836-4021: the move-outs that reach `at`, found by a backward DFS
    // that stops at moves and (re)initializations; backedges (moves in a later loop
    // iteration) are only searched when the path is definitely initialized on entry.
    // The reinitialization list feeds labels only.
    fn get_moved_indexes(&self, at: Location, mpi: usize) -> Vec<usize> {
        let mut mpis = vec![mpi];
        while let Some(parent) = self.moves.move_paths[*mpis.last().unwrap()].parent { mpis.push(parent); }
        let mut stack = vec![];
        let mut back_edge_stack = vec![];
        for predecessor in self.cfg.predecessor_locations(self.body, at) {
            if self.cfg.location_dominates(at, predecessor) { back_edge_stack.push(predecessor); } else { stack.push(predecessor); }
        }
        let mut reached_start = false;
        // Is the mpi initialized as an argument?
        let is_argument = (1..=self.body.arg_count).any(|arg| {
            self.moves.rev_lookup.find_local(Local(arg)).is_some_and(|path| mpis.contains(&path))
        });
        let mut visited = vec![];
        let mut result = vec![];
        let mut dfs_iter = |result: &mut Vec<usize>, location: Location| -> bool {
            if visited.contains(&location) { return true; }
            visited.push(location);
            let data = &self.body.basic_blocks[location.block.0 as usize];
            // Only moves written by the user count: not the move-outs of StorageDead.
            if !matches!(data.statements.get(location.statement_index).map(|s| &s.kind), Some(StatementKind::StorageDead(_))) {
                for &moi in &self.moves.loc_map[location] {
                    if mpis.contains(&self.moves.moves[moi].path) { result.push(moi); return true; }
                }
            }
            self.moves.init_loc_map[location].iter().any(|&ii| {
                let init = self.moves.inits[ii];
                match init.kind {
                    InitKind::Deep | InitKind::NonPanicPathOnly => mpis.contains(&init.path),
                    InitKind::Shallow => mpi == init.path,
                }
            })
        };
        while let Some(location) = stack.pop() {
            if dfs_iter(&mut result, location) { continue; }
            let predecessors = self.cfg.predecessor_locations(self.body, location);
            if predecessors.is_empty() { reached_start = true; }
            for predecessor in predecessors {
                if self.cfg.location_dominates(location, predecessor) { back_edge_stack.push(predecessor); } else { stack.push(predecessor); }
            }
        }
        if (is_argument || !reached_start) && result.is_empty() {
            while let Some(location) = back_edge_stack.pop() {
                if dfs_iter(&mut result, location) { continue; }
                back_edge_stack.extend(self.cfg.predecessor_locations(self.body, location));
            }
        }
        result
    }
}
// rustc_mir_dataflow move_paths/mod.rs:106,419: the path itself, then a DFS whose stack
// receives each visited path's first child and then its next sibling.
pub(super) fn find_in_move_path_or_its_descendants(moves: &MoveData, root: usize, pred: impl Fn(usize) -> bool) -> Option<usize> {
    if pred(root) { return Some(root); }
    let mut todo = vec![moves.move_paths[root].first_child?];
    while let Some(mpi) = todo.pop() {
        if pred(mpi) { return Some(mpi); }
        let path = &moves.move_paths[mpi];
        if let Some(child) = path.first_child { todo.push(child); }
        if let Some(sibling) = path.next_sibling { todo.push(sibling); }
    }
    None
}
fn is_prefix_of(prefix: &Place, place: &Place) -> bool {
    prefix.local == place.local && place.projection.starts_with(&prefix.projection)
}

/// diagnostics/move_errors.rs:42 MoveError, with the error code of its IllegalMoveOriginKind.
struct MoveError { place: Place, location: Location, kind: ErrorKind }
enum GroupedMoveError {
    MovesFromPlace { span: Span, kind: ErrorKind },
    MovesFromValue { span: Span, move_from: usize, kind: ErrorKind },
    OtherIllegalMove { span: Span, kind: ErrorKind },
}
// ported from N:rustc_borrowck/src/diagnostics/move_errors.rs @8d1a764
// report_move_errors/group_move_errors/append_to_grouped_errors/append_binding_error:103-232.
// The moves of one pattern-match are grouped into one error at the matched expression
// (or the `let` statement); the binding lists only label the error and are not kept.
// `moves` is None for promoted bodies, whose locals are never user bindings.
fn report_move_errors(body: &Body, moves: Option<&MoveData>, errors: Vec<MoveError>, out: &mut Vec<BorrowckError>) {
    let mut grouped: Vec<GroupedMoveError> = vec![];
    for MoveError { place: _original_path, location, kind } in errors {
        let block = &body.basic_blocks[location.block.0 as usize];
        let stmt = block.statements.get(location.statement_index);
        if let Some(Statement { kind: StatementKind::Assign(place, Rvalue::Use(Operand::Move(move_from))), source_info }) = stmt {
            if place.projection.is_empty() {
                if let LocalInfo::User { match_place: Some((opt_match_place, match_span)), .. } = &body.local_decls[place.local.0 as usize].local_info {
                    let moves = moves.expect("promoted MIR has no user bindings");
                    append_binding_error(&mut grouped, moves, kind, move_from, opt_match_place.as_ref(), *match_span, source_info.span);
                    continue;
                }
            }
        }
        // move_spans: the statement or terminator span (closure captures are stopped).
        let span = stmt.map_or_else(|| block.terminator.as_ref().unwrap().source_info.span, |s| s.source_info.span);
        grouped.push(GroupedMoveError::OtherIllegalMove { span, kind });
    }
    for error in grouped {
        let (GroupedMoveError::MovesFromPlace { span, kind } | GroupedMoveError::MovesFromValue { span, kind, .. }
            | GroupedMoveError::OtherIllegalMove { span, kind }) = error;
        let error = BorrowckError { kind, span };
        if !out.contains(&error) { out.push(error); }
    }
}
fn append_binding_error(grouped: &mut Vec<GroupedMoveError>, moves: &MoveData, kind: ErrorKind, move_from: &Place,
    match_place: Option<&Place>, match_span: Span, statement_span: Span) {
    let from_simple_let = match_place.is_none();
    let match_place = match_place.unwrap_or(move_from);
    match moves.rev_lookup.find(match_place) {
        // Error with the match place.
        LookupResult::Parent(_) => {
            if grouped.iter().any(|ge| matches!(ge, GroupedMoveError::MovesFromPlace { span, .. } if *span == match_span)) { return; }
            // Don't need to point to x in let x = ... .
            let span = if from_simple_let { statement_span } else { match_span };
            grouped.push(GroupedMoveError::MovesFromPlace { span, kind });
        }
        // Error with the pattern.
        LookupResult::Exact(_) => {
            let LookupResult::Parent(Some(mpi)) = moves.rev_lookup.find(move_from) else {
                unreachable!("move_from should be a projection from match_place");
            };
            if grouped.iter().any(|ge| matches!(ge, GroupedMoveError::MovesFromValue { span, move_from, .. } if *span == match_span && *move_from == mpi)) {
                return;
            }
            grouped.push(GroupedMoveError::MovesFromValue { span: match_span, move_from: mpi, kind });
        }
    }
}
fn illegal_move(tcx: &Tcx, body: &Body, place: &Place) -> Option<ErrorKind> {
    for (i, elem) in place.projection.iter().enumerate() {
        let base = Place { local: place.local, projection: place.projection[..i].to_vec() };
        let ty = tcx.tys.kind(base.projection.iter().fold(PlaceTy::from_ty(body.local_decls[base.local.0 as usize].ty), |ty, e| ty.projection_ty(tcx, e).expect("invalid MIR place")).ty);
        let kind = match (elem, ty) {
            (PlaceElem::Deref, TyKind::Ref(..) | TyKind::Ptr(..)) => Some(MoveBorrowedContent),
            (PlaceElem::Field(..), TyKind::Adt(def, ..)) if tcx.adt_destructor(*def).is_some() => Some(MoveDropType),
            (PlaceElem::Index(_), TyKind::Array(..) | TyKind::Slice(_))
            | (PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. }, TyKind::Slice(_)) => Some(MoveIndexed),
            _ => None,
        };
        if kind.is_some() { return kind; }
    }
    None
}

fn last_projection(place: &Place) -> Option<(PlaceElem, Place)> {
    let (elem, projection) = place.projection.split_last()?;
    Some((elem.clone(), Place { local: place.local, projection: projection.to_vec() }))
}

/// Rebase Relax nll.rs::compute_dominators onto MIR; reuse its RPO/intersect
/// algorithm and MIR's Visitor for edges. No second dataflow engine is introduced.
struct CheckCfg { predecessors: Vec<Vec<BasicBlock>>, idom: Vec<usize>, reverse_postorder: Vec<usize> }
impl CheckCfg {
    fn new(body: &Body) -> Self {
        struct Edges(Vec<BasicBlock>);
        impl Visitor for Edges { fn visit_target(&mut self, b: &BasicBlock, _: bool, _: Location) { self.0.push(*b); } }
        let n = body.basic_blocks.len();
        let mut successors = vec![vec![]; n];
        let mut predecessors = vec![vec![]; n];
        for (b, block) in body.basic_blocks.iter().enumerate() {
            let mut edges = Edges(vec![]);
            edges.visit_terminator(block.terminator.as_ref().unwrap(), Location { block: BasicBlock(b as u32), statement_index: block.statements.len() });
            for &s in &edges.0 { predecessors[s.0 as usize].push(BasicBlock(b as u32)); }
            successors[b] = edges.0;
        }
        let mut seen = BitSet::new_empty(n);
        let mut stack = vec![(0, false)];
        let mut order = vec![];
        while let Some((b, finish)) = stack.pop() {
            if finish { order.push(b); continue; }
            if !seen.insert(b) { continue; }
            stack.push((b, true));
            // nightly Postorder consumes successors with next_back().
            for s in &successors[b] { stack.push((s.0 as usize, false)); }
        }
        order.reverse();
        let mut rank = vec![usize::MAX; n];
        for (i, &b) in order.iter().enumerate() { rank[b] = i; }
        let mut idom = vec![usize::MAX; n]; idom[0] = 0;
        loop {
            let mut changed = false;
            for &b in order.iter().skip(1) {
                let mut new = usize::MAX;
                for p in predecessors[b].iter().map(|p| p.0 as usize).filter(|&p| idom[p] != usize::MAX) {
                    if new == usize::MAX { new = p; continue; }
                    let mut p = p;
                    while p != new { if rank[p] > rank[new] { p = idom[p]; } else { new = idom[new]; } }
                }
                if new != usize::MAX && new != idom[b] { idom[b] = new; changed = true; }
            }
            if !changed { break; }
        }
        Self { predecessors, idom, reverse_postorder: order }
    }
    fn dominates(&self, a: BasicBlock, b: BasicBlock) -> bool {
        let mut b = b.0 as usize;
        loop {
            if b == a.0 as usize { return true; }
            let parent = self.idom[b];
            if parent == usize::MAX || parent == b { return false; } b = parent;
        }
    }
    fn location_dominates(&self, a: Location, b: Location) -> bool {
        if a.block == b.block { a.statement_index <= b.statement_index } else { self.dominates(a.block, b.block) }
    }
    fn predecessor_locations(&self, body: &Body, at: Location) -> Vec<Location> {
        if at.statement_index > 0 { return vec![Location { statement_index: at.statement_index - 1, ..at }]; }
        self.predecessors[at.block.0 as usize].iter().map(|&block| Location {
            block, statement_index: body.basic_blocks[block.0 as usize].statements.len(),
        }).collect()
    }
    fn is_predecessor(&self, body: &Body, from: Location, to: Location) -> bool {
        let mut stack = self.predecessor_locations(body, to);
        let mut seen = vec![];
        while let Some(at) = stack.pop() {
            if at == from { return true; }
            if seen.contains(&at) { continue; } seen.push(at);
            stack.extend(self.predecessor_locations(body, at));
        }
        false
    }
}

// lib.rs:464-492 promoted MoveVisitor: only illegal-move checks, with promoted locals,
// reported per promoted body through report_move_errors.
fn check_promoted_moves(tcx: &Tcx, body: &Body, result: &mut MirBorrowckResult) {
    struct Moves<'a> { tcx: &'a Tcx, body: &'a Body, errors: Vec<MoveError> }
    impl Visitor for Moves<'_> {
        fn visit_operand(&mut self, op: &Operand, at: Location) {
            if let Operand::Move(place) = op {
                if let Some(kind) = illegal_move(self.tcx, self.body, place) {
                    self.errors.push(MoveError { place: place.clone(), location: at, kind });
                }
            }
        }
    }
    for promoted in &body.promoted {
        check_supported(tcx, promoted, &mut result.unsupported);
        if !result.unsupported.is_empty() { continue; }
        let mut visitor = Moves { tcx, body: promoted, errors: vec![] };
        for (b, block) in promoted.basic_blocks.iter().enumerate() { visitor.visit_basic_block_data(BasicBlock(b as u32), block); }
        report_move_errors(promoted, None, visitor.errors, &mut result.errors);
        check_promoted_moves(tcx, promoted, result);
    }
}
