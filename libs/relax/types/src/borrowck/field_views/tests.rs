//! Field views on hand-built MIR: the verdicts of the BIR tests (compiler/tests/borrowck/
//! fv_accept.rs, fv_reject.rs) in MIR shape: summaries (direct, transitive, recursive, opaque),
//! narrowing of a borrow fed to a call, extended two-phase borrows, relaxed vs strict reporting.

use super::*;
use crate::borrowck::borrow_set::BorrowSet;
use crate::borrowck::check::mir_borrowck_fv;
use crate::borrowck::constraints::OutlivesConstraintSet;
use crate::borrowck::mir_nll::{compute_regions, LocationMap, RegionInput};
use crate::borrowck::region_infer::values::LivenessValues;
use crate::borrowck::region_infer::{NllRegionVariableOrigin as Origin, RegionDefinition};
use crate::borrowck::type_check::free_region_relations::UniversalRegionRelations;
use crate::borrowck::universal_regions::{RegionClassification as Class, UniversalRegions};
use crate::mir::move_paths::MoveData;
use crate::types::{RegionId, RegionKind, UniverseIndex};

const SRC: &str = "struct S { a: i32, b: i32, p: *mut i32 }\nfn bump(s: &mut S) {}\nfn clear(s: &mut S) {}\nfn bump_twice(s: &mut S) {}\nfn rec(s: &mut S) {}\nfn whole(s: &mut S) -> &mut S { s }\nfn raw(s: &mut S) {}\nfn f(s: &mut S) {}\nfn add(s: &mut S, x: i32) {}\nfn count(s: &mut S) -> i32 { 0 }\nfn main() {}\n";

struct Fx { prog: Program, tcx: Tcx }
fn fx() -> Fx {
    let session = std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("relax-fv-{}-{index}.rs", std::process::id()));
    std::fs::write(&path, SRC).unwrap();
    let mut prog = Program::new(&session);
    prog.load_crate("custom", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
    std::fs::remove_file(path).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    Fx { prog, tcx }
}
impl Fx {
    fn def(&self, name: &str) -> DefId { (0..self.prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| self.prog.name(d) == name).unwrap() }
    fn s(&self) -> TyId { self.tcx.tys.intern(TyKind::Adt(self.def("S"), self.tcx.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY)) }
    fn rf(&self, m: bool, t: TyId, var: Option<u32>) -> TyId {
        let r = var.map_or(RegionId::ERASED, |v| self.tcx.tys.intern_region(RegionKind::ReVar(v)));
        self.tcx.tys.intern(TyKind::Ref(m, t, r))
    }
    fn fn_op(&self, name: &str) -> Operand {
        let ty = self.tcx.tys.intern(TyKind::FnDef(self.def(name), self.tcx.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
        Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: Const::Val(ConstValue::ZeroSized, ty) }))
    }
}
struct Bodies(std::collections::HashMap<DefId, Body>);
impl FvBodies for Bodies {
    fn body_of(&self, cx: &EvalCx<'_>, _: &Tcx, d: DefId, _: &[TyId]) -> Option<FvBody> {
        self.0.get(&d).map(|b| FvBody { body: b.clone(), env: cx.typing_env })
    }
}

