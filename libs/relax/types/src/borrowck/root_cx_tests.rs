//! The root schedule on hand-built bodies: nested requirements (region and type), the two-phase
//! application for bodies depending on opaque types, root-wide hidden types and shared taint.
use super::constraints::{ConstraintCategory as Cat, OutlivesConstraint, ReturnConstraint};
use super::mir_nll::{LocationMap, MirTypeck, MirTypeckSummary};
use super::polonius::PoloniusContext;
use super::region_infer::NllRegionVariableOrigin as Origin;
use super::root_cx::{BorrowCheckRootCtxt, RootBody};
use super::type_check::{ClauseOwner, DeferredClosureRequirement, Locations, MirTypeckRegionConstraints};
use super::type_check_tests::Fake;
use super::universal_regions::{RegionClassification as Class, UniversalRegions};
use crate::consts::GenericArg;
use crate::infer::opaque_types::{OpaqueTypeKey, ProvisionalHiddenType};
use crate::mir::*;
use crate::mir::move_paths::MoveData;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;
use crate::types::{RegionId, RegionKind, TyKind};

fn at(b: u32, s: usize) -> Location { Location { block: BasicBlock(b), statement_index: s } }
fn span(line: u32) -> Span { Span { file: 7, lo: line, hi: line + 1 } }
fn test_prog() -> Program { Program::new(&std::sync::Arc::new(crate::Session::default())) }
fn empty_body(tcx: &Tcx) -> Body {
    let block = BasicBlockData { statements: vec![], terminator: Some(Terminator { kind: TerminatorKind::Return, source_info: SourceInfo { span: span(99), ..SourceInfo::default() } }), is_cleanup: false };
    Body { phase: MirPhase::Analysis(AnalysisPhase::Initial), source: MirSource { owner: DefId(0), kind: BodyKind::Fn }, basic_blocks: vec![block],
        local_decls: vec![LocalDecl { ty: tcx.tys.unit, mutability: Mutability::Mut, source_info: SourceInfo::default(), user_ty: vec![], local_info: LocalInfo::Boring, kind: LocalKind::Return }],
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }], user_type_annotations: vec![], var_debug_info: vec![], span: Span::default(), upvars: Vec::new(), allocs: AllocMap::default(),
        arg_count: 0, spread_arg: None, promoted: vec![], coroutine: None }
}
fn region(tcx: &Tcx, i: u32) -> RegionId { tcx.tys.intern_region(RegionKind::ReEarlyParam(i)) }
/// A type-checked body whose universal regions are `names` (global first) with their classes; its
/// variables 0.. are exactly those.
fn typeck(tcx: &Tcx, b: &Body, names: &[(RegionId, Class)], outlives: Vec<OutlivesConstraint>) -> (MirTypeck, Fake<'static>) {
    let mut universals = UniversalRegions::new(0, None, names.iter().enumerate().map(|(i, (_, c))| (i, *c)).collect());
    universals.indices = names.iter().enumerate().map(|(i, (r, _))| (*r, i)).collect();
    let mut constraints = MirTypeckRegionConstraints::new(LocationMap::new(b).num_points());
    for c in outlives { constraints.outlives_constraints.push(c); }
    let tcx: &'static Tcx = unsafe { &*(tcx as *const Tcx) };
    let mut fake = Fake::new(tcx, names.len());
    for (i, _) in names.iter().enumerate() { fake.regions[i].0 = Origin::FreeRegion; }
    (MirTypeck { universals, known: vec![], n_explicit_known: 0, constraints, polonius: PoloniusContext::default(), tainted_by_errors: false,
        summary: MirTypeckSummary { errors: vec![], mir_bugs: vec![], dropck_overflows: vec![], deferred_closure_requirements: vec![] }, closure_slots: vec![], closure_sig_erased_offset: 0 }, fake)
}
fn edge(sup: usize, sub: usize, line: u32, category: Cat) -> OutlivesConstraint {
    OutlivesConstraint { sup, sub, location: Some(0), span: span(line), category, from_closure: false }
}
fn uses(id: ClosureId, point: Location) -> DeferredClosureRequirement {
    DeferredClosureRequirement { def: ClauseOwner::Closure(id), args: vec![], locations: Locations::Single(point) }
}
const CHILD: ClosureId = ClosureId { file: 7, expr: 1 };

