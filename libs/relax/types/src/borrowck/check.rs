// ported from N:rustc_borrowck/src/lib.rs @8d1a764 (MirBorrowckCtxt)
use super::borrow_set::{BorrowData, BorrowSet, LocalsStateAtExit};
use super::field_views::FieldViews;
use super::borrowck_errors::{AccessKind, BorrowInfo, BorrowckError, Detail, ErrorKind, FvNote, InitAction, MoveSite};
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
    /// Fields of the closure environment this (closure) body mutated through, to be marked `mut` by its creator.
    pub used_mut_upvars: Vec<FieldIdx>,
    pub region_errors: Vec<super::region_infer::RegionErrorKind>,
    pub tainted_by_errors: bool,
    /// Relaxed crates: conflicts accepted only with field views / extended two-phase borrows
    /// (rustc would reject them), and the records the export to rustc rewrites.
    pub fv_only: Vec<BorrowckError>,
    pub fv_records: Vec<super::field_views::FvRecordRaw>,
    /// Relaxed crates: conflicts accepted by an analysis stronger than NLL (Polonius, disjoint
    /// indices), with the facts their export to rustc needs.
    pub relaxed_records: Vec<super::field_views::RelaxedRecord>,
}
impl MirBorrowckResult {
    pub fn accepted(&self) -> bool { self.errors.is_empty() && self.unsupported.is_empty() && self.region_errors.is_empty() && !self.tainted_by_errors }
}

/// Check one typed, renumbered analysis MIR body against its solved regions.
/// MoveData is the same body-owned, Copy-filtered arena used by MIR type checking.
/// C-NLL5 supplies it and the region input; this entry never invokes BIR borrowck.
pub fn mir_borrowck(tcx: &Tcx, prog: &Program, body: &Body, regions: &mut MirRegionResult, moves: &MoveData) -> MirBorrowckResult {
    mir_borrowck_fv(tcx, prog, body, regions, moves, None)
}

/// `mir_borrowck` with relaxed-Rust field views (field_views/check_hook.rs).
pub fn mir_borrowck_fv(tcx: &Tcx, prog: &Program, body: &Body, regions: &mut MirRegionResult, moves: &MoveData, fv: Option<&FieldViews<'_>>) -> MirBorrowckResult {
    mir_borrowck_nested(tcx, prog, body, regions, moves, fv, &NestedUsedMut::default())
}

/// What the nested bodies of a body reported: the closure fields each mutated through (`used_mut_upvars`),
/// which the creator turns into uses of the captured variables.
#[derive(Default)]
pub struct NestedUsedMut(pub Vec<(ClosureId, Vec<FieldIdx>)>);
impl NestedUsedMut {
    fn get(&self, id: ClosureId) -> Option<&[FieldIdx]> { self.0.iter().find(|(c, _)| *c == id).map(|(_, f)| &f[..]) }
}

/// `mir_borrowck_fv` for a body that creates closures: `nested` has their results.
///
/// The verdict is NLL's, as stable rustc's (1.97.1 runs no Polonius). Polonius (rustc's location-sensitive
/// "polonius alpha", `-Zpolonius=next`, ported in polonius/) decides only (a) for a body NLL rejects in a
/// crate with relaxed Rust's `polonius` feature: the NLL errors Polonius does not have are accepted and
/// reported as such (`fv_only`, with a `relaxed_records` entry for the export to rustc), and (b) in the
/// opt-in verification sample, where Polonius must accept what NLL accepts.
pub fn mir_borrowck_nested(tcx: &Tcx, prog: &Program, body: &Body, regions: &mut MirRegionResult, moves: &MoveData, fv: Option<&FieldViews<'_>>, nested: &NestedUsedMut) -> MirBorrowckResult {
    let nll = check_with_scopes_fv(prog, tcx, body, regions, moves, fv, nested);
    if !nll.unsupported.is_empty() { return nll; }
    let verify = nll.accepted() && verify_polonius_sample();
    let relaxed = !nll.errors.is_empty() && relaxed_feature(prog, body, "polonius");
    if !verify && !relaxed { return nll; }
    let polonius_regions = compute_polonius_regions(tcx, prog, body, regions, moves);
    let mut polonius = check_with_scopes_fv(prog, tcx, body, &polonius_regions, moves, fv, nested);
    if verify {
        assert!(polonius.accepted(), "Polonius must accept NLL-accepted MIR: {polonius:?}");
        return nll;
    }
    let accepted: Vec<BorrowckError> = nll.errors.iter().filter(|e| !polonius.errors.contains(e)).cloned().collect();
    if accepted.is_empty() { return nll; }
    let feeds = super::field_views::find_feeds(body);
    for mut error in accepted {
        polonius.relaxed_records.push(super::field_views::RelaxedRecord::Polonius(polonius_record(tcx, prog, body, &feeds, &error)));
        error.fv = Some(Box::new(FvNote::Polonius));
        if !polonius.fv_only.contains(&error) { polonius.fv_only.push(error); }
    }
    *regions = polonius_regions;
    polonius
}

/// Whether relaxed Rust `feature` is on for the crate of `body`.
pub(super) fn relaxed_feature(prog: &Program, body: &Body, feature: &str) -> bool {
    // MIR whose owner is not a def of `prog` (unit fixtures) belongs to no crate: strict
    let Some(def) = prog.defs.get(body.source.owner.0 as usize) else { return false };
    prog.crates.get(def.krate as usize).is_some_and(|c| c.relaxed(prog).iter().any(|f| f == feature))
}