fn p(i: u32) -> Place { Place::local(Local(i)) }
/// `(*_l).field`
fn fld(l: u32, f: u32, t: TyId) -> Place { Place { local: Local(l), projection: vec![PlaceElem::Deref, PlaceElem::Field(FieldIdx(f), t)] } }
fn deref(l: u32) -> Place { Place { local: Local(l), projection: vec![PlaceElem::Deref] } }
fn at(b: u32, s: usize) -> Location { Location { block: BasicBlock(b), statement_index: s } }
fn st(kind: StatementKind, lo: u32) -> Statement { Statement { kind, source_info: SourceInfo { span: Span { file: 0, lo, hi: lo + 1 }, ..SourceInfo::default() } } }
fn val(ty: TyId) -> Operand { Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: Const::Val(ConstValue::ZeroSized, ty) })) }
fn assign(dst: Place, op: Operand) -> StatementKind { StatementKind::Assign(dst, Rvalue::Use(op)) }
fn borrow(dst: u32, src: Place, region: u32, kind: BorrowKind) -> StatementKind { StatementKind::Assign(p(dst), Rvalue::Ref(Region(region), kind, src)) }
fn call(f: &Fx, name: &str, args: Vec<Operand>, dest: u32, target: u32, span_lo: u32) -> Terminator {
    Terminator { kind: TerminatorKind::Call { func: f.fn_op(name), args, destination: p(dest), target: Some(BasicBlock(target)), unwind: UnwindAction::Continue,
        call_source: CallSource::Normal, fn_span: Span { file: 0, lo: span_lo, hi: span_lo + 1 }, arg_spans: vec![] }, source_info: SourceInfo { span: Span { file: 0, lo: span_lo, hi: span_lo + 1 }, ..SourceInfo::default() } }
}
fn ret() -> Terminator { Terminator { kind: TerminatorKind::Return, source_info: SourceInfo::default() } }
fn body(f: &Fx, owner: &str, tys: &[(TyId, LocalKind)], arg_count: u32, blocks: Vec<BasicBlockData>) -> Body {
    Body { phase: MirPhase::Analysis(AnalysisPhase::Initial), source: MirSource { owner: f.def(owner), kind: BodyKind::Fn }, basic_blocks: blocks,
        local_decls: tys.iter().map(|&(ty, kind)| LocalDecl { ty, mutability: Mutability::Mut, source_info: SourceInfo::default(), user_ty: vec![], local_info: LocalInfo::Boring, kind }).collect(),
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }], span: Span::default(), user_type_annotations: vec![], upvars: vec![], allocs: Default::default(), var_debug_info: vec![],
        arg_count, spread_arg: None, promoted: vec![], coroutine: None }
}
fn blk(stmts: Vec<Statement>, term: Terminator) -> BasicBlockData { BasicBlockData { statements: stmts, terminator: Some(term), is_cleanup: false } }

/// `fn <name>(s: &mut S) { (*s).<field> = ..; }` (the unit return is local 0).
fn writer(f: &Fx, name: &str, fields: &[u32]) -> Body {
    let (i, s) = (f.tcx.tys.i32_, f.s());
    let stmts = fields.iter().map(|&k| st(assign(fld(1, k, i), val(i)), 1)).collect();
    body(f, name, &[(f.tcx.tys.unit, LocalKind::Return), (f.rf(true, s, None), LocalKind::Arg)], 1, vec![blk(stmts, ret())])
}
fn env<'a>(f: &'a Fx, cx: &'a EvalCx<'a>, cache: &'a SummaryCx, bodies: &'a Bodies) -> FvEnv<'a> { FvEnv { prog: &f.prog, tcx: &f.tcx, cx, cache, bodies } }
fn eval<'a>(f: &'a Fx, state: &'a crate::tcx::EvalState) -> EvalCx<'a> { EvalCx::new(&f.prog, &f.tcx, TypingEnv::fully_monomorphized(&f.prog), state) }
fn field(f: &Fx, k: u32) -> PlaceElem { PlaceElem::Field(FieldIdx(k), f.tcx.tys.i32_) }