#[test]
fn nested_region_requirements_reach_the_parent_before_it_is_solved() {
    let tcx = Tcx::new(); let (a, b_) = (region(&tcx, 0), region(&tcx, 1));
    let body = empty_body(&tcx); let moves = MoveData::gather_moves(&body, &test_prog(), &tcx, crate::tcx::TypingEnv::fully_monomorphized(&test_prog()), &|_| true);
    let stat = tcx.tys.intern_region(RegionKind::ReStatic);
    // The child's `'a: 'b` is not provable there; its external regions are the parent's, in the other order.
    let (child, mut cf) = typeck(&tcx, &body, &[(stat, Class::Global), (a, Class::External), (b_, Class::External)], vec![edge(1, 2, 13, Cat::Return(ReturnConstraint::Normal))]);
    let (mut parent, mut pf) = typeck(&tcx, &body, &[(stat, Class::Global), (b_, Class::External), (a, Class::External)], vec![]);
    parent.summary.deferred_closure_requirements = vec![uses(CHILD, at(0, 0))];
    pf.query_ty = Some(tcx.tys.i32_);
    let prog = test_prog();
    let result = BorrowCheckRootCtxt { tcx: &tcx, prog: &prog }.do_mir_borrowck(vec![
        RootBody { key: ClauseOwner::Closure(CHILD), body: &body, moves: &moves, infcx: &mut cf, typeck: child, nested: true },
        RootBody { key: ClauseOwner::Def(DefId(0)), body: &body, moves: &moves, infcx: &mut pf, typeck: parent, nested: false },
    ]);
    let [child, parent] = &result.bodies[..] else { panic!() };
    // The child propagates the requirement instead of reporting it, but is not accepted: the root failed.
    assert!(child.result.errors.is_empty() && child.result.region_errors.is_empty());
    assert!(child.result.tainted_by_errors && !child.result.accepted());
    let requirements = child.requirements.as_ref().unwrap();
    assert_eq!((requirements.num_external_vids, requirements.outlives.len()), (3, 1));
    // `'a: 'b` is the parent's vid 2 outliving vid 1, which nothing implies there.
    assert!(!parent.result.accepted());
    assert!(parent.result.region_errors.iter().any(|e| matches!(e, super::region_infer::RegionErrorKind::RegionError { longer_fr: 2, shorter_fr: 1, .. })));
    // The original blame is kept: span, category and the use's location, marked as from the closure.
    let blame = parent.regions.regioncx.best_blame_constraint(&tcx, 2, parent.regions.regioncx.definitions[2].origin, 1).unwrap();
    assert_eq!((blame.span.lo, blame.span.file, blame.category, blame.location, blame.from_closure), (13, 7, Cat::Return(ReturnConstraint::Normal), Some(0), true));
}

#[test]
fn nested_type_requirements_are_converted_by_the_parents_context() {
    use super::region_infer::{TypeTest, VerifyBound};
    let tcx = Tcx::new();
    let body = empty_body(&tcx); let moves = MoveData::gather_moves(&body, &test_prog(), &tcx, crate::tcx::TypingEnv::fully_monomorphized(&test_prog()), &|_| true);
    let (stat, a) = (tcx.tys.intern_region(RegionKind::ReStatic), region(&tcx, 0));
    let (mut child, mut cf) = typeck(&tcx, &body, &[(stat, Class::Global), (a, Class::External)], vec![]);
    // `T: 'a` for a type parameter: unprovable in the child, so promoted to its creator.
    child.constraints.type_tests.push(TypeTest { generic_kind: tcx.tys.intern(TyKind::Param(0)), lower_bound: 1,
        span: super::constraints::Span { lo: 4, hi: 5 }, verify_bound: VerifyBound::AnyBound(vec![]) });
    let (mut parent, mut pf) = typeck(&tcx, &body, &[(stat, Class::Global), (a, Class::External)], vec![]);
    parent.summary.deferred_closure_requirements = vec![uses(CHILD, at(0, 0))];
    pf.query_ty = Some(tcx.tys.i32_);
    let prog = test_prog();
    let result = BorrowCheckRootCtxt { tcx: &tcx, prog: &prog }.do_mir_borrowck(vec![
        RootBody { key: ClauseOwner::Closure(CHILD), body: &body, moves: &moves, infcx: &mut cf, typeck: child, nested: true },
        RootBody { key: ClauseOwner::Def(DefId(0)), body: &body, moves: &moves, infcx: &mut pf, typeck: parent, nested: false },
    ]);
    let child = &result.bodies[0];
    assert!(child.result.region_errors.is_empty(), "{:?}", child.result.region_errors);
    let requirements = child.requirements.as_ref().unwrap();
    assert!(!requirements.types.is_empty());
    // Applied through the parent's inference context at the aggregate's location, once per requirement.
    assert_eq!(pf.pushed, vec![Locations::Single(at(0, 0)); requirements.types.len()]);
}