/// The export facts of an NLL error Polonius accepts: the loan NLL kept alive and, when it is the
/// receiver of a std map lookup whose key compares by std's own `Hash`/`Eq`/`Ord`, that lookup (a second
/// lookup with the same key then finds the same entry, which makes the re-lookup rewrite exact).
fn polonius_record(tcx: &Tcx, prog: &Program, body: &Body, feeds: &crate::fxhash::HashMap<Local, super::field_views::Feed>, error: &BorrowckError) -> super::field_views::PoloniusRecord {
    use super::field_views::{Feed, Lookup, PoloniusRecord};
    let span_at = |at: Location| {
        let block = &body.basic_blocks[at.block.0 as usize];
        block.statements.get(at.statement_index).map_or_else(|| block.terminator.as_ref().unwrap().source_info.span, |s| s.source_info.span)
    };
    let Detail::Conflict { borrow, .. } = &error.detail else {
        return PoloniusRecord { code: error.code(), span: error.span, loan: error.span, lookup: None };
    };
    let loan = span_at(borrow.reserve_location);
    let lookup = (|| {
        if !borrow.assigned_place.projection.is_empty() { return None; }
        let Some(Feed::Call(bi, 0)) = feeds.get(&borrow.assigned_place.local).copied() else { return None };
        let term = body.basic_blocks[bi].terminator.as_ref()?;
        let TerminatorKind::Call { func: Operand::Constant(c), args, .. } = &term.kind else { return None };
        let Const::Val(_, fty) = &c.const_ else { return None };
        let TyKind::FnDef(d, _, _) = tcx.tys.kind(*fty) else { return None };
        let mutable = match prog.name(*d) { "get_mut" => true, "get" => false, _ => return None };
        // an inherent method of std's HashMap / BTreeMap (the map is what the loan borrows; Relax's
        // std is one crate, compiler/std/core)
        let map_ty = place_ty(tcx, body, &borrow.borrowed_place);
        let TyKind::Adt(map, ..) = tcx.tys.kind(map_ty) else { return None };
        // reached from a variable through built-in derefs only (no overloaded deref to run twice)
        if !matches!(body.local_decls[borrow.borrowed_place.local.0 as usize].kind, LocalKind::UserVar | LocalKind::Arg) { return None; }
        for (i, elem) in borrow.borrowed_place.projection.iter().enumerate() {
            match elem {
                PlaceElem::Field(..) => {}
                PlaceElem::Deref => {
                    let base = Place { local: borrow.borrowed_place.local, projection: borrow.borrowed_place.projection[..i].to_vec() };
                    if !matches!(tcx.tys.kind(place_ty(tcx, body, &base)), TyKind::Ref(..)) { return None; }
                }
                _ => return None,
            }
        }
        if !matches!(prog.def_path(*map).as_str(), "core::collections::hash_map::HashMap" | "core::collections::btree_map::BTreeMap") { return None; }
        if prog.syms.str(prog.crates[prog.def(*d).krate as usize].name) != "core" { return None; }
        // an inherent impl (no trait): std's own method
        let imp = prog.def(prog.def(*d).parent);
        if prog.def(*d).kind != crate::program::DefKind::AssocFn || imp.kind != crate::program::DefKind::Impl { return None; }
        let ast = &prog.files[imp.file as usize].ast;
        if ast.as_impl(&ast.item(imp.item).kind)?.trait_.is_some() { return None; }
        // the key `&Q`: Q's Hash / Eq / Ord must be std's own
        let key = args.get(1)?;
        let key_ty = match key { Operand::Copy(p) | Operand::Move(p) => place_ty(tcx, body, p), Operand::Constant(c) => match &c.const_ { Const::Val(_, t) => *t, _ => return None }, _ => return None };
        let TyKind::Ref(_, q, _) = tcx.tys.kind(key_ty) else { return None };
        if !std_compared(tcx, *q, 0) { return None; }
        Some(Lookup { mutable, call: term.source_info.span })
    })();
    PoloniusRecord { code: error.code(), span: error.span, loan, lookup }
}

fn place_ty(tcx: &Tcx, body: &Body, place: &Place) -> crate::types::TyId {
    place.projection.iter().fold(PlaceTy::from_ty(body.local_decls[place.local.0 as usize].ty),
        |ty, elem| ty.projection_ty(tcx, elem).expect("invalid MIR place")).ty
}

/// Types whose `Hash`, `Eq` and `Ord` are std's own impls (pure functions of the value): integers,
/// `bool`, `char`, `str`, `String`, and references / tuples / arrays / slices of them.
fn std_compared(tcx: &Tcx, ty: crate::types::TyId, depth: u32) -> bool {
    if depth > 8 { return false; }
    match tcx.tys.kind(ty) {
        TyKind::Int(_) | TyKind::Bool | TyKind::Char | TyKind::Str => true,
        TyKind::Ref(_, t, _) | TyKind::Slice(t) | TyKind::Array(t, _) => std_compared(tcx, *t, depth + 1),
        TyKind::Tuple(ts) => tcx.tys.list(*ts).iter().all(|t| std_compared(tcx, *t, depth + 1)),
        TyKind::Adt(d, ..) => tcx.lang_items.get(crate::lang_items::LangItem::String) == Some(*d),
        _ => false,
    }
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
    MirRegionResult { regioncx, locations: regions.locations.clone(), type_tests: regions.type_tests.clone(), universe_causes: regions.universe_causes.clone(),
        polonius: regions.polonius.clone(), nested: regions.nested, tainted_by_errors: regions.tainted_by_errors }
}

