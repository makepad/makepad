//! MIR type check on hand-built analysis MIR, with N01's operations supplied by a
//! recording fake. These establish the consumer side (relations, locations,
//! categories, provenance), not normalization/WF/opaque producer correctness.
use super::constraints::{AnnotationSource, ConstraintCategory as Cat, OutlivesConstraint, ReturnConstraint};
use super::region_infer::NllRegionVariableOrigin as Origin;
use super::type_check::*;
use crate::lang_items::LangItem;
use crate::consts::GenericArg;
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;
use crate::types::variance::V;
use crate::types::{BoundVar, FnHeader, RegionArgsId, RegionId, RegionKind, Safety, TyId, TyKind, UniverseIndex};

#[derive(Debug, PartialEq)]
enum Op {
    Prove(LangItem, Vec<GenericArg>, Locations, Cat),
    Wf(TyId, Locations, Cat),
    Clauses(ClauseOwner, Vec<GenericArg>, Locations),
    Ascribe(TyId, UserType, Span),
    AscribeSkipWf(TyId, TyId, Span),
    Opaque(TyId, V, TyId),
}
struct Fake<'t> {
    tcx: &'t Tcx,
    universe: UniverseIndex,
    /// Region vids 0..first_var are the renumbered body's; later ones are allocated here.
    regions: Vec<(Origin, UniverseIndex)>,
    ops: Vec<Op>,
    fn_sig: Option<PolyFnSig>,
    annotations: Vec<(Span, TyId)>,
    closure_sig: Option<FnSig>,
    /// The M2b answer of `rvalue_query_ty` (e.g. a closure aggregate's type).
    query_ty: Option<TyId>,
    /// `is_inline_const` and `unevaluated_const_ty` answers for the operand-order test.
    inline_const: bool,
    const_ty: Option<TyId>,
}
impl<'t> Fake<'t> {
    fn new(tcx: &'t Tcx, first_var: usize) -> Self {
        Self { tcx, universe: UniverseIndex::ROOT, regions: vec![(Origin::Existential, UniverseIndex::ROOT); first_var],
            ops: vec![], fn_sig: None, annotations: vec![], closure_sig: None, query_ty: None, inline_const: false, const_ty: None }
    }
}
impl BorrowckTypeOps for Fake<'_> {
    fn universe(&self) -> UniverseIndex { self.universe }
    fn create_next_universe(&mut self) -> UniverseIndex { self.universe = UniverseIndex(self.universe.0 + 1); self.universe }
    fn next_nll_region_var(&mut self, origin: Origin, universe: UniverseIndex) -> RegionId {
        self.regions.push((origin, universe));
        self.tcx.tys.intern_region(RegionKind::ReVar(self.regions.len() as u32 - 1))
    }
    fn to_region_vid(&self, r: RegionId) -> usize {
        match self.tcx.tys.region_kind(r) { RegionKind::ReVar(v) => v as usize, RegionKind::ReStatic => 0, k => panic!("not renumbered: {k:?}") }
    }
    fn encountered_re_error(&self) -> bool { false }
    fn shallow_resolve(&mut self, ty: TyId) -> TyId { ty }
    fn shallow_resolve_const(&mut self, ct: crate::consts::Const) -> crate::consts::Const { ct }
    fn next_ty_var(&mut self, _: Span) -> TyId { unreachable!() }
    fn generalize_ty_var(&mut self, _: u32, _: V, _: TyId, _: Span) -> Result<TyId, NoSolution> { unreachable!() }
    fn relate_opaque(&mut self, _: &mut MirTypeckRegionConstraints, a: TyId, v: V, b: TyId, _: Locations, _: Cat) -> Result<(), NoSolution> {
        self.ops.push(Op::Opaque(a, v, b)); Ok(())
    }
    fn prove_trait_ref(&mut self, _: &mut MirTypeckRegionConstraints, t: LangItem, args: &[GenericArg], l: Locations, c: Cat) { self.ops.push(Op::Prove(t, args.to_vec(), l, c)); }
    fn prove_wf(&mut self, _: &mut MirTypeckRegionConstraints, ty: TyId, l: Locations, c: Cat) { self.ops.push(Op::Wf(ty, l, c)); }
    fn normalize_and_prove_instantiated_clauses(&mut self, _: &mut MirTypeckRegionConstraints, o: ClauseOwner, args: &[GenericArg], l: Locations) {
        self.ops.push(Op::Clauses(o, args.to_vec(), l));
    }
    fn normalize(&mut self, _: &mut MirTypeckRegionConstraints, ty: TyId, _: Locations) -> TyId { ty }
    fn deeply_normalize_sig(&mut self, _: &mut MirTypeckRegionConstraints, sig: &FnSig, _: Location) -> Result<FnSig, ()> { Ok(sig.clone()) }
    fn struct_tail(&mut self, _: &mut MirTypeckRegionConstraints, ty: TyId, _: Location) -> TyId { ty }
    fn ascribe_user_type(&mut self, _: &mut MirTypeckRegionConstraints, t: TyId, u: UserType, s: Span) { self.ops.push(Op::Ascribe(t, u, s)); }
    fn ascribe_user_type_skip_wf(&mut self, _: &mut MirTypeckRegionConstraints, t: TyId, u: TyId, s: Span) { self.ops.push(Op::AscribeSkipWf(t, u, s)); }
    fn user_type_annotation(&self, id: UserTypeId) -> (Span, TyId) { self.annotations[id.0 as usize] }
    fn closure_user_provided_sig(&mut self, _: &Body) -> Option<FnSig> { self.closure_sig.clone() }
    fn fn_def_sig(&self, _: DefId, _: &[TyId], _: RegionArgsId) -> PolyFnSig { self.fn_sig.clone().unwrap() }
    fn field_ty(&self, _: PlaceTy, _: FieldIdx) -> Option<TyId> { None }
    fn aggregate_field_ty(&mut self, _: &mut MirTypeckRegionConstraints, _: &AggregateKind, _: FieldIdx, _: Location) -> Result<TyId, usize> { Err(0) }
    fn rvalue_query_ty(&self, _: &Body, _: &Rvalue) -> TyId { self.query_ty.unwrap() }
    fn unevaluated_const_ty(&self, _: DefId, _: &[GenericArg]) -> TyId { self.const_ty.unwrap() }
    fn static_ptr(&self, _: &ConstOperand) -> Option<DefId> { None }
    fn static_ty(&mut self, _: &mut MirTypeckRegionConstraints, _: DefId, _: Locations) -> TyId { unreachable!() }
    fn type_is_sized_modulo_regions(&mut self, ty: TyId) -> bool { !matches!(self.tcx.tys.kind(ty), TyKind::Str | TyKind::Slice(_) | TyKind::Dyn(..)) }
    fn is_privately_uninhabited(&mut self, ty: TyId) -> bool { ty == self.tcx.tys.never }
    fn is_upvar_field_projection(&self, _: &Body, _: &Place) -> Option<FieldIdx> { None }
    fn simd_const_arg(&self, _: DefId) -> Option<(usize, &'static str)> { None }
    fn closure_parent_args(&self, _: ClauseOwner, _: &[GenericArg]) -> (Vec<GenericArg>, Vec<GenericArg>) { (vec![], vec![]) }
    fn unsized_fn_params(&self) -> bool { false }
    fn is_inline_const(&self, _: DefId) -> bool { self.inline_const }
}