#[test]
fn bodies_depending_on_opaques_get_requirements_twice_and_hidden_types_are_shared() {
    let tcx = Tcx::new(); let (prog, tcx2, opaque) = (|| { let (p, t, o) = super::type_check_tests::opaque_fixture(); (p, t, o) })();
    let _ = tcx;
    let tcx = tcx2;
    let body = empty_body(&tcx); let moves = MoveData::gather_moves(&body, &prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &|_| true);
    let (stat, a) = (tcx.tys.intern_region(RegionKind::ReStatic), region(&tcx, 0));
    let (child, mut cf) = typeck(&tcx, &body, &[(stat, Class::Global), (a, Class::External)], vec![edge(1, 0, 13, Cat::Assignment)]);
    let (mut parent, mut pf) = typeck(&tcx, &body, &[(stat, Class::Global), (a, Class::External)], vec![]);
    parent.summary.deferred_closure_requirements = vec![uses(CHILD, at(0, 0))];
    pf.query_ty = Some(tcx.tys.i32_);
    // The parent defines the opaque `Opaque<'static, 'static> := i32`; the child has no opaque use.
    let key = OpaqueTypeKey { def_id: opaque, args: vec![GenericArg::Lifetime(stat), GenericArg::Lifetime(stat)].into() };
    pf.opaques = vec![(key.clone(), ProvisionalHiddenType::new(tcx.tys.i32_))];
    let result = BorrowCheckRootCtxt { tcx: &tcx, prog: &prog }.do_mir_borrowck(vec![
        RootBody { key: ClauseOwner::Closure(CHILD), body: &body, moves: &moves, infcx: &mut cf, typeck: child, nested: true },
        RootBody { key: ClauseOwner::Def(DefId(0)), body: &body, moves: &moves, infcx: &mut pf, typeck: parent, nested: false },
    ]);
    // The hidden type is registered once in the opaque's own parameters, and every use equated with it.
    assert_eq!(result.hidden_types, vec![(opaque, tcx.tys.i32_)]);
    assert_eq!(pf.equated, vec![(tcx.tys.i32_, tcx.tys.i32_)]);
    // The child does not depend on opaques: it was checked eagerly, and the parent applied its final
    // requirements exactly once.
    let parent = &result.bodies[1];
    assert!(parent.deferred_opaque_type_errors.is_empty());
    assert_eq!(parent.regions.regioncx.outlives_constraints().outlives.iter().filter(|c| c.from_closure).count(), 1);
}