#[test]
fn direct_transitive_recursive_and_opaque_summaries() {
    let f = fx();
    let (i, s) = (f.tcx.tys.i32_, f.s());
    let sref = f.rf(true, s, None);
    let mut m = std::collections::HashMap::new();
    m.insert(f.def("bump"), writer(&f, "bump", &[1]));
    // bump_twice: reborrow the whole pointee, feed it to bump
    m.insert(f.def("bump_twice"), body(&f, "bump_twice", &[(f.tcx.tys.unit, LocalKind::Return), (sref, LocalKind::Arg), (sref, LocalKind::Temp), (f.tcx.tys.unit, LocalKind::Temp)], 1, vec![
        blk(vec![st(borrow(2, deref(1), 9, BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow }), 1)], call(&f, "bump", vec![Operand::Move(p(2))], 3, 1, 2)),
        blk(vec![], ret())]));
    // rec: calls itself with the whole parameter, then writes field 0
    m.insert(f.def("rec"), body(&f, "rec", &[(f.tcx.tys.unit, LocalKind::Return), (sref, LocalKind::Arg), (f.tcx.tys.unit, LocalKind::Temp)], 1, vec![
        blk(vec![], call(&f, "rec", vec![Operand::Copy(p(1))], 2, 1, 1)), blk(vec![st(assign(fld(1, 0, i), val(i)), 2)], ret())]));
    // whole: returns its parameter
    m.insert(f.def("whole"), body(&f, "whole", &[(sref, LocalKind::Return), (sref, LocalKind::Arg)], 1, vec![blk(vec![st(assign(p(0), Operand::Move(p(1))), 1)], ret())]));
    // raw: a raw pointer from the pointee
    let ptr = f.tcx.tys.intern(TyKind::Ptr(true, s));
    m.insert(f.def("raw"), body(&f, "raw", &[(f.tcx.tys.unit, LocalKind::Return), (sref, LocalKind::Arg), (ptr, LocalKind::Temp)], 1, vec![
        blk(vec![st(StatementKind::Assign(p(2), Rvalue::RawPtr(RawPtrKind::Mut, deref(1))), 1)], ret())]));
    // a raw-pointer field: any use is the whole value
    let pty = f.tcx.tys.intern(TyKind::Ptr(true, i));
    m.insert(f.def("clear"), body(&f, "clear", &[(f.tcx.tys.unit, LocalKind::Return), (sref, LocalKind::Arg), (pty, LocalKind::Temp)], 1, vec![
        blk(vec![st(assign(p(2), Operand::Copy(Place { local: Local(1), projection: vec![PlaceElem::Deref, PlaceElem::Field(FieldIdx(2), pty)] })), 1)], ret())]));
    let bodies = Bodies(m);
    let (cache, state) = (SummaryCx::default(), crate::tcx::EvalState::new());
    let cx = eval(&f, &state);
    let e = env(&f, &cx, &cache, &bodies);
    let parts = |d: &str| summaries(&e, f.def(d), &[], 0).as_ref().clone();
    assert_eq!(parts("bump"), vec![Summary::Parts(vec![(vec![field(&f, 1)], true)])]);
    assert_eq!(parts("bump_twice"), parts("bump"));
    // the recursive call has no summary yet: the reference passed on is opaque
    assert_eq!(parts("rec"), vec![Summary::Whole]);
    assert_eq!(parts("whole"), vec![Summary::Whole]);
    assert_eq!(parts("raw"), vec![Summary::Whole]);
    assert_eq!(parts("clear"), vec![Summary::Whole]);
    // an unknown body keeps the whole borrow
    assert_eq!(parts("count"), Vec::<Summary>::new());
}