fn p(i: u32) -> Place { Place::local(Local(i)) }
fn at(b: u32, s: usize) -> Location { Location { block: BasicBlock(b), statement_index: s } }
fn span(line: u32) -> Span { Span { file: 7, lo: line, hi: line + 1 } }
fn block(stmts: Vec<StatementKind>, term: TerminatorKind) -> BasicBlockData {
    BasicBlockData { statements: stmts.into_iter().enumerate().map(|(i, kind)| Statement { kind, source_info: SourceInfo { span: span(i as u32 + 1), ..SourceInfo::default() } }).collect(),
        terminator: Some(Terminator { kind: term, source_info: SourceInfo { span: span(99), ..SourceInfo::default() } }), is_cleanup: false }
}
/// _0 is the return place, _1 the one argument; the rest are user variables.
fn body(tys: &[TyId], blocks: Vec<BasicBlockData>) -> Body {
    Body { phase: MirPhase::Analysis(AnalysisPhase::Initial), source: MirSource { owner: DefId(0), kind: BodyKind::Fn },
        basic_blocks: blocks, local_decls: tys.iter().enumerate().map(|(i, &ty)| LocalDecl { ty,
            mutability: Mutability::Mut, source_info: SourceInfo { span: span(50 + i as u32), ..SourceInfo::default() }, user_ty: vec![],
            local_info: if i == 0 { LocalInfo::Boring } else { LocalInfo::User { is_ref_binding: false, can_be_made_mutable: false, match_place: None } },
            kind: match i { 0 => LocalKind::Return, 1 => LocalKind::Arg, _ => LocalKind::UserVar } }).collect(),
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }], user_type_annotations: vec![],
        var_debug_info: vec![], arg_count: 1, spread_arg: None, promoted: vec![], coroutine: None }
}
fn re(tcx: &Tcx, v: u32) -> RegionId { tcx.tys.intern_region(RegionKind::ReVar(v)) }
fn bound(tcx: &Tcx, v: u32) -> RegionId { tcx.tys.intern_region(RegionKind::ReBound(0, BoundVar(v))) }
fn rf(tcx: &Tcx, r: RegionId, t: TyId, m: bool) -> TyId { tcx.tys.intern(TyKind::Ref(m, t, r)) }
fn fn_ptr(tcx: &Tcx, inputs: Vec<TyId>, output: TyId, n: u32) -> TyId { tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&inputs), output, FnHeader::RUST, n)) }
fn val(ty: TyId) -> Operand { Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: Const::Val(ConstValue::ZeroSized, ty) })) }
fn assign(dst: u32, rv: Rvalue) -> StatementKind { StatementKind::Assign(p(dst), rv) }
fn copy(src: u32) -> Rvalue { Rvalue::Use(Operand::Copy(p(src))) }
fn check<'t>(tcx: &'t Tcx, fake: &mut Fake<'t>, b: &Body) -> MirTypeckResults {
    check_in(tcx, &test_prog(), fake, b)
}
/// Bodies over collected declarations relate them through the declaration queries
/// (variances_of), which read the program that declared them.
fn check_in<'t>(tcx: &'t Tcx, prog: &Program, fake: &mut Fake<'t>, b: &Body) -> MirTypeckResults {
    let io: Vec<TyId> = (1..=b.arg_count as usize).chain([0]).map(|l| b.local_decls[l].ty).collect();
    type_check(tcx, prog, fake, b, &io, None)
}
/// (sup, sub, point, category) of the constraints, without the I/O equalities
/// (Locations::All with BoringNoLocation).
fn edges(r: &MirTypeckResults) -> Vec<(usize, usize, Option<usize>, Cat)> {
    r.constraints.outlives_constraints.outlives.iter().filter(|c| c.category != Cat::BoringNoLocation)
        .map(|c| (c.sup, c.sub, c.location, c.category)).collect()
}
fn clean(r: &MirTypeckResults) { assert!(r.mir_bugs.is_empty(), "{:?}", r.mir_bugs); assert!(r.errors.is_empty(), "{:?}", r.errors); }