#[cfg(test)]
pub(super) fn check_with_scopes(prog: &Program, tcx: &Tcx, body: &Body, regions: &MirRegionResult, moves: &MoveData) -> MirBorrowckResult {
    check_with_scopes_fv(prog, tcx, body, regions, moves, None, &NestedUsedMut::default())
}
fn check_with_scopes_fv(prog: &Program, tcx: &Tcx, body: &Body, regions: &MirRegionResult, moves: &MoveData, fv: Option<&FieldViews<'_>>, nested: &NestedUsedMut) -> MirBorrowckResult {
    assert!(regions.locations.matches(body), "region locations belong to another MIR layout");
    let mut result = MirBorrowckResult { errors: vec![], used_mut: BitSet::new_empty(body.local_decls.len()),
        unsupported: vec![], used_mut_upvars: vec![], region_errors: regions.regioncx.errors.clone(), tainted_by_errors: regions.tainted_by_errors,
        fv_only: vec![], fv_records: vec![], relaxed_records: vec![] };
    check_supported(tcx, body, Some(nested), &mut result.unsupported);
    if !result.unsupported.is_empty() { return result; }
    let invalidated = matches!(body.source.kind, BodyKind::Fn | BodyKind::Closure(_) | BodyKind::Shim(_));
    let borrows = BorrowSet::build(tcx, body, invalidated, moves);
    let analysis = Borrowck::new(prog, tcx, body, regions, &borrows, moves);
    let results = analysis.iterate_to_fixpoint();
    let mut cursor = results.cursor();
    let mut cx = MirBorrowckCtxt { prog, tcx, body, moves, borrows: &borrows, cfg: CheckCfg::new(body), result,
        access_errors: vec![], reservation_errors: vec![], uninitialized_errors: BitSet::new_empty(body.local_decls.len()),
        move_errors: vec![], buffered_move_errors: vec![], buffered_mut_errors: vec![],
        fv: fv.map(|v| fv_hook::FvRun::new(v, body, &borrows)), fv_own: None,
        d3: relaxed_feature(prog, body, "disjoint-index"), d3_feeds: None, d3_taken: None, nested };
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


fn check_supported(tcx: &Tcx, body: &Body, nested: Option<&NestedUsedMut>, unsupported: &mut Vec<&'static str>) {
    struct Check<'a> { tcx: &'a Tcx, body: &'a Body, nested: Option<&'a NestedUsedMut>, stops: &'a mut Vec<&'static str> }
    impl Check<'_> { fn stop(&mut self, s: &'static str) { if !self.stops.contains(&s) { self.stops.push(s); } } }
    impl Visitor for Check<'_> {
        fn visit_place(&mut self, place: &Place, context: crate::mir::visit::PlaceContext, at: Location) {
            let mut ty = PlaceTy::from_ty(self.body.local_decls[place.local.0 as usize].ty);
            for elem in &place.projection {
                if matches!(elem, PlaceElem::Subtype(_)) { self.stop("Subtype must be removed before borrowck"); return; }
                let Some(next) = ty.projection_ty(self.tcx, elem) else { self.stop("MIR projected type is unavailable"); return; };
                ty = next;
            }
            self.super_place(place, context, at);
        }
        fn visit_rvalue(&mut self, rv: &Rvalue, at: Location) {
            match rv {
                Rvalue::Aggregate(kind, _) if matches!(&**kind, AggregateKind::Closure(id, _) | AggregateKind::Coroutine(id, _) if self.nested.is_some_and(|n| n.get(*id).is_none())) =>
                    self.stop("closure aggregate without its nested body's used-mut upvars"),
                Rvalue::CopyForDeref(_) => self.stop("runtime rvalue in borrowck MIR"),
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
            if matches!(term.kind, TerminatorKind::Yield { .. }) && self.body.coroutine.is_none() { self.stop("Yield outside a coroutine body"); }
            self.super_terminator(term, at);
        }
    }
    let mut check = Check { tcx, body, nested, stops: unsupported };
    for (b, block) in body.basic_blocks.iter().enumerate() { check.visit_basic_block_data(BasicBlock(b as u32), block); }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadKind { Copy, Borrow(BorrowKind) }
#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteKind { MutableBorrow(BorrowKind), Mutate, Move, StorageDeadOrDrop, Replace }
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadOrWrite { Read(ReadKind), Write(WriteKind), Reservation(WriteKind), Activation(WriteKind, usize) }
use ReadOrWrite::*;
impl WriteKind {
    fn access_kind(self) -> AccessKind {
        match self {
            WriteKind::MutableBorrow(k) => AccessKind::MutableBorrow(k), WriteKind::Mutate => AccessKind::Mutate,
            WriteKind::Move => AccessKind::Move, WriteKind::StorageDeadOrDrop => AccessKind::StorageDeadOrDrop, WriteKind::Replace => AccessKind::Replace,
        }
    }
}
impl ReadOrWrite {
    fn access_kind(self) -> AccessKind {
        match self {
            Read(ReadKind::Copy) => AccessKind::Copy, Read(ReadKind::Borrow(k)) => AccessKind::Borrow(k),
            Write(k) | Reservation(k) | Activation(k, _) => k.access_kind(),
        }
    }
}