#[test]
fn closure_captures_summarize_what_the_closure_does_through_a_reference() {
    let f = fx();
    let (i, s) = (f.tcx.tys.i32_, f.s());
    let tys = &f.tcx.tys;
    let sref = f.rf(true, s, None);
    let up = tys.intern(TyKind::Tuple(tys.mk_list(&[sref, i])));
    let closure = tys.intern(TyKind::Closure(0, 7, crate::types::ClosureArgs { parent_args: tys.unit, closure_kind_ty: tys.unit, closure_sig_as_fn_ptr_ty: tys.unit, tupled_upvars_ty: up }));
    let envref = f.rf(true, closure, None);
    // `(*(*_1).0).a = ..; _2 = (*_1).1`
    let cap0 = Place { local: Local(1), projection: vec![PlaceElem::Deref, PlaceElem::Field(FieldIdx(0), sref), PlaceElem::Deref, field(&f, 0)] };
    let cap1 = Place { local: Local(1), projection: vec![PlaceElem::Deref, PlaceElem::Field(FieldIdx(1), i)] };
    let b = body(&f, "f", &[(tys.unit, LocalKind::Return), (envref, LocalKind::Arg), (i, LocalKind::Temp)], 1, vec![
        blk(vec![st(assign(cap0, val(i)), 1), st(assign(p(2), Operand::Copy(cap1)), 2)], ret())]);
    let bodies = Bodies(Default::default());
    let (cache, state) = (SummaryCx::default(), crate::tcx::EvalState::new());
    let cx = eval(&f, &state);
    let e = env(&f, &cx, &cache, &bodies);
    assert_eq!(closure_captures(&e, &b), vec![Summary::Parts(vec![(vec![field(&f, 0)], true)]), Summary::Whole]);
}

/// Region input with `n` existential regions live at the given points (region 0 is 'static).
fn regions(b: &Body, f: &Fx, scopes: &[Vec<Location>]) -> crate::borrowck::mir_nll::MirRegionResult {
    let map = LocationMap::new(b);
    let mut definitions = vec![RegionDefinition { origin: Origin::FreeRegion, universe: UniverseIndex(0), external_name: None }];
    definitions.extend(scopes.iter().map(|_| RegionDefinition { origin: Origin::Existential, universe: UniverseIndex(0), external_name: None }));
    let mut liveness = LivenessValues::new(definitions.len(), map.num_points());
    liveness.add_all_points(0);
    for (r, points) in scopes.iter().enumerate() { for &l in points { liveness.add_point(r + 1, map.point(l)); } }
    let input = RegionInput { universe_causes: vec![], polonius: Default::default(), tainted_by_errors: false,
        relations: UniversalRegionRelations::create(UniversalRegions::new(0, None, vec![(0, Class::Global)]), definitions.len(), &[]),
        definitions, outlives: OutlivesConstraintSet::default(), liveness, type_tests: vec![] };
    compute_regions(&f.tcx, &f.prog, b, input, false)
}

/// `fn f(s: &mut S) { let first = &(*s).<held>; <callee>(&mut *s); use(first) }`
fn overlap_body(f: &Fx) -> Body {
    let (i, s) = (f.tcx.tys.i32_, f.s());
    let two_phase = BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow };
    body(f, "f", &[(f.tcx.tys.unit, LocalKind::Return), (f.rf(true, s, None), LocalKind::Arg), (f.rf(false, i, Some(1)), LocalKind::UserVar),
        (f.rf(true, s, Some(2)), LocalKind::Temp), (f.tcx.tys.unit, LocalKind::Temp), (i, LocalKind::UserVar)], 1, vec![
        blk(vec![st(borrow(2, fld(1, 0, i), 1, BorrowKind::Shared), 10), st(borrow(3, deref(1), 2, two_phase), 20)], call(f, "bump", vec![Operand::Move(p(3))], 4, 1, 30)),
        blk(vec![st(assign(p(5), Operand::Copy(deref(2))), 40)], ret())])
}
fn check(f: &Fx, b: &Body, bodies: &Bodies, relaxed: bool) -> crate::borrowck::check::MirBorrowckResult {
    let moves = MoveData::gather_moves(b, &f.prog, &f.tcx, TypingEnv::fully_monomorphized(&f.prog), &|_| true);
    let mut rg = regions(b, f, &[vec![at(0, 0), at(0, 1), at(0, 2), at(1, 0)], vec![at(0, 1), at(0, 2)]]);
    let (cache, state) = (SummaryCx::default(), crate::tcx::EvalState::new());
    let views = FieldViews::new(&f.prog, TypingEnv::fully_monomorphized(&f.prog), &state, &cache, bodies, relaxed, Default::default());
    let r = mir_borrowck_fv(&f.tcx, &f.prog, b, &mut rg, &moves, Some(&views));
    assert!(r.unsupported.is_empty(), "{:?}", r.unsupported);
    r
}
fn bodies_with(f: &Fx, touched: &[u32]) -> Bodies {
    let mut m = std::collections::HashMap::new();
    m.insert(f.def("bump"), writer(f, "bump", touched));
    Bodies(m)
}