#[test]
fn assignment_of_shorter_reference_emits_outlives_at_location() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let (r1, r2) = (rf(&tcx, re(&tcx, 1), t, false), rf(&tcx, re(&tcx, 2), t, false));
    let b = body(&[tcx.tys.unit, r1, r2], vec![block(vec![StatementKind::Nop, assign(2, copy(1))], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    let r = check(&tcx, &mut fake, &b); clean(&r);
    assert_eq!(edges(&r), [(1, 2, Some(1), Cat::Assignment)]);
    let c: &OutlivesConstraint = r.constraints.outlives_constraints.outlives.iter().find(|c| c.category == Cat::Assignment).unwrap();
    assert_eq!(c.span, span(2)); assert!(!c.from_closure);
    // `Copy` operands prove `T: Copy`; assignments prove the place type Sized.
    assert!(fake.ops.contains(&Op::Prove(LangItem::Copy, vec![GenericArg::Type(r1)], Locations::Single(at(0, 1)), Cat::CopyBound)));
    assert!(fake.ops.contains(&Op::Prove(LangItem::Sized, vec![GenericArg::Type(r2)], Locations::Single(at(0, 1)), Cat::SizedBound)));
    // The signature is equated everywhere.
    let io: Vec<_> = r.constraints.outlives_constraints.outlives.iter().filter(|c| c.category == Cat::BoringNoLocation).collect();
    assert!(io.iter().all(|c| c.location.is_none() && c.sup == c.sub || c.location.is_none()));
}

#[test]
fn assignment_categories_follow_destination_kind() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let r1 = rf(&tcx, re(&tcx, 1), t, false);
    let mut b = body(&[rf(&tcx, re(&tcx, 0), t, false), r1, rf(&tcx, re(&tcx, 2), t, false)],
        vec![block(vec![assign(0, copy(1)), assign(2, copy(1))], TerminatorKind::Return)]);
    let cats = |b: &Body| { let mut f = Fake::new(&tcx, 3); edges(&check(&tcx, &mut f, b)).into_iter().map(|e| e.3).collect::<Vec<_>>() };
    assert_eq!(cats(&b), [Cat::Return(ReturnConstraint::Normal), Cat::Assignment]);
    b.local_decls[2].kind = LocalKind::Temp; b.local_decls[2].local_info = LocalInfo::Boring;
    assert_eq!(cats(&b)[1], Cat::Boring);
    b.local_decls[2].local_info = LocalInfo::AggregateTemp;
    assert_eq!(cats(&b)[1], Cat::Usage);
    b.source.kind = BodyKind::Static;
    assert_eq!(cats(&b)[0], Cat::UseAsStatic);
    b.source.kind = BodyKind::Const;
    assert_eq!(cats(&b)[0], Cat::UseAsConst);
}

#[test]
fn mutable_references_are_invariant_in_their_referent() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let inner = |v| rf(&tcx, re(&tcx, v), t, false);
    let b = body(&[tcx.tys.unit, rf(&tcx, re(&tcx, 1), inner(3), true), rf(&tcx, re(&tcx, 2), inner(4), true)],
        vec![block(vec![assign(2, Rvalue::Use(Operand::Move(p(1))))], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 5);
    let r = check(&tcx, &mut fake, &b); clean(&r);
    let mut e: Vec<_> = edges(&r).into_iter().map(|e| (e.0, e.1)).collect(); e.sort();
    assert_eq!(e, [(1, 2), (3, 4), (4, 3)]);
    // A shared referent is covariant.
    let b = body(&[tcx.tys.unit, rf(&tcx, re(&tcx, 1), inner(3), false), rf(&tcx, re(&tcx, 2), inner(4), false)],
        vec![block(vec![assign(2, copy(1))], TerminatorKind::Return)]);
    let mut e: Vec<_> = edges(&check(&tcx, &mut Fake::new(&tcx, 5), &b)).into_iter().map(|e| (e.0, e.1)).collect(); e.sort();
    assert_eq!(e, [(1, 2), (3, 4)]);
}

#[test]
fn call_arguments_destination_and_late_bound_regions() {
    let (prog, tcx) = collect("fn f<'a>(x: &'a i32) -> &'a i32 { x }"); let t = tcx.tys.i32_;
    // fn f<'x>(&'x i32) -> &'x i32, called as `_3 = f(copy _2)`.
    let f = tcx.tys.intern(TyKind::FnDef(named(&prog, "f"), tcx.tys.mk_list(&[]), RegionArgsId::EMPTY));
    let late = rf(&tcx, bound(&tcx, 0), t, false);
    let call = |source| TerminatorKind::Call { func: val(f), args: vec![Operand::Copy(p(2))], destination: p(3), target: Some(BasicBlock(1)),
        unwind: UnwindAction::Continue, call_source: source, fn_span: span(80), arg_spans: vec![] };
    let mut b = body(&[tcx.tys.unit, t, rf(&tcx, re(&tcx, 1), t, false), rf(&tcx, re(&tcx, 2), t, false)],
        vec![block(vec![], call(CallSource::Normal)), block(vec![], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    fake.fn_sig = Some(PolyFnSig { sig: FnSig { inputs: vec![late], output: late, header: FnHeader::RUST }, bound_regions: 1 });
    let r = type_check(&tcx, &prog, &mut fake, &b, &[t, tcx.tys.unit], None); clean(&r);
    // The late-bound region becomes vid 3, live at the call (point 0).
    assert_eq!(fake.regions[3], (Origin::Existential, UniverseIndex::ROOT));
    assert!(r.constraints.liveness_constraints.point_liveness(3).is_some_and(|l| l.contains(0)));
    let instantiated = rf(&tcx, re(&tcx, 3), t, false);
    assert_eq!(edges(&r), [(3, 2, Some(0), Cat::Assignment), (1, 3, Some(0), Cat::CallArgument(Some(f)))]);
    // Raw signature WF before normalization, at the call.
    assert_eq!(fake.ops.iter().filter(|o| **o == Op::Wf(instantiated, Locations::Single(at(0, 0)), Cat::Boring)).count(), 2);
    // Calls not written by the user blame no argument.
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = call(CallSource::OverloadedOperator);
    let mut fake = Fake::new(&tcx, 3); fake.fn_sig = Some(PolyFnSig { sig: FnSig { inputs: vec![late], output: late, header: FnHeader::RUST }, bound_regions: 1 });
    assert_eq!(edges(&type_check(&tcx, &prog, &mut fake, &b, &[t, tcx.tys.unit], None))[1].3, Cat::Boring);
    // Wrong arity is ill-typed MIR.
    b.basic_blocks[0].terminator.as_mut().unwrap().kind = TerminatorKind::Call { func: val(f), args: vec![], destination: p(3), target: Some(BasicBlock(1)),
        unwind: UnwindAction::Continue, call_source: CallSource::Normal, fn_span: span(80), arg_spans: vec![] };
    let mut fake = Fake::new(&tcx, 3); fake.fn_sig = Some(PolyFnSig { sig: FnSig { inputs: vec![late], output: late, header: FnHeader::RUST }, bound_regions: 1 });
    let r = type_check(&tcx, &prog, &mut fake, &b, &[t, tcx.tys.unit], None);
    assert!(r.tainted_by_errors && r.mir_bugs[0].message.contains("wrong # of args"));
}

#[test]
fn return_and_yield_categories() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let (r0, r1) = (rf(&tcx, re(&tcx, 0), t, false), rf(&tcx, re(&tcx, 1), t, false));
    let mut b = body(&[r0, r1, rf(&tcx, re(&tcx, 2), t, false)], vec![block(vec![assign(0, copy(1))],
        TerminatorKind::Yield { value: Operand::Copy(p(1)), resume: BasicBlock(1), resume_arg: p(2), drop: None }), block(vec![], TerminatorKind::Return)]);
    b.coroutine = Some(CoroutineInfo { yield_ty: rf(&tcx, re(&tcx, 3), t, false), resume_ty: rf(&tcx, re(&tcx, 4), t, false), kind: CoroutineKind::Coroutine });
    let mut fake = Fake::new(&tcx, 7);
    let io = [r1, r0];
    let r = type_check(&tcx, &test_prog(), &mut fake, &b, &io, Some((rf(&tcx, re(&tcx, 5), t, false), rf(&tcx, re(&tcx, 6), t, false)))); clean(&r);
    assert_eq!(edges(&r), [(1, 0, Some(0), Cat::Return(ReturnConstraint::Normal)), (1, 3, Some(1), Cat::Yield), (4, 2, Some(1), Cat::Yield)]);
    // Universal yield/resume types are equated with the body's everywhere.
    let all: Vec<_> = r.constraints.outlives_constraints.outlives.iter().filter(|c| c.category == Cat::BoringNoLocation).map(|c| (c.sup, c.sub, c.location)).collect();
    for e in [(5, 3, None), (3, 5, None), (6, 4, None), (4, 6, None)] { assert!(all.contains(&e), "{all:?}"); }
}

#[test]
fn cast_categories() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let ptr = |m, v| tcx.tys.intern(TyKind::Ptr(m, rf(&tcx, re(&tcx, v), t, false)));
    let cast = |kind, ty| assign(2, Rvalue::Cast(kind, Operand::Copy(p(1)), ty));
    let implicit = |i| Cat::Cast { is_raw_ptr_dyn_type_cast: false, is_implicit_coercion: i, unsize_to: None };
    // *mut &'1 i32 as *const &'2 i32.
    let b = body(&[tcx.tys.unit, ptr(true, 1), ptr(false, 2)], vec![block(vec![cast(CastKind::PointerCoercion(PointerCoercion::MutToConstPointer, CoercionSource::Implicit), ptr(false, 2))], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 3), &b); clean(&r);
    assert_eq!(edges(&r)[0], (1, 2, Some(0), implicit(true)));
    // fn(&'1 i32) as unsafe fn(&'2 i32): the source becomes unsafe, arguments are contravariant.
    let f = |v, safety| tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[rf(&tcx, re(&tcx, v), t, false)]), tcx.tys.unit, FnHeader { safety, ..FnHeader::RUST }, 0));
    let b = body(&[tcx.tys.unit, f(1, Safety::Safe), f(2, Safety::Unsafe)], vec![block(vec![cast(CastKind::PointerCoercion(PointerCoercion::UnsafeFnPointer, CoercionSource::AsCast), f(2, Safety::Unsafe))], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 3), &b); clean(&r);
    assert_eq!(edges(&r)[0], (2, 1, Some(0), implicit(false)));
    // Unsizing proves CoerceUnsized; the target is recorded region-erased.
    let arr = rf(&tcx, re(&tcx, 1), tcx.tys.intern(TyKind::Array(t, tcx.tys.const_usize(2))), false);
    let slice = rf(&tcx, re(&tcx, 2), tcx.tys.intern(TyKind::Slice(t)), false);
    let b = body(&[tcx.tys.unit, arr, slice], vec![block(vec![cast(CastKind::PointerCoercion(PointerCoercion::Unsize, CoercionSource::Implicit), slice)], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    check(&tcx, &mut fake, &b);
    let erased = rf(&tcx, RegionId::ERASED, tcx.tys.intern(TyKind::Slice(t)), false);
    assert!(fake.ops.contains(&Op::Prove(LangItem::CoerceUnsized, vec![GenericArg::Type(arr), GenericArg::Type(slice)], Locations::Single(at(0, 0)),
        Cat::Cast { is_raw_ptr_dyn_type_cast: false, is_implicit_coercion: true, unsize_to: Some(erased) })));
    // Numeric casts are only checked for shape.
    let b = body(&[tcx.tys.unit, tcx.tys.f64_, t], vec![block(vec![cast(CastKind::IntToInt, t)], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 1), &b);
    assert!(r.mir_bugs[0].message.contains("Invalid IntToInt"));
}

#[test]
fn reify_and_closure_fn_pointer_casts() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    // A higher-ranked fn item reified to `fn(&'2 i32)`: the instantiated signature is proven WF.
    let item = tcx.tys.intern(TyKind::FnDef(DefId(9), tcx.tys.mk_list(&[]), RegionArgsId::EMPTY));
    let target = fn_ptr(&tcx, vec![rf(&tcx, re(&tcx, 2), t, false)], tcx.tys.unit, 0);
    let b = body(&[tcx.tys.unit, item, target], vec![block(vec![assign(2, Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer, CoercionSource::Implicit), Operand::Copy(p(1)), target))], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    fake.fn_sig = Some(PolyFnSig { sig: FnSig { inputs: vec![rf(&tcx, bound(&tcx, 0), t, false)], output: tcx.tys.unit, header: FnHeader::RUST }, bound_regions: 1 });
    let r = check(&tcx, &mut fake, &b); clean(&r);
    let cat = Cat::Cast { is_raw_ptr_dyn_type_cast: false, is_implicit_coercion: true, unsize_to: None };
    let fresh = fn_ptr(&tcx, vec![rf(&tcx, re(&tcx, 3), t, false)], tcx.tys.unit, 0);
    assert!(fake.ops.contains(&Op::Wf(fresh, Locations::Single(at(0, 0)), cat)));
    // Fresh '3 relates to '2 by the WF'd subtyping; the binder relation adds an existential '4.
    let e = edges(&r);
    assert!(e.contains(&(2, 3, Some(0), cat)) && e.contains(&(2, 4, Some(0), cat)), "{e:?}");
    // A closure's signature becomes a fn pointer of the target's safety.
    let sig = fn_ptr(&tcx, vec![rf(&tcx, re(&tcx, 1), t, false)], tcx.tys.unit, 0);
    let closure = tcx.tys.intern(TyKind::Closure(7, 1, crate::types::ClosureArgs { parent_args: item, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: tcx.tys.unit }));
    let b = body(&[tcx.tys.unit, closure, target], vec![block(vec![assign(2, Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::ClosureFnPointer, CoercionSource::AsCast), Operand::Copy(p(1)), target))], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 3), &b); clean(&r);
    assert_eq!(edges(&r)[0], (2, 1, Some(0), Cat::Cast { is_raw_ptr_dyn_type_cast: false, is_implicit_coercion: false, unsize_to: None }));
}

pub(crate) fn collect(source: &str) -> (Program, Tcx) {
    let session = std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = std::env::temp_dir().join(format!("relax-type-check-{}-{n}.rs", std::process::id()));
    std::fs::write(&file, source).unwrap();
    let mut prog = Program::new(&session);
    prog.load_crate("custom", file.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    std::fs::remove_file(file).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (prog, tcx)
}
fn named(prog: &Program, name: &str) -> DefId {
    (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| prog.name(d) == name).unwrap()
}
/// An RPIT whose opaque has the declared variances [Bi (parent 'a), Inv (captured 'a)]
/// (rustc_hir_analysis/src/variance/mod.rs:98-239: an opaque's parent lifetimes are
/// bivariant, its captured duplicates invariant).
pub(crate) fn opaque_fixture() -> (Program, Tcx, DefId) {
    let (prog, tcx) = collect("trait Bound<'a> {} fn f<'a: 'a>(x: &'a u32) -> impl Bound<'a> { loop {} }");
    let opaque = prog.opaque_types_defined_by[&named(&prog, "f")][0];
    assert_eq!(tcx.variances_of(&prog, opaque), [crate::types::variance::V::Bi, crate::types::variance::V::Inv]);
    (prog, tcx, opaque)
}

#[test]
fn adt_lifetime_arguments_follow_declared_variance() {
    let (prog, tcx) = collect("struct Co<'a>(&'a i32); struct Contra<'a>(fn(&'a i32)); struct Inv<'a>(fn(&'a i32) -> &'a i32); struct Bi<'a, T>(T);");
    let adt = |name, v| tcx.tys.intern(TyKind::Adt(named(&prog, name), tcx.tys.mk_list(&[]), tcx.tys.intern_region_args(&[re(&tcx, v)])));
    for (name, expected) in [("Co", vec![(1, 2)]), ("Contra", vec![(2, 1)]), ("Inv", vec![(1, 2), (2, 1)])] {
        let b = body(&[tcx.tys.unit, adt(name, 1), adt(name, 2)], vec![block(vec![assign(2, Rvalue::Use(Operand::Move(p(1))))], TerminatorKind::Return)]);
        let r = check_in(&tcx, &prog, &mut Fake::new(&tcx, 3), &b); clean(&r);
        let mut e: Vec<_> = edges(&r).into_iter().map(|e| (e.0, e.1)).collect(); e.sort();
        assert_eq!(e, expected, "{name}");
    }
    // An unused (bivariant) lifetime parameter relates nothing.
    let bi = |v| tcx.tys.intern(TyKind::Adt(named(&prog, "Bi"), tcx.tys.mk_list(&[tcx.tys.i32_]), tcx.tys.intern_region_args(&[re(&tcx, v)])));
    let b = body(&[tcx.tys.unit, bi(1), bi(2)], vec![block(vec![assign(2, Rvalue::Use(Operand::Move(p(1))))], TerminatorKind::Return)]);
    let r = check_in(&tcx, &prog, &mut Fake::new(&tcx, 3), &b); clean(&r);
    assert!(edges(&r).is_empty());
}

#[test]
fn fn_pointer_arguments_are_contravariant() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let f = |a, r| fn_ptr(&tcx, vec![rf(&tcx, re(&tcx, a), t, false)], rf(&tcx, re(&tcx, r), t, false), 0);
    let b = body(&[tcx.tys.unit, f(1, 3), f(2, 4)], vec![block(vec![assign(2, copy(1))], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 5), &b); clean(&r);
    let mut e: Vec<_> = edges(&r).into_iter().map(|e| (e.0, e.1)).collect(); e.sort();
    assert_eq!(e, [(2, 1), (3, 4)]);
    // Headers must agree.
    let unsafe_f = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[rf(&tcx, re(&tcx, 2), t, false)]), rf(&tcx, re(&tcx, 4), t, false), FnHeader { safety: Safety::Unsafe, ..FnHeader::RUST }, 0));
    let b = body(&[tcx.tys.unit, f(1, 3), unsafe_f], vec![block(vec![assign(2, copy(1))], TerminatorKind::Return)]);
    assert!(check(&tcx, &mut Fake::new(&tcx, 5), &b).mir_bugs[0].message.contains("bad assignment"));
}

#[test]
fn higher_ranked_fn_pointers_use_placeholders_and_existentials() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let hr = fn_ptr(&tcx, vec![rf(&tcx, bound(&tcx, 0), t, false)], tcx.tys.unit, 1);
    let mono = |v| fn_ptr(&tcx, vec![rf(&tcx, re(&tcx, v), t, false)], tcx.tys.unit, 0);
    // for<'x> fn(&'x i32) <: fn(&'2 i32): the subtype's binder is existential ('3), no universe.
    let b = body(&[tcx.tys.unit, hr, mono(2)], vec![block(vec![assign(2, copy(1))], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    let r = check(&tcx, &mut fake, &b); clean(&r);
    assert_eq!(fake.regions[3], (Origin::Existential, UniverseIndex::ROOT));
    assert_eq!(edges(&r), [(2, 3, Some(0), Cat::Assignment)]);
    assert!(r.constraints.universe_causes.is_empty());
    // fn(&'1 i32) <: for<'x> fn(&'x i32): the supertype's binder becomes a placeholder
    // in a new universe, recorded with the related types; '1 must outlive it.
    let b = body(&[tcx.tys.unit, mono(1), hr], vec![block(vec![assign(2, copy(1))], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    let r = check(&tcx, &mut fake, &b); clean(&r);
    let placeholder = tcx.tys.intern_region(RegionKind::RePlaceholder { universe: UniverseIndex(1), var: BoundVar(0) });
    assert_eq!(fake.regions[3], (Origin::Placeholder(placeholder), UniverseIndex(1)));
    assert_eq!(r.constraints.placeholder_indices.lookup_index(placeholder), 0);
    assert_eq!(r.constraints.universe_causes, [(UniverseIndex(1), UniverseInfo::RelateTys { expected: hr, found: mono(1) })]);
    assert_eq!(edges(&r), [(3, 1, Some(0), Cat::Assignment)]);
    // Invariant: both directions, a placeholder and an existential per side.
    let hr2 = |v| fn_ptr(&tcx, vec![rf(&tcx, bound(&tcx, 0), t, false), rf(&tcx, re(&tcx, v), t, false)], tcx.tys.unit, 1);
    let b = body(&[tcx.tys.unit, rf(&tcx, re(&tcx, 1), hr2(1), true), rf(&tcx, re(&tcx, 2), hr2(2), true)],
        vec![block(vec![assign(2, Rvalue::Use(Operand::Move(p(1))))], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    let r = check(&tcx, &mut fake, &b); clean(&r);
    let origins: Vec<_> = fake.regions[3..].iter().map(|(o, u)| (matches!(o, Origin::Placeholder(_)), u.0)).collect();
    assert_eq!(origins, [(true, 1), (false, 1), (true, 2), (false, 2)]);
    assert_eq!(r.constraints.universe_causes.len(), 2);
    // The outer `'1: '2`; per pass the bound arguments relate placeholder and existential
    // both ways and the free ones are equated.
    let mut e: Vec<_> = edges(&r).into_iter().map(|e| (e.0, e.1)).collect(); e.sort();
    assert_eq!(e, [(1, 2), (1, 2), (1, 2), (2, 1), (2, 1), (3, 4), (4, 3), (5, 6), (6, 5)]);
}

#[test]
fn reborrow_constraints_follow_derefs_of_references() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    // _2 = &'2 (*_1) with _1: &'1 mut i32: '1: '2; through `&'3 &'1 mut i32` also '3: '2.
    let inner = rf(&tcx, re(&tcx, 1), t, true);
    let deref = |l| Place { local: Local(l), projection: vec![PlaceElem::Deref] };
    let b = body(&[tcx.tys.unit, inner, rf(&tcx, re(&tcx, 2), t, false)], vec![block(vec![assign(2, Rvalue::Ref(Region(2), BorrowKind::Shared, deref(1)))], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 3), &b); clean(&r);
    assert!(edges(&r).contains(&(1, 2, Some(0), Cat::Boring)));
    let outer = rf(&tcx, re(&tcx, 3), inner, false);
    let place = Place { local: Local(1), projection: vec![PlaceElem::Deref, PlaceElem::Deref] };
    let b = body(&[tcx.tys.unit, outer, rf(&tcx, re(&tcx, 2), t, false)], vec![block(vec![assign(2, Rvalue::Ref(Region(2), BorrowKind::Shared, place))], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 4), &b); clean(&r);
    let e: Vec<_> = edges(&r).into_iter().filter(|e| e.3 == Cat::Boring).map(|e| (e.0, e.1)).collect();
    assert_eq!(e, [(1, 2), (3, 2)]);
    // Through a shared outer reference the walk stops at the first shared deref.
    let shared_inner = rf(&tcx, re(&tcx, 1), t, false);
    let b = body(&[tcx.tys.unit, rf(&tcx, re(&tcx, 3), shared_inner, false), rf(&tcx, re(&tcx, 2), t, false)],
        vec![block(vec![assign(2, Rvalue::Ref(Region(2), BorrowKind::Shared, Place { local: Local(1), projection: vec![PlaceElem::Deref, PlaceElem::Deref] }))], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 4), &b);
    let e: Vec<_> = edges(&r).into_iter().filter(|e| e.3 == Cat::Boring).map(|e| (e.0, e.1)).collect();
    assert_eq!(e, [(1, 2)]);
}

#[test]
fn user_annotations_hold_everywhere() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let annotated = rf(&tcx, re(&tcx, 0), t, false);
    let mut b = body(&[tcx.tys.unit, t, rf(&tcx, re(&tcx, 2), t, false)], vec![block(vec![
        StatementKind::AscribeUserType(p(2), UserTypeProjection { base: UserTypeAnnotationIndex(0), projection: vec![] }, Variance::Covariant),
    ], TerminatorKind::Return)]);
    b.user_type_annotations = vec![UserTypeId(0)];
    b.local_decls[2].user_ty = vec![UserTypeProjection { base: UserTypeAnnotationIndex(0), projection: vec![] }];
    let mut fake = Fake::new(&tcx, 3);
    fake.annotations = vec![(span(40), annotated)];
    let r = check(&tcx, &mut fake, &b); clean(&r);
    // The canonical annotation is ascribed first, then the declaration (invariant) and the statement (covariant).
    assert_eq!(fake.ops[0], Op::Ascribe(annotated, UserType::Annotation(UserTypeId(0)), span(40)));
    let all: Vec<_> = r.constraints.outlives_constraints.outlives.iter().filter(|c| c.category != Cat::BoringNoLocation)
        .map(|c| (c.sup, c.sub, c.location, c.category, c.span)).collect();
    let decl = Cat::TypeAnnotation(AnnotationSource::Declaration);
    let asc = Cat::TypeAnnotation(AnnotationSource::Ascription);
    assert_eq!(all, [(0, 2, None, decl, span(40)), (2, 0, None, decl, span(40)), (2, 0, None, asc, span(1))]);
}

#[test]
fn promoted_constraints_move_to_the_use_location() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    // Promoted: _0 = &'5 _1 ... returned (Return, demoted to Boring) at the parent's use.
    let promoted_ty = rf(&tcx, re(&tcx, 5), t, false);
    let mut promoted = body(&[promoted_ty, t, rf(&tcx, re(&tcx, 6), t, false)], vec![block(vec![StatementKind::Nop, assign(0, copy(2))], TerminatorKind::Return)]);
    promoted.arg_count = 0; promoted.source.kind = BodyKind::Promoted(DefId(0), PromotedId(0));
    let use_promoted = Operand::Constant(Box::new(ConstOperand { span: span(30), user_ty: None,
        const_: Const::Unevaluated { def: DefId(0), args: vec![], promoted: Some(PromotedId(0)) } }));
    let mut b = body(&[tcx.tys.unit, t, rf(&tcx, re(&tcx, 2), t, false)], vec![block(vec![StatementKind::Nop, StatementKind::Nop, assign(2, Rvalue::Use(use_promoted))], TerminatorKind::Return)]);
    b.promoted = vec![promoted];
    let mut fake = Fake::new(&tcx, 7);
    let r = check(&tcx, &mut fake, &b); clean(&r);
    let all: Vec<_> = r.constraints.outlives_constraints.outlives.iter().map(|c| (c.sup, c.sub, c.location, c.category, c.span)).collect();
    // The promoted's return keeps its own span but moves to point 2 as Boring; the promoted
    // type is equated with the constant's type there; then the assignment.
    assert!(all.contains(&(6, 5, Some(2), Cat::Boring, span(2))), "{all:?}");
    assert!(all.contains(&(5, 2, Some(2), Cat::Assignment, span(3))), "{all:?}");
    // Regions free in the constant's type are live at the use.
    assert!(r.constraints.liveness_constraints.point_liveness(5).is_some_and(|l| l.contains(2)));
}

#[test]
fn closure_aggregates_defer_requirements_and_prove_bounds() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let id = ClosureId { file: 7, expr: 3 };
    let closure = tcx.tys.intern(TyKind::Closure(7, 3, crate::types::ClosureArgs { parent_args: tcx.tys.unit, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: fn_ptr(&tcx, vec![], tcx.tys.unit, 0), tupled_upvars_ty: tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[]))) }));
    let b = body(&[tcx.tys.unit, t, closure], vec![block(vec![StatementKind::Nop, assign(2, Rvalue::Aggregate(Box::new(AggregateKind::Closure(id, vec![GenericArg::Type(t)])), vec![]))], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    fake.query_ty = Some(closure);
    let r = check(&tcx, &mut fake, &b); clean(&r);
    assert_eq!(r.deferred_closure_requirements, [DeferredClosureRequirement { def: ClauseOwner::Closure(id), args: vec![GenericArg::Type(t)], locations: Locations::Single(at(0, 1)) }]);
    assert!(fake.ops.contains(&Op::Clauses(ClauseOwner::Closure(id), vec![GenericArg::Type(t)], Locations::Single(at(0, 1)))));
}

#[test]
fn closure_signature_annotations_skip_wf() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let mut b = body(&[t, tcx.tys.unit, t, t], vec![block(vec![], TerminatorKind::Return)]);
    b.source.kind = BodyKind::Closure(3); b.arg_count = 3;
    let mut fake = Fake::new(&tcx, 1);
    fake.closure_sig = Some(FnSig { inputs: vec![t, t], output: t, header: FnHeader::RUST });
    let io = [tcx.tys.unit, t, t, t];
    let r = type_check(&tcx, &test_prog(), &mut fake, &b, &io, None); clean(&r);
    let skip: Vec<_> = fake.ops.iter().filter_map(|o| if let Op::AscribeSkipWf(_, _, s) = o { Some(*s) } else { None }).collect();
    // The implicit `self` (_1) is skipped; arguments _2, _3 then the return place.
    assert_eq!(skip, [span(52), span(53), span(50)]);
}

#[test]
fn cleanup_edges_and_unsized_locals_are_checked() {
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let mut b = body(&[tcx.tys.unit, t, tcx.tys.str_], vec![block(vec![], TerminatorKind::Goto { target: BasicBlock(1) }), block(vec![], TerminatorKind::Return)]);
    b.basic_blocks[1].is_cleanup = true;
    let r = check(&tcx, &mut Fake::new(&tcx, 1), &b);
    assert!(r.mir_bugs.iter().any(|m| m.message.contains("cleanuppad mismatch")), "{:?}", r.mir_bugs);
    assert!(r.mir_bugs.iter().any(|m| m.message.contains("return on cleanup block")));
    // E0161 once for the unsized user variable.
    assert_eq!(r.errors.iter().map(|e| e.code()).collect::<Vec<_>>(), ["E0161"]);
    assert_eq!(r.errors[0].span, span(52));
    assert!(r.tainted_by_errors);
}

// --- re-audit restorations (parity/reaudit.REPORT.md) ---
#[test]
fn reaudit_inline_const_bounds_follow_the_constant_checks() {
    // mod.rs:1707-1732: super_operand runs visit_const_operand (here the TypeOf
    // ascription) before the inline const's closure bounds are proven.
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let konst = Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None,
        const_: Const::Unevaluated { def: DefId(5), args: vec![], promoted: None } }));
    let b = body(&[tcx.tys.unit, t, t], vec![block(vec![assign(2, Rvalue::Use(konst))], TerminatorKind::Return)]);
    let mut fake = Fake::new(&tcx, 3);
    fake.inline_const = true; fake.const_ty = Some(t);
    let r = check(&tcx, &mut fake, &b); clean(&r);
    let ascribe = fake.ops.iter().position(|o| matches!(o, Op::Ascribe(_, UserType::TypeOf(DefId(5), _), _))).unwrap();
    let clauses = fake.ops.iter().position(|o| matches!(o, Op::Clauses(ClauseOwner::Def(DefId(5)), _, _))).unwrap();
    assert!(ascribe < clauses, "{:?}", fake.ops);
    assert_eq!(r.deferred_closure_requirements.len(), 1);
}
#[test]
fn reaudit_generic_arguments_are_related_in_declaration_order() {
    // relate_args_invariantly / combine_ty_args walk one GenericArgs list: the lifetime
    // `'a` (declared first) is related before the type argument `T`.
    let (prog, tcx) = collect("struct P<'a, T>(&'a T);");
    let t = tcx.tys.i32_;
    let adt = |a, r| tcx.tys.intern(TyKind::Adt(named(&prog, "P"), tcx.tys.mk_list(&[rf(&tcx, re(&tcx, r), t, false)]), tcx.tys.intern_region_args(&[re(&tcx, a)])));
    let b = body(&[tcx.tys.unit, adt(1, 3), adt(2, 4)], vec![block(vec![assign(2, Rvalue::Use(Operand::Move(p(1))))], TerminatorKind::Return)]);
    let r = check_in(&tcx, &prog, &mut Fake::new(&tcx, 5), &b); clean(&r);
    let e: Vec<_> = edges(&r).into_iter().map(|e| (e.0, e.1)).collect();
    assert_eq!(e, [(1, 2), (3, 4)]);
    // Invariantly (`&mut P<..>`): the same order, both directions per argument.
    let m = |a, r| rf(&tcx, re(&tcx, 9), adt(a, r), true);
    let b = body(&[tcx.tys.unit, m(1, 3), m(2, 4)], vec![block(vec![assign(2, Rvalue::Use(Operand::Move(p(1))))], TerminatorKind::Return)]);
    let r = check_in(&tcx, &prog, &mut Fake::new(&tcx, 10), &b); clean(&r);
    let e: Vec<_> = edges(&r).into_iter().map(|e| (e.0, e.1)).filter(|e| e.0 != 9).collect();
    assert_eq!(e, [(2, 1), (1, 2), (4, 3), (3, 4)]);
}
#[test]
fn reaudit_closure_parent_arguments_are_related_first() {
    // ClosureArgs are [parent args.., kind, signature, upvars]: the owner's regions first.
    let tcx = Tcx::new(); let t = tcx.tys.i32_;
    let closure = |o, s, c| {
        let owner = tcx.tys.intern(TyKind::FnDef(DefId(3), tcx.tys.mk_list(&[]), tcx.tys.intern_region_args(&[re(&tcx, o)])));
        let sig = fn_ptr(&tcx, vec![rf(&tcx, re(&tcx, s), t, false)], tcx.tys.unit, 0);
        let captures = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[rf(&tcx, re(&tcx, c), t, false)])));
        tcx.tys.intern(TyKind::Closure(7, 1, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: tcx.tys.prim(crate::program::Prim::I8), closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: captures }))
    };
    let b = body(&[tcx.tys.unit, closure(1, 3, 5), closure(2, 4, 6)], vec![block(vec![assign(2, Rvalue::Use(Operand::Move(p(1))))], TerminatorKind::Return)]);
    let r = check(&tcx, &mut Fake::new(&tcx, 7), &b); clean(&r);
    let firsts: Vec<_> = edges(&r).into_iter().map(|e| e.0.min(e.1)).collect();
    assert_eq!(firsts, [1, 1, 3, 3, 5, 5]);
}

fn test_prog() -> crate::program::Program { crate::program::Program::new(&std::sync::Arc::new(crate::Session::default())) }