struct MirBorrowckCtxt<'a> {
    prog: &'a Program, tcx: &'a Tcx, body: &'a Body, moves: &'a MoveData, borrows: &'a BorrowSet,
    cfg: CheckCfg, result: MirBorrowckResult,
    access_errors: Vec<(Place, Span)>, reservation_errors: Vec<Place>, uninitialized_errors: BitSet,
    /// lib.rs:779 move_errors: illegal moves, grouped and reported after the walk.
    move_errors: Vec<MoveError>,
    /// diagnostics/mod.rs:143 buffered_move_errors: one E0382 per set of move-outs, in
    /// first-insertion order; a later error for the same set replaces the earlier one.
    buffered_move_errors: Vec<(Vec<usize>, Place, BorrowckError)>,
    /// diagnostics/mod.rs:164 buffered_mut_errors: (binding span, index into errors, count).
    buffered_mut_errors: Vec<(Span, usize, usize)>,
    /// Relaxed-Rust field views (field_views/check_hook.rs) and the loan the access being
    /// checked creates (its parts are narrowed by the callee's summary).
    fv: Option<fv_hook::FvRun<'a>>, fv_own: Option<usize>,
    /// Relaxed Rust D3 (disjoint indices, field_views/disjoint.rs): on for the body's crate; the
    /// single-use borrow temps that feed calls, and the locals whose address is taken (lazily).
    d3: bool, d3_feeds: Option<crate::fxhash::HashMap<Local, super::field_views::Feed>>, d3_taken: Option<BitSet>,
    /// The nested bodies' `used_mut_upvars`.
    nested: &'a NestedUsedMut,
}
impl MirBorrowckCtxt<'_> {
    fn error_with(&mut self, kind: ErrorKind, span: Span, detail: Detail) {
        let error = BorrowckError::with(kind, span, detail);
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
            StatementKind::FakeRead(_, place) => self.check_if_path_or_subpath_is_moved(place, InitAction::Use, at, span, state),
            StatementKind::StorageDead(local) => self.access_place(&Place::local(*local), Shallow(None),
                Write(WriteKind::StorageDeadOrDrop), true, at, span, state),
            StatementKind::Intrinsic(NonDivergingIntrinsic::Assume(op)) => self.consume_operand(op, at, span, state),
            StatementKind::AscribeUserType(..) | StatementKind::PlaceMention(..)
            | StatementKind::ConstEvalCounter | StatementKind::StorageLive(..) => {}
            // does not affect borrowck itself: the lint `tail_expr_drop_order` for borrows still live there
            StatementKind::BackwardIncompatibleDropHint { place, reason: BackwardIncompatibleDropReason::Edition2024 } => self.check_backward_incompatible_drop(place, state),
            _ => unreachable!("preflight rejected invalid phase"),
        }
    }
    /// lib.rs `check_backward_incompatible_drop`: `BackwardIncompatibleDropHint`s are the places whose
    /// drop moves in edition 2024; a borrow still live there makes the lint `tail_expr_drop_order`
    /// (borrowck would fail at the new drop location). Reported against the crate.
    fn check_backward_incompatible_drop(&mut self, place: &Place, state: BorrowckDomain<'_>) {
        if !crate::lint::enabled() { return; }
        let Some(pty) = place.ty(self.body, self.tcx) else { return };
        // a type that does not need drop is treated like a `StorageDead`
        let env = crate::tcx::TypingEnv::fully_monomorphized(self.prog);
        let depth = if self.tcx.needs_drop(self.prog, env, pty.ty) { AccessDepth::Drop } else { Shallow(None) };
        let mut found: Option<Span> = None;
        each_borrow_involving_path(self.tcx, self.body, (depth, place), self.borrows, |i| state.borrows.contains(i), |_, borrow| {
            if matches!(borrow.kind, BorrowKind::Fake(_)) { return ControlFlow::Continue(()); }
            found = Some(self.span_at(borrow.reserve_location));
            ControlFlow::Break(())
        });
        if let Some(span) = found {
            let owner = self.body.source.owner;
            if !crate::lint::external(self.prog, span.file, span.lo) {
                crate::lint::emit(self.prog, owner, &crate::lint_tail_drop::TAIL_EXPR_DROP_ORDER, span.file, span.lo, "relative drop order changing in Rust 2024");
            }
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
    fn visit_after_primary_terminator_effect(&mut self, term: &Terminator, at: Location, state: BorrowckDomain<'_>) {
        if matches!(term.kind, TerminatorKind::Return | TerminatorKind::UnwindResume | TerminatorKind::TailCall { .. } | TerminatorKind::CoroutineDrop)
            && matches!(self.borrows.locals_state_at_exit(), LocalsStateAtExit::AllAreInvalidated) {
            for i in state.borrows.iter() { self.check_for_invalidation_at_exit(&self.borrows[i], at, term.source_info.span); }
        }
        // lib.rs visit_terminator_after_primary_effect: a movable coroutine may be moved at a yield,
        // so no borrow of its local data may be live across it.
        if matches!(term.kind, TerminatorKind::Yield { .. }) && self.movable_coroutine() {
            for i in state.borrows.iter() { self.check_for_local_borrow(&self.borrows[i], at, term.source_info.span); }
        }
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
                self.fv_own = self.fv_own_loan(at, place);
                self.access_place(place, depth, access, false, at, span, state);
                self.fv_own = None;
                let action = if *kind == BorrowKind::Fake(FakeBorrowKind::Shallow) { InitAction::MatchOn } else { InitAction::Borrow };
                self.check_if_path_or_subpath_is_moved(place, action, at, span, state);
            }
            Rvalue::RawPtr(kind, place) => {
                let (depth, access) = match kind {
                    RawPtrKind::Mut => (Deep, Write(WriteKind::MutableBorrow(BorrowKind::Mut { kind: MutBorrowKind::Default }))),
                    RawPtrKind::Const => (Deep, Read(ReadKind::Borrow(BorrowKind::Shared))),
                    RawPtrKind::FakeForPtrMetadata => (Shallow(Some(ArtificialField::ArrayLength)), Read(ReadKind::Copy)),
                };
                self.access_place(place, depth, access, false, at, span, state);
                self.check_if_path_or_subpath_is_moved(place, InitAction::Borrow, at, span, state);
            }
            Rvalue::Use(op) | Rvalue::Repeat(op, _) | Rvalue::UnaryOp(_, op) | Rvalue::Cast(_, op, _) => self.consume_operand(op, at, span, state),
            Rvalue::BinaryOp(_, ops) => {
                self.consume_operand(&ops.0, at, span, state); self.consume_operand(&ops.1, at, span, state);
            }
            Rvalue::Discriminant(place) => {
                self.access_place(place, Shallow(None), Read(ReadKind::Copy), false, at, span, state);
                self.check_if_path_or_subpath_is_moved(place, InitAction::Use, at, span, state);
            }
            Rvalue::Aggregate(kind, ops) => {
                // Report back which mutable upvars were moved into the closure and mutated by it.
                if let AggregateKind::Closure(id, _) | AggregateKind::Coroutine(id, _) = &**kind {
                    for &field in self.nested.get(*id).unwrap_or(&[]) { self.propagate_closure_used_mut_upvar(&ops[field.0 as usize]); }
                }
                for op in ops { self.consume_operand(op, at, span, state); }
            }
            Rvalue::ThreadLocalRef(_) => {}
            Rvalue::CopyForDeref(_) => unreachable!("preflight rejected runtime rvalue"),
        }
    }
    fn consume_operand(&mut self, op: &Operand, at: Location, span: Span, state: BorrowckDomain<'_>) {
        let (place, access, allow) = match op {
            Operand::Constant(_) | Operand::RuntimeChecks(_) => return,
            Operand::Copy(place) => (place, Read(ReadKind::Copy), false),
            Operand::Move(place) => { self.check_movable_place(place, at); (place, Write(WriteKind::Move), true) }
        };
        self.access_place(place, Deep, access, allow, at, span, state);
        self.check_if_path_or_subpath_is_moved(place, InitAction::Use, at, span, state);
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
        let permission = self.check_access_permissions(place, access, allow, at, span, state);
        let conflict = self.check_access_for_conflict(place, depth, access, at, span, state);
        if permission || conflict { self.access_errors.push((place.clone(), span)); }
    }
    /// Whether `access` of `place` conflicts with the borrow `i` of `bplace`/`bkind`: the place
    /// overlap and the access/borrow kind rules (rustc check_access_for_conflict's filter).
    fn pair_conflicts(&self, place: &Place, depth: AccessDepth, access: ReadOrWrite, bplace: &Place, bkind: BorrowKind, i: usize, at: Location) -> bool {
        if !borrow_conflicts_with_place(self.tcx, self.body, bplace, bkind, place, depth, PlaceConflictBias::Overlap) { return false; }
        match (access, bkind) {
            (Activation(_, activating), _) if i == activating => false,
            (Read(_), BorrowKind::Shared | BorrowKind::Fake(_))
            | (Read(ReadKind::Borrow(BorrowKind::Fake(FakeBorrowKind::Shallow))), BorrowKind::Mut { .. })
            | (Reservation(_), BorrowKind::Shared | BorrowKind::Fake(_))
            | (Write(WriteKind::Move), BorrowKind::Fake(FakeBorrowKind::Shallow)) => false,
            (Read(_), BorrowKind::Mut { .. }) => is_active(|a, b| self.cfg.dominates(a, b), &self.borrows[i], at),
            _ => true,
        }
    }
    fn check_access_for_conflict(&mut self, place: &Place, depth: AccessDepth, access: ReadOrWrite, at: Location, span: Span, state: BorrowckDomain<'_>) -> bool {
        let mut candidates = vec![];
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
                candidates.push(i);
                // Without field views the first conflicting borrow is the error.
                if self.fv.is_none() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
            });
        for i in candidates {
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
                    WriteKind::StorageDeadOrDrop => { self.lifetime_error(borrow, place, at, span); return true; }
                },
            };
            let detail = Detail::Conflict { place: place.clone(), access: access.access_kind(), location: at, borrow: BorrowInfo::from(borrow) };
            let mut error = BorrowckError::with(kind, span, detail);
            // Relaxed Rust D3: two borrows of distinct elements of one array / slice / Vec.
            if let Some(outcome) = self.disjoint_outcome(i, place, access, at) {
                error.fv = Some(Box::new(FvNote::Disjoint { proven: outcome.record.proven }));
                let r = super::field_views::RelaxedRecord::Disjoint(outcome.record);
                if !self.result.relaxed_records.iter().any(|x| x.same_site(&r)) { self.result.relaxed_records.push(r); }
                if !self.result.fv_only.contains(&error) { self.result.fv_only.push(error); }
                if matches!(access, Reservation(_)) && !self.reservation_errors.contains(place) { self.reservation_errors.push(place.clone()); }
                continue;
            }
            // Relaxed Rust: the conflict may disappear under the callees' field views.
            if let Some(outcome) = self.fv_outcome(i, place, depth, access, at, span) {
                error.fv = outcome.note.map(Box::new);
                if let Some(record) = outcome.record { self.result.fv_records.push(record); }
                if outcome.accepted {
                    if self.fv.as_ref().is_some_and(|f| f.views.relaxed) {
                        if !self.result.fv_only.contains(&error) { self.result.fv_only.push(error); }
                        // As for a reported reservation conflict: its activation is the same finding.
                        if matches!(access, Reservation(_)) && !self.reservation_errors.contains(place) { self.reservation_errors.push(place.clone()); }
                        continue;
                    }
                }
            }
            if matches!(access, Reservation(_)) { self.reservation_errors.push(place.clone()); }
            if !self.result.errors.contains(&error) { self.result.errors.push(error); }
            return true;
        }
        false
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
    fn lifetime_error(&mut self, borrow: &BorrowData, dropped: &Place, at: Location, _drop_span: Span) {
        let borrow_span = self.span_at(borrow.reserve_location);
        let key = (Place::local(borrow.borrowed_place.local),borrow_span);
        if self.access_errors.contains(&key) { return; }
        self.access_errors.push(key);
        let decl = &self.body.local_decls[borrow.borrowed_place.local.0 as usize];
        let kind = if matches!(decl.local_info, LocalInfo::StaticRef { is_thread_local: true, .. }) { ThreadLocalEscapes }
            else if !dropped.projection.starts_with(&borrow.borrowed_place.projection) && self.has_destructor_prefix(&borrow.borrowed_place) { BorrowAcrossDestructor }
            else if decl.kind == LocalKind::Temp { TemporaryDropped } else { DoesNotLiveLongEnough };
        let span = if kind == TemporaryDropped { decl.source_info.span } else { self.span_at(borrow.reserve_location) };
        self.error_with(kind, span, Detail::Lifetime { borrow: BorrowInfo::from(borrow), dropped_place: dropped.clone(), location: at });
    }
    /// lib.rs `movable_coroutine`: the body is a coroutine whose movability is `Movable`.
    fn movable_coroutine(&self) -> bool {
        self.body.coroutine.as_ref().is_some_and(|c| c.kind.movability() == Movability::Movable)
    }
    /// lib.rs `check_for_local_borrow` (E0626): `borrow` of local data live at a yield.
    fn check_for_local_borrow(&mut self, borrow: &BorrowData, yield_location: Location, _yield_span: Span) {
        if !super::path_utils::borrow_of_local_data(&borrow.borrowed_place) { return; }
        let span = self.span_at(borrow.reserve_location);
        let key = (Place::local(borrow.borrowed_place.local), span);
        if self.access_errors.contains(&key) { return; }
        self.access_errors.push(key);
        self.error_with(BorrowAcrossYield, span, Detail::BorrowAcrossYield { borrow: BorrowInfo::from(borrow), yield_location });
    }
    fn check_for_invalidation_at_exit(&mut self, borrow: &BorrowData, at: Location, span: Span) {
        let mut root = Place::local(borrow.borrowed_place.local);
        let thread_local = matches!(self.body.local_decls[root.local.0 as usize].local_info, LocalInfo::StaticRef { is_thread_local: true, .. });
        if thread_local { root.projection.push(PlaceElem::Deref); }
        if borrow_conflicts_with_place(self.tcx, self.body, &borrow.borrowed_place, borrow.kind, &root,
            if thread_local { Deep } else { Shallow(None) }, PlaceConflictBias::Overlap) { self.lifetime_error(borrow, &root, at, span); }
    }
    fn first_reaching_init(&self, local: Local, state: BorrowckDomain<'_>) -> Option<usize> {
        let path = self.moves.rev_lookup.find_local(local)?;
        self.moves.init_path_map[path].iter().copied().find(|&i| state.ever_inits.contains(i))
    }
    fn add_used_mut(&mut self, root: &Place, allow: bool, state: BorrowckDomain<'_>) {
        if root.projection.is_empty() {
            // If the local may have been initialized and is now being mutated, the mutation may be a
            // reassignment, which justifies the `mut`.
            if !allow && self.first_reaching_init(root.local, state).is_some() { self.result.used_mut.insert(root.local.0 as usize); }
        } else if !allow {
            if let Some(field) = self.upvar_field(root) { self.note_used_mut_upvar(field); }
        }
    }
    fn upvar_field(&self, place: &Place) -> Option<FieldIdx> { super::path_utils::is_upvar_field_projection(self.tcx, self.body, place) }
    fn note_used_mut_upvar(&mut self, field: FieldIdx) {
        if !self.result.used_mut_upvars.contains(&field) { self.result.used_mut_upvars.push(field); }
    }
    /// lib.rs `propagate_closure_used_mut_upvar`: a mutable upvar moved into a closure that mutates it
    /// marks the variable it came from as used mutably.
    fn propagate_closure_used_mut_upvar(&mut self, operand: &Operand) {
        let (Operand::Move(place) | Operand::Copy(place)) = operand else { return };
        if place.projection.is_empty() && !self.body.local_decls[place.local.0 as usize].is_user_variable() {
            if matches!(self.tcx.tys.kind(self.body.local_decls[place.local.0 as usize].ty), TyKind::Ref(true, ..) | TyKind::Ptr(true, _)) {
                // The variable will be marked as mutable by the borrow.
                return;
            }
            // A `move` closure inside a non-move closure whose body mutates: the capture may be copied from
            // a shared reference, so search the local the capture comes from.
            // `let mut i = 0; || { move || { i += 1; }; };`
            let path = self.moves.rev_lookup.find_local(place.local).expect("temporary should be tracked");
            let [init_index] = self.moves.init_path_map[path][..] else { panic!("temporary should be initialized exactly once") };
            let crate::mir::move_paths::InitLocation::Statement(loc) = self.moves.inits[init_index].location else { panic!("temporary initialized in arguments") };
            let stmt = &self.body.basic_blocks[loc.block.0 as usize].statements[loc.statement_index];
            match &stmt.kind {
                StatementKind::Assign(_, Rvalue::Ref(_, _, source) | Rvalue::Use(Operand::Copy(source) | Operand::Move(source))) => {
                    let source = source.clone();
                    self.propagate_used_mut_place(&source);
                }
                _ => panic!("closures should only capture user variables or references to user variables"),
            }
        } else {
            self.propagate_used_mut_place(place);
        }
    }
    fn propagate_used_mut_place(&mut self, place: &Place) {
        // (a) modifying something through a mut-ref, (b) something local to our parent, (c) a nested
        // closure modifying a path starting from a place captured by its parent.
        if let Some(field) = self.upvar_field(place) { self.note_used_mut_upvar(field); return; }
        for i in (0..place.projection.len()).rev() {
            let base = Place { local: place.local, projection: place.projection[..i].to_vec() };
            if place.projection[i] == PlaceElem::Deref && matches!(self.tcx.tys.kind(self.ty(&base)), TyKind::Ref(true, ..)) { return; }
            if let Some(field) = self.upvar_field(&base) { self.note_used_mut_upvar(field); return; }
        }
        self.result.used_mut.insert(place.local.0 as usize);
    }
    fn check_access_permissions(&mut self, place: &Place, access: ReadOrWrite, allow: bool, at: Location, span: Span, state: BorrowckDomain<'_>) -> bool {
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
        let Some(init) = self.first_reaching_init(place.local, state) else { return false; };
        let detail = Detail::Mutability { place: place.clone(), the_place_err: the_place_err.clone(), access: access.access_kind(), location: at, init: Some(init) };
        let code = if borrow { "E0596" } else if place.projection.is_empty() { "E0384" } else { "E0594" };
        if self.relaxed_mut(&the_place_err, span, code) { return false; }
        if !borrow {
            self.error_with(if place.projection.is_empty() { Reassigned } else { AssignImmutable }, span, detail);
        } else {
            self.report_mutable_borrow_error(&the_place_err, span, detail);
        }
        true
    }
    /// Relaxed Rust `mut`: a plain by-value user binding that is only missing its `mut` is accepted, and the
    /// edit that adds it (at the binding, from the MIR's own binding span) is recorded for `makepad fix --to-strict`.
    /// A `ref` binding, `self`, a closure environment or upvar, and a place behind a reference keep the error.
    fn relaxed_mut(&self, err: &Place, span: Span, code: &'static str) -> bool {
        use crate::mir::{BindingForm, ByRef, LocalInfo, LocalKind, VarBindingForm};
        if !err.projection.is_empty() || err.local.0 == 1 && matches!(self.body.source.kind, BodyKind::Closure(_)) { return false; }
        let prog = self.prog;
        let file = span.file as usize;
        let Some(&k) = prog.file_crate.get(file) else { return false };
        if !prog.crates[k as usize].relaxed(prog).iter().any(|f| f == "mut") { return false; }
        let decl = &self.body.local_decls[err.local.0 as usize];
        if !matches!(decl.kind, LocalKind::UserVar | LocalKind::Arg) { return false; }
        let LocalInfo::User(BindingForm::Var(VarBindingForm { binding_mode: crate::mir::BindingMode(ByRef::No, Mutability::Not), .. })) = &decl.local_info else { return false };
        let Some(name) = self.body.var_debug_info.iter().find(|v| v.place.local == err.local && v.place.projection.is_empty()).map(|v| v.name.as_str()) else { return false };
        if name == "self" { return false; }
        let pos = decl.source_info.span.lo;
        if decl.source_info.span.file != span.file { return false; }
        let at = |p: u32| { let (l, c) = prog.files[file].line_col(p); format!("{}:{}:{}", prog.file_paths[file], l, c) };
        crate::relaxed::note(crate::relaxed::Note {
            at: at(span.lo), feature: "mut", code,
            msg: format!("accepted only with relaxed `mut`: `{name}` is not declared as mutable; will write: `mut {name}` at {}", at(pos)),
            fix: Some((prog.file_paths[file].clone(), vec![crate::relaxed::Edit { lo: pos, hi: pos, text: "mut ".to_string() }])),
        });
        true
    }
    // diagnostics/mutability_errors.rs:226-250,659: a second mutable borrow of the same
    // immutable binding joins the first error, whose primary span becomes the binding.
    fn report_mutable_borrow_error(&mut self, the_place_err: &Place, span: Span, detail: Detail) {
        let decl = &self.body.local_decls[the_place_err.local.0 as usize];
        let can_be_made_mutable = decl.can_be_made_mutable();
        if !the_place_err.projection.is_empty() || !can_be_made_mutable {
            self.result.errors.push(BorrowckError::with(BorrowImmutable, span, detail));
            return;
        }
        let binding_span = decl.source_info.span;
        if let Some(entry) = self.buffered_mut_errors.iter_mut().find(|(s, ..)| *s == binding_span) {
            entry.2 += 1;
            if entry.2 == 2 { self.result.errors[entry.1].span = binding_span; }
            return;
        }
        self.buffered_mut_errors.push((binding_span, self.result.errors.len(), 1));
        self.result.errors.push(BorrowckError::with(BorrowImmutable, span, detail));
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
                TyKind::Ref(true, ..) => {
                    // Mutably borrowed data is mutable, but only with a unique path to the `&mut`: a by-reference
                    // upvar keeps the mode of the access.
                    let mode = match self.upvar_field(place) { Some(field) if self.body.upvars[field.0 as usize].is_by_ref() => allow, _ => true };
                    self.is_mutable(&base, mode)
                }
                TyKind::Ptr(true, _) => Ok((place.clone(), allow)),
                TyKind::Adt(def, ..) if self.tcx.adts[def].is_box() => self.is_mutable(&base, allow),
                _ => panic!("invalid MIR dereference"),
            }
        } else if let Some(field) = self.upvar_field(place) {
            // An upvar field of a closure environment: it is mutable if it was declared so (or its variable's
            // mutation is allowed), and if the environment itself is (not an `Fn` closure). The mutability is
            // blamed on `place` so that the outer variable is registered as mutable.
            match (self.body.upvars[field.0 as usize].mutability, allow) {
                (Mutability::Not, false) => Err(place.clone()),
                (Mutability::Not, true) | (Mutability::Mut, _) => { self.is_mutable(&base, allow)?; Ok((place.clone(), allow)) }
            }
        } else { self.is_mutable(&base, allow) }
    }
    fn check_movable_place(&mut self, place: &Place, at: Location) {
        if let Some(kind) = illegal_move(self.tcx, self.body, place) { self.move_errors.push(MoveError { place: place.clone(), location: at, kind }); }
    }
    fn exact_path(&self, place: &Place) -> Option<usize> {
        match self.moves.rev_lookup.find(place) { LookupResult::Exact(i) => Some(i), LookupResult::Parent(_) => None }
    }
    fn check_if_full_path_is_moved(&mut self, place: &Place, action: InitAction, at: Location, span: Span, state: BorrowckDomain<'_>) {
        let path = match self.moves.rev_lookup.find(place) {
            LookupResult::Exact(i) | LookupResult::Parent(Some(i)) => i,
            LookupResult::Parent(None) => panic!("missing move path for MIR local"),
        };
        if state.uninits.contains(path) {
            let prefix = self.moves.move_paths[path].place.clone();
            self.report_use_of_moved_or_uninitialized(&prefix, place, path, action, at, span);
        }
    }
    fn uninit_descendant(&self, path: usize, state: BorrowckDomain<'_>) -> Option<usize> {
        find_in_move_path_or_its_descendants(self.moves, path, |i| state.uninits.contains(i))
    }
    fn check_if_path_or_subpath_is_moved(&mut self, place: &Place, action: InitAction, at: Location, span: Span, state: BorrowckDomain<'_>) {
        self.check_if_full_path_is_moved(place, action, at, span, state);
        if let Some((PlaceElem::Subslice { from, to, from_end: false }, base)) = last_projection(place) {
            if matches!(self.tcx.tys.kind(self.ty(&base)), TyKind::Array(..)) {
                if let Some(path) = self.exact_path(&base) {
                    let mut child = self.moves.move_paths[path].first_child;
                    while let Some(i) = child {
                        if let Some(PlaceElem::ConstantIndex { offset, from_end: false, .. }) = self.moves.move_paths[i].place.projection.last() {
                            if (from..to).contains(offset) {
                                if let Some(i) = self.uninit_descendant(i, state) { self.report_use_of_moved_or_uninitialized(&base, &base, i, action, at, span); return; }
                            }
                        }
                        child = self.moves.move_paths[i].next_sibling;
                    }
                }
                return;
            }
        }
        if let Some(path) = self.exact_path(place) {
            if let Some(i) = self.uninit_descendant(path, state) { self.report_use_of_moved_or_uninitialized(place, place, i, action, at, span); }
        }
    }
    fn check_if_assigned_path_is_moved(&mut self, place: &Place, at: Location, span: Span, state: BorrowckDomain<'_>) {
        for i in (0..place.projection.len()).rev() {
            let base = Place { local: place.local, projection: place.projection[..i].to_vec() };
            match place.projection[i] {
                PlaceElem::Deref => { self.check_if_full_path_is_moved(&base, InitAction::Use, at, span, state); break; }
                PlaceElem::Subslice { .. } => panic!("assignment to MIR subslice"),
                PlaceElem::Field(..) => {
                    let ty = self.tcx.tys.kind(self.ty(&base));
                    if matches!(ty, TyKind::Adt(def, ..) if self.tcx.adt_destructor(*def).is_some()) {
                        self.check_if_path_or_subpath_is_moved(&base, InitAction::Assignment, at, span, state); break;
                    }
                    if matches!(ty, TyKind::Adt(..) | TyKind::Tuple(_)) {
                        let mut shortest = None;
                        for prefix in prefixes(&base, PrefixSet::Shallow) {
                            if let Some(path) = self.exact_path(&prefix) { if state.uninits.contains(path) { shortest = Some(path); } }
                        }
                        if let Some(path) = shortest {
                            if matches!(ty, TyKind::Adt(def, ..) if self.tcx.adts[def].is_union())
                                && self.moves.path_map[path].iter().any(|&i| self.cfg.is_predecessor(self.body, self.moves.moves[i].source, at)) { continue; }
                            let prefix = self.moves.move_paths[path].place.clone();
                            self.report_use_of_moved_or_uninitialized(&prefix, &base, path, InitAction::PartialAssignment, at, span);
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
    fn report_use_of_moved_or_uninitialized(&mut self, moved_place: &Place, used_place: &Place, mpi: usize, action: InitAction, at: Location, span: Span) {
        let sites = self.get_moved_indexes(at, mpi);
        let move_out_indices: Vec<usize> = sites.iter().map(|s| s.moi).collect();
        let detail = Detail::Moved { action, moved_place: moved_place.clone(), used_place: used_place.clone(), location: at, mpi, moves: sites };
        if move_out_indices.is_empty() {
            if self.uninitialized_errors.insert(used_place.local.0 as usize) {
                self.error_with(Uninitialized, span, detail);
            }
            return;
        }
        let error = BorrowckError::with(Moved, span, detail);
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
    fn get_moved_indexes(&self, at: Location, mpi: usize) -> Vec<MoveSite> {
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
        let mut dfs_iter = |result: &mut Vec<MoveSite>, location: Location, traversed_back_edge: bool| -> bool {
            if visited.contains(&location) { return true; }
            visited.push(location);
            let data = &self.body.basic_blocks[location.block.0 as usize];
            // Only moves written by the user count: not the move-outs of StorageDead.
            if !matches!(data.statements.get(location.statement_index).map(|s| &s.kind), Some(StatementKind::StorageDead(_))) {
                for &moi in &self.moves.loc_map[location] {
                    if mpis.contains(&self.moves.moves[moi].path) { result.push(MoveSite { moi, traversed_back_edge }); return true; }
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
            if dfs_iter(&mut result, location, false) { continue; }
            let predecessors = self.cfg.predecessor_locations(self.body, location);
            if predecessors.is_empty() { reached_start = true; }
            for predecessor in predecessors {
                if self.cfg.location_dominates(location, predecessor) { back_edge_stack.push(predecessor); } else { stack.push(predecessor); }
            }
        }
        if (is_argument || !reached_start) && result.is_empty() {
            while let Some(location) = back_edge_stack.pop() {
                if dfs_iter(&mut result, location, true) { continue; }
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
    MovesFromPlace { span: Span, kind: ErrorKind, original: (Place, Location) },
    MovesFromValue { span: Span, move_from: usize, kind: ErrorKind, original: (Place, Location) },
    OtherIllegalMove { span: Span, kind: ErrorKind, original: (Place, Location) },
}
// ported from N:rustc_borrowck/src/diagnostics/move_errors.rs @8d1a764
// report_move_errors/group_move_errors/append_to_grouped_errors/append_binding_error:103-232.
// The moves of one pattern-match are grouped into one error at the matched expression
// (or the `let` statement); the binding lists only label the error and are not kept.
// `moves` is None for promoted bodies, whose locals are never user bindings.
fn report_move_errors(body: &Body, moves: Option<&MoveData>, errors: Vec<MoveError>, out: &mut Vec<BorrowckError>) {
    let mut grouped: Vec<GroupedMoveError> = vec![];
    for MoveError { place: original_path, location, kind } in errors {
        let original = (original_path.clone(), location);
        let block = &body.basic_blocks[location.block.0 as usize];
        let stmt = block.statements.get(location.statement_index);
        if let Some(Statement { kind: StatementKind::Assign(place, Rvalue::Use(Operand::Move(move_from))), source_info }) = stmt {
            if place.projection.is_empty() {
                if let LocalInfo::User(BindingForm::Var(VarBindingForm { opt_match_place: Some((opt_match_place, match_span)), .. })) = &body.local_decls[place.local.0 as usize].local_info {
                    let moves = moves.expect("promoted MIR has no user bindings");
                    append_binding_error(&mut grouped, moves, kind, move_from, opt_match_place.as_ref(), *match_span, source_info.span, original);
                    continue;
                }
            }
        }
        // move_spans: the statement or terminator span (closure captures are stopped).
        let span = stmt.map_or_else(|| block.terminator.as_ref().unwrap().source_info.span, |s| s.source_info.span);
        grouped.push(GroupedMoveError::OtherIllegalMove { span, kind, original });
    }
    for error in grouped {
        let (GroupedMoveError::MovesFromPlace { span, kind, original } | GroupedMoveError::MovesFromValue { span, kind, original, .. }
            | GroupedMoveError::OtherIllegalMove { span, kind, original }) = error;
        let error = BorrowckError::with(kind, span, Detail::IllegalMove { place: original.0, location: original.1 });
        if !out.contains(&error) { out.push(error); }
    }
}
fn append_binding_error(grouped: &mut Vec<GroupedMoveError>, moves: &MoveData, kind: ErrorKind, move_from: &Place,
    match_place: Option<&Place>, match_span: Span, statement_span: Span, original: (Place, Location)) {
    let from_simple_let = match_place.is_none();
    let match_place = match_place.unwrap_or(move_from);
    match moves.rev_lookup.find(match_place) {
        // Error with the match place.
        LookupResult::Parent(_) => {
            if grouped.iter().any(|ge| matches!(ge, GroupedMoveError::MovesFromPlace { span, .. } if *span == match_span)) { return; }
            // Don't need to point to x in let x = ... .
            let span = if from_simple_let { statement_span } else { match_span };
            grouped.push(GroupedMoveError::MovesFromPlace { span, kind, original });
        }
        // Error with the pattern.
        LookupResult::Exact(_) => {
            let LookupResult::Parent(Some(mpi)) = moves.rev_lookup.find(move_from) else {
                unreachable!("move_from should be a projection from match_place");
            };
            if grouped.iter().any(|ge| matches!(ge, GroupedMoveError::MovesFromValue { span, move_from, .. } if *span == match_span && *move_from == mpi)) {
                return;
            }
            grouped.push(GroupedMoveError::MovesFromValue { span: match_span, move_from: mpi, kind, original });
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
        // A promoted body is only searched for illegal moves: what the closures it creates mutate is not asked.
        check_supported(tcx, promoted, None, &mut result.unsupported);
        if !result.unsupported.is_empty() { continue; }
        let mut visitor = Moves { tcx, body: promoted, errors: vec![] };
        for (b, block) in promoted.basic_blocks.iter().enumerate() { visitor.visit_basic_block_data(BasicBlock(b as u32), block); }
        let first = result.errors.len();
        report_move_errors(promoted, None, visitor.errors, &mut result.errors);
        // The places of these errors are in the promoted body's local numbering; the diagnostics of the
        // parent cannot describe them, so they keep their code and span only.
        for error in &mut result.errors[first..] { error.detail = Detail::None; }
        check_promoted_moves(tcx, promoted, result);
    }
}

// Relaxed-Rust field views and extended two-phase borrows (a child module: it extends the
// private checker state).
#[path = "field_views/check_hook.rs"]
mod fv_hook;
#[path = "field_views/disjoint.rs"]
mod disjoint;