#[test]
fn borrow_fed_to_a_call_borrows_only_the_fields_the_callee_uses() {
    let f = fx();
    let b = overlap_body(&f);
    // the callee uses field b, the live borrow is of field a: accepted only with field views
    let r = check(&f, &b, &bodies_with(&f, &[1]), true);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.fv_only.len(), 1, "{:?}", r.fv_only);
    assert!(matches!(r.fv_only[0].fv.as_deref(), Some(crate::borrowck::borrowck_errors::FvNote::Narrowed { .. })));
    assert_eq!(r.fv_records.len(), 1);
    assert_eq!(r.fv_records[0].span.lo, 10, "the field-level borrow is the one exported");
    assert!(!r.fv_records[0].default);
    // a strict crate reports it as an error that carries the note
    let r = check(&f, &b, &bodies_with(&f, &[1]), false);
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert!(r.fv_only.is_empty());
    assert!(matches!(r.errors[0].fv.as_deref(), Some(crate::borrowck::borrowck_errors::FvNote::Narrowed { .. })));
    // the callee also uses field a: the conflict stands, with a note naming what it touches
    let r = check(&f, &b, &bodies_with(&f, &[0, 1]), true);
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert_eq!(r.errors[0].code(), "E0502");
    assert!(r.fv_only.is_empty() && r.fv_records.is_empty());
    assert!(matches!(r.errors[0].fv.as_deref(), Some(crate::borrowck::borrowck_errors::FvNote::Remaining { .. })));
    // an unknown callee keeps the whole borrow
    let r = check(&f, &b, &Bodies(Default::default()), true);
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert!(r.errors[0].fv.is_none());
}

#[test]
fn without_field_views_the_conflict_is_reported_as_before() {
    let f = fx();
    let b = overlap_body(&f);
    let moves = MoveData::gather_moves(&b, &f.prog, &f.tcx, TypingEnv::fully_monomorphized(&f.prog), &|_| true);
    let mut rg = regions(&b, &f, &[vec![at(0, 0), at(0, 1), at(0, 2), at(1, 0)], vec![at(0, 1), at(0, 2)]]);
    let r = crate::borrowck::check::mir_borrowck(&f.tcx, &f.prog, &b, &mut rg, &moves);
    assert_eq!(r.errors.len(), 1);
    assert_eq!(r.errors[0].code(), "E0502");
    let _ = BorrowSet::build(&f.tcx, &b, true, &moves);
}

/// `fn f(vm: &mut S) { add(vm, count(vm)) }`: `vm` is reserved for `add` and borrowed again
/// by `count` before the call (extended two-phase borrows).
fn two_phase_body(f: &Fx, second_live: bool) -> Body {
    let (i, s) = (f.tcx.tys.i32_, f.s());
    let two_phase = BorrowKind::Mut { kind: MutBorrowKind::TwoPhaseBorrow };
    let mut t = call(f, "count", vec![Operand::Move(p(3))], 4, 1, 30);
    if let TerminatorKind::Call { arg_spans, .. } = &mut t.kind { *arg_spans = vec![Span { file: 0, lo: 28, hi: 33 }]; }
    let mut add = call(f, "add", vec![Operand::Move(p(2)), Operand::Move(p(4))], 5, 2, 50);
    if let TerminatorKind::Call { arg_spans, .. } = &mut add.kind { *arg_spans = vec![Span { file: 0, lo: 12, hi: 14 }, Span { file: 0, lo: 16, hi: 40 }]; }
    let _ = second_live;
    body(f, "f", &[(f.tcx.tys.unit, LocalKind::Return), (f.rf(true, s, None), LocalKind::Arg), (f.rf(true, s, Some(1)), LocalKind::Temp), (f.rf(true, s, Some(2)), LocalKind::Temp),
        (i, LocalKind::Temp), (f.tcx.tys.unit, LocalKind::Temp)], 1, vec![
        blk(vec![st(borrow(2, deref(1), 1, two_phase), 12), st(borrow(3, deref(1), 2, two_phase), 28)], t),
        blk(vec![], add),
        blk(vec![], ret())])
}