#[test]
fn a_nested_body_with_opaque_uses_applies_its_requirements_modulo_opaques_and_then_finally() {
    let (prog, tcx, opaque) = super::type_check_tests::opaque_fixture();
    let body = empty_body(&tcx); let moves = MoveData::gather_moves(&body, &prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &|_| true);
    let (stat, a) = (tcx.tys.intern_region(RegionKind::ReStatic), region(&tcx, 0));
    let (child, mut cf) = typeck(&tcx, &body, &[(stat, Class::Global), (a, Class::External)], vec![edge(1, 0, 13, Cat::Assignment)]);
    let (mut parent, mut pf) = typeck(&tcx, &body, &[(stat, Class::Global), (a, Class::External)], vec![]);
    parent.summary.deferred_closure_requirements = vec![uses(CHILD, at(0, 0))];
    pf.query_ty = Some(tcx.tys.i32_);
    // The child uses the opaque (a non-defining use there); the parent has the defining use.
    let key = OpaqueTypeKey { def_id: opaque, args: vec![GenericArg::Lifetime(stat), GenericArg::Lifetime(stat)].into() };
    cf.opaques = vec![(key.clone(), ProvisionalHiddenType::new(tcx.tys.i32_))];
    cf.non_defining = vec![key.clone()];
    pf.opaques = vec![(key, ProvisionalHiddenType::new(tcx.tys.i32_))];
    let result = BorrowCheckRootCtxt { tcx: &tcx, prog: &prog }.do_mir_borrowck(vec![
        RootBody { key: ClauseOwner::Closure(CHILD), body: &body, moves: &moves, infcx: &mut cf, typeck: child, nested: true },
        RootBody { key: ClauseOwner::Def(DefId(0)), body: &body, moves: &moves, infcx: &mut pf, typeck: parent, nested: false },
    ]);
    assert_eq!(result.hidden_types, vec![(opaque, tcx.tys.i32_)]);
    // The child's use is checked against the parent's definition, though it is not a defining use itself.
    assert_eq!(cf.equated, vec![(tcx.tys.i32_, tcx.tys.i32_)]);
    // Requirements modulo opaques, then the final ones: both reach the parent.
    assert_eq!(result.bodies[1].regions.regioncx.outlives_constraints().outlives.iter().filter(|c| c.from_closure).count(), 2);
}

#[test]
fn a_failed_body_taints_the_whole_root() {
    let tcx = Tcx::new();
    let body = empty_body(&tcx); let moves = MoveData::gather_moves(&body, &test_prog(), &tcx, crate::tcx::TypingEnv::fully_monomorphized(&test_prog()), &|_| true);
    let stat = tcx.tys.intern_region(RegionKind::ReStatic);
    let (ok, mut okf) = typeck(&tcx, &body, &[(stat, Class::Global)], vec![]);
    let (mut failed, mut ff) = typeck(&tcx, &body, &[(stat, Class::Global)], vec![]);
    failed.tainted_by_errors = true;
    let prog = test_prog();
    let result = BorrowCheckRootCtxt { tcx: &tcx, prog: &prog }.do_mir_borrowck(vec![
        RootBody { key: ClauseOwner::Closure(CHILD), body: &body, moves: &moves, infcx: &mut okf, typeck: ok, nested: true },
        RootBody { key: ClauseOwner::Def(DefId(0)), body: &body, moves: &moves, infcx: &mut ff, typeck: failed, nested: false },
    ]);
    assert!(result.bodies.iter().all(|c| c.result.tainted_by_errors && !c.result.accepted()));
}

#[test]
fn the_creator_learns_what_its_nested_bodies_were_written_as() {
    let tcx = Tcx::new();
    let mut child_body = empty_body(&tcx);
    let kind = CoroutineKind::Desugared(CoroutineDesugaring::Async, CoroutineSource::Block);
    child_body.coroutine = Some(CoroutineInfo { yield_ty: tcx.tys.unit, resume_ty: tcx.tys.unit, kind });
    let body = empty_body(&tcx); let moves = MoveData::gather_moves(&body, &test_prog(), &tcx, crate::tcx::TypingEnv::fully_monomorphized(&test_prog()), &|_| true);
    let stat = tcx.tys.intern_region(RegionKind::ReStatic);
    let (child, mut cf) = typeck(&tcx, &child_body, &[(stat, Class::Global)], vec![]);
    let (mut parent, mut pf) = typeck(&tcx, &body, &[(stat, Class::Global)], vec![]);
    parent.summary.deferred_closure_requirements = vec![DeferredClosureRequirement { def: ClauseOwner::Coroutine(CHILD), args: vec![], locations: Locations::Single(at(0, 0)) }];
    pf.query_ty = Some(tcx.tys.i32_);
    let prog = test_prog();
    let result = BorrowCheckRootCtxt { tcx: &tcx, prog: &prog }.do_mir_borrowck(vec![
        RootBody { key: ClauseOwner::Coroutine(CHILD), body: &child_body, moves: &moves, infcx: &mut cf, typeck: child, nested: true },
        RootBody { key: ClauseOwner::Def(DefId(0)), body: &body, moves: &moves, infcx: &mut pf, typeck: parent, nested: false },
    ]);
    assert_eq!(result.bodies[1].coroutine_kinds, vec![(CHILD.expr, kind)]);
    assert!(result.bodies[0].coroutine_kinds.is_empty());
}