#[test]
fn extended_two_phase_borrows_accept_a_reborrow_before_the_call() {
    let f = fx();
    let b = two_phase_body(&f, false);
    let mut m = std::collections::HashMap::new();
    m.insert(f.def("count"), writer(&f, "count", &[0]));
    m.insert(f.def("add"), writer(&f, "add", &[0]));
    let bodies = Bodies(m);
    let moves = MoveData::gather_moves(&b, &f.prog, &f.tcx, TypingEnv::fully_monomorphized(&f.prog), &|_| true);
    // region 1 (the reservation) lives to the call; region 2 (the argument) ends at its call
    let run = |relaxed: bool| {
        let mut rg = regions(&b, &f, &[vec![at(0, 0), at(0, 1), at(0, 2), at(1, 0)], vec![at(0, 1), at(0, 2)]]);
        let (cache, state) = (SummaryCx::default(), crate::tcx::EvalState::new());
        let views = FieldViews::new(&f.prog, TypingEnv::fully_monomorphized(&f.prog), &state, &cache, &bodies, relaxed, Default::default());
        mir_borrowck_fv(&f.tcx, &f.prog, &b, &mut rg, &moves, Some(&views))
    };
    let r = run(true);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    // the reservation of the argument; its activation at the inner call is the same finding
    assert_eq!(r.fv_only.len(), 1, "{:?}", r.fv_only);
    assert!(r.fv_only.iter().all(|e| matches!(e.fv.as_deref(), Some(crate::borrowck::borrowck_errors::FvNote::TwoPhase { .. }))));
    assert_eq!(r.fv_records.len(), 1);
    let hoist = r.fv_records[0].hoist.as_ref().expect("the argument is hoisted");
    assert_eq!(hoist, &vec![Span { file: 0, lo: 12, hi: 14 }]);
    assert_eq!(r.fv_records[0].span, Span { file: 0, lo: 16, hi: 40 });
    // strict: rustc's E0499 is kept, flagged
    let r = run(false);
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert_eq!(r.errors[0].code(), "E0499");
    assert!(matches!(r.errors[0].fv.as_deref(), Some(crate::borrowck::borrowck_errors::FvNote::TwoPhase { .. })));
}

#[test]
fn rendered_notes_and_export_records_name_the_places() {
    use crate::borrowck::diagnostics::{region_name::UniversalNames, render_fv_only, DiagCx};
    let f = fx();
    let mut b = overlap_body(&f);
    b.var_debug_info.push(VarDebugInfo { name: "s".into(), source_info: SourceInfo::default(), place: p(1) });
    let bodies = bodies_with(&f, &[1]);
    let r = check(&f, &b, &bodies, true);
    let moves = MoveData::gather_moves(&b, &f.prog, &f.tcx, TypingEnv::fully_monomorphized(&f.prog), &|_| true);
    let rg = regions(&b, &f, &[vec![], vec![]]);
    let borrows = BorrowSet::build(&f.tcx, &b, true, &moves);
    let names = UniversalNames::default();
    let cx = DiagCx::new(&f.tcx, &f.prog, f.def("f"), &b, &moves, &rg, &borrows, &names);
    let only = render_fv_only(&cx, &r);
    assert_eq!(only.len(), 1);
    assert!(only[0].fv_only);
    assert!(only[0].notes.iter().any(|n| n == "accepted with field views (relaxed Rust): `bump` uses mutably `s.b`"), "{:?}", only[0].notes);
}
