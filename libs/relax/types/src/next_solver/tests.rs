use super::*;
use crate::program::Program;
use crate::tcx::{Tcx, TypingMode};
use crate::infer::InferCtxt;
use crate::types::{TyKind, AliasTyKind, RegionKind};
fn program(source: &str, label: &str) -> (Program, Tcx) {
    let dir = std::env::temp_dir().join(format!("relax-round11-solver-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap(); let path = dir.join("lib.rs"); std::fs::write(&path, source).unwrap();
    let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
    prog.load_crate(label, path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap(); (prog, tcx)
}
fn named(prog: &Program, name: &str) -> DefId { prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == name).map(|(i,_)| DefId(i as u32)).unwrap() }
#[test]
fn round11_impl_candidates_and_nested_projection() {
    let (p, t) = program("trait Marker {} impl Marker for u32 {} trait Iter { type Item; } impl Iter for u32 { type Item = u32; } trait Outer { type Item; } impl<T: Iter> Outer for T where T::Item: Marker { type Item = T::Item; }", "projection");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default()); let env = ParamEnvId::EMPTY;
    let marker = named(&p, "Marker");
    for (ty, yes) in [(t.tys.u32_, true), (t.tys.bool_, false)] {
        let r = infcx.evaluate_root_goal(&p, Goal { param_env: env.clone(), predicate: collect::trait_predicate(marker, vec![GenericArg::Type(ty)]) }, None);
        assert_eq!(r.is_ok_and(|r| r.certainty.is_yes()), yes);
    }
    let outer = named(&p, "Outer"); let assoc = p.traits[p.def(outer).sub as usize][0];
    let alias = t.alias_ty_with_args(AliasTyKind::Projection { def_id: assoc }, &[GenericArg::Type(t.tys.u32_)]);
    let mut ocx = fulfill::ObligationCtxt::new(&infcx, &p);
    assert_eq!(ocx.deeply_normalize_ty(&env, alias).unwrap(), t.tys.u32_);
}
#[test]
fn round11_projection_keeps_full_lifetime_arguments() {
    let (p,t) = program("trait Get<'a> { type Out; } struct S; impl<'a> Get<'a> for S { type Out = &'a u32; }", "regions");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let a = t.tys.intern_region(RegionKind::ReLateParam(crate::types::RegionScope::Item(named(&p,"Get")), crate::types::LateParamKind::Anon(0)));
    let s = t.tys.intern(TyKind::Adt(named(&p,"S"), t.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
    let alias = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p,"Out") }, &[GenericArg::Type(s), GenericArg::Lifetime(a)]);
    let mut ocx = fulfill::ObligationCtxt::new(&infcx,&p);
    assert_eq!(ocx.deeply_normalize_ty(&ParamEnvId::EMPTY, alias).unwrap(), t.tys.intern(TyKind::Ref(false,t.tys.u32_,a)));
}
#[test]
fn round11_param_env_projection_is_rigid_without_equality_bound() {
    let (p,t) = program("trait Tr { type Out; }", "rigid");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default()); let param = t.tys.intern(TyKind::Param(0));
    let env = t.intern_param_env(ParamEnv { clauses: vec![collect::trait_predicate(named(&p,"Tr"),vec![GenericArg::Type(param)])].into() });
    let alias = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p,"Out") }, &[GenericArg::Type(param)]);
    let mut ocx = fulfill::ObligationCtxt::new(&infcx,&p); let value = ocx.deeply_normalize_ty(&env,alias).unwrap();
    assert!(matches!(t.tys.kind(value), TyKind::Alias(crate::consts::IsRigid::Yes,_)));
}
#[test]
fn round11_implied_wf_bounds_supply_generic_outlives() {
    let (p,t) = program("", "implied");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let a = t.tys.intern_region(RegionKind::ReEarlyParam(0)); let param = t.tys.intern(TyKind::Param(0));
    let input = t.tys.intern(TyKind::Ref(false,param,a));
    infcx.register_ty_outlives(param,a);
    assert!(infcx.resolve_regions(&p,ParamEnvId::EMPTY,&[input]).is_empty());
}
#[test]
fn round33_implied_bounds_failure_drops_only_that_type() {
    // rustc_trait_selection/src/traits/outlives_bounds.rs:58-73: a type whose implied-bounds
    // query fails contributes no bounds; the others still apply (no whole-environment failure).
    let (p,t) = program("trait Tr { type Out; }", "implied_failure");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let a = t.tys.intern_region(RegionKind::ReEarlyParam(0)); let param = t.tys.intern(TyKind::Param(0));
    let input = t.tys.intern(TyKind::Ref(false,param,a));
    let failing = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p,"Out") }, &[GenericArg::Type(t.tys.u8_)]);
    assert!(implied_outlives_bounds::implied_outlives_bounds(&infcx, &p, &ParamEnvId::EMPTY, failing).is_empty());
    assert_eq!(implied_outlives_bounds::implied_outlives_bounds(&infcx, &p, &ParamEnvId::EMPTY, input).len(), 1);
    assert_eq!(implied_outlives_bounds::implied_outlives_bounds(&infcx, &p, &ParamEnvId::EMPTY, t.tys.u32_).len(), 0);
    infcx.register_ty_outlives(param,a);
    assert!(infcx.resolve_regions(&p,ParamEnvId::EMPTY,&[failing, input]).is_empty());
}
#[test]
fn round11_rpitit_rejects_e0477_and_accepts_declared_outlives() {
    for positive in [false,true] {
        let clause = if positive { " where 'b: 'a" } else { "" };
        let source = format!("trait Copy {{}} impl<'x> Copy for &'x u32 {{}} trait Tr {{ fn f<'a, 'b>(_: &'a u32, x: &'b u32) -> impl Copy + 'a{clause}; }} struct S; impl Tr for S {{ fn f<'a, 'b>(_: &'a u32, x: &'b u32) -> &'b u32{clause} {{ x }} }}");
        let (p,t) = program(&source, if positive { "rpitit_yes" } else { "rpitit_no" });
        let method = *p.trait_item_def_id.keys().find(|&&d| p.def(d).kind == crate::program::DefKind::AssocFn).unwrap();
        let r = t.collect_return_position_impl_trait_in_trait_tys(&p,method);
        assert_eq!(r.is_ok(),positive,"{r:?}");
    }
}
#[test]
fn round33_rpitit_hidden_regions_map_through_captured_duplicates() {
    // compare_impl_item.rs:717-719,921-928: the remap skips the trait method's full generics,
    // so an impl lifetime maps to the RPITIT's captured duplicate (later IndexMap key wins),
    // renumbered by -trait args +impl args.
    let (p,t) = program("trait Copy {} impl<'x> Copy for &'x u8 {} trait T { fn get<'a: 'a>(&'a self, x: &'a u8) -> impl Copy; } struct S; impl T for S { fn get<'a: 'a>(&'a self, x: &'a u8) -> impl Copy { x } }", "rpitit_remap");
    let method = *p.trait_item_def_id.keys().find(|&&d| p.def(d).kind == crate::program::DefKind::AssocFn).unwrap();
    let values = t.collect_return_position_impl_trait_in_trait_tys(&p, method).unwrap();
    assert_eq!(values.len(), 1);
    let (rpitit, ty) = values[0];
    // The value is the impl's opaque; its region arguments are the impl method's `'a`.
    let TyKind::Alias(_, alias) = t.tys.kind(ty).clone() else { panic!("RPITIT value {}", crate::typeck::ty_to_string(false, &p, &t, ty)) };
    let duplicate = *t.generic_indices_of(&p, rpitit).lifetimes.last().unwrap();
    let regions = t.tys.region_args(alias.regions);
    assert!(!regions.is_empty());
    for &r in regions.iter() { assert_eq!(t.tys.region_kind(r), RegionKind::ReEarlyParam(duplicate - 1)); }
    assert_eq!(t.dcx.err_count(), 0);
}
#[test]
fn round11_rpitit_full_signature_checks_function_header() {
    let (p,t) = program("trait Copy {} impl Copy for u32 {} trait Tr { fn f() -> impl Copy; } struct S; impl Tr for S { unsafe fn f() -> u32 { 1 } }", "rpitit_header");
    let method = *p.trait_item_def_id.keys().find(|&&d| p.def(d).kind == crate::program::DefKind::AssocFn).unwrap();
    assert!(t.collect_return_position_impl_trait_in_trait_tys(&p,method).is_err());
}

#[test]
fn round12_global_cache_replays_reads_and_invalidates_with_revision() {
    let (p, mut t) = program("trait Marker {} impl Marker for u32 {}", "global_cache");
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p,"Marker"),vec![GenericArg::Type(t.tys.u32_)]) };
    let node = {
        let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
        let (r, node) = t.queries.engine.with_anon_task(&mut || infcx.evaluate_root_goal(&p, goal.clone(), None));
        assert!(r.unwrap().certainty.is_yes()); node
    };
    let cached = {
        let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
        let (r, hit) = t.queries.engine.with_anon_task(&mut || infcx.evaluate_root_goal(&p, goal.clone(), None));
        assert!(r.unwrap().certainty.is_yes()); hit
    };
    assert_eq!(node,cached,"cache hits record the same collected-program dependency");
    t.trait_impls.clear(); t.publish_collected();
    let infcx = InferCtxt::new(&t,TypingMode::PostAnalysis, Default::default());
    assert!(infcx.evaluate_root_goal(&p,goal,None).is_err());
}
#[test]
fn round12_erased_opaque_rerun_preserves_owner_scope() {
    let (p,t) = program("trait Marker {} impl Marker for bool {} fn f() -> impl Marker { true }", "erased_opaque");
    let opaque = p.defs.iter().position(|d| d.kind == crate::program::DefKind::OpaqueTy).map(|i| DefId(i as u32)).unwrap();
    let alias = t.alias_ty_with_args(AliasTyKind::Opaque { def_id: opaque }, &[]);
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(alias), term: Term::Ty(t.tys.bool_) }))) };
    for owns in [false,true,false] {
        let owners = t.intern_defining_opaques(if owns { vec![opaque] } else { vec![] });
        let infcx = InferCtxt::new(&t, TypingMode::Typeck { defining_opaque_types_and_generators: owners }, Default::default());
        assert_eq!(infcx.evaluate_root_goal(&p,goal.clone(),None).is_ok(),owns);
        assert_eq!(infcx.inner.borrow_mut().opaque_types().is_empty(),!owns);
    }
}
#[test]
fn round12_stalled_fulfillment_wakes_on_type_and_subtype_progress() {
    let (p,t) = program("trait Marker {} impl Marker for bool {}", "stalled");
    let infcx = InferCtxt::new(&t,TypingMode::PostAnalysis, Default::default()); let env = ParamEnvId::EMPTY;
    let a = infcx.next_ty_var(); let b = infcx.next_ty_var();
    let goal = Goal { param_env: env.clone(), predicate: collect::trait_predicate(named(&p,"Marker"),vec![GenericArg::Type(a)]) };
    let result = infcx.evaluate_root_goal(&p,goal.clone(),None).unwrap();
    assert!(result.stalled_on.is_some());
    assert!(!eval_ctxt::fast_path::rerunning_stalled_goal_may_make_progress(&infcx,result.stalled_on.as_ref()));
    let mut ocx = fulfill::ObligationCtxt::new(&infcx,&p); ocx.register_obligation(goal);
    ocx.register_obligations(infcx.at(&p, env.clone()).sub(a,b).unwrap());
    assert!(ocx.engine.try_evaluate_obligations(&infcx,&p).is_empty());
    assert!(ocx.engine.has_pending_obligations());
    ocx.eq(&env,b,t.tys.bool_).unwrap();
    ocx.eq(&env,a,t.tys.bool_).unwrap();
    assert!(eval_ctxt::fast_path::rerunning_stalled_goal_may_make_progress(&infcx,result.stalled_on.as_ref()));
    ocx.evaluate_obligations_error_on_ambiguity().unwrap();
}
#[test]
fn round12_structural_unsize_requires_only_tail_parameters() {
    let (p,t) = program("#[lang = \"sized\"] trait Sized {} #[lang = \"unsize\"] trait Unsize<T: ?Sized> {} struct Tail<T: ?Sized> { head: u32, tail: T } struct Repeated<T> { first: T, last: T }", "unsize");
    let infcx = InferCtxt::new(&t,TypingMode::PostAnalysis, Default::default());
    let array = t.tys.intern(TyKind::Array(t.tys.u32_,t.tys.const_usize(2)));
    let slice = t.tys.intern(TyKind::Slice(t.tys.u32_));
    let wrong = t.tys.intern(TyKind::Slice(t.tys.bool_));
    let wrap = |name,ty| t.tys.intern(TyKind::Adt(named(&p,name),t.tys.mk_list(&[ty]),crate::types::RegionArgsId::EMPTY));
    for (a,b,yes) in [(array,slice,true),(array,wrong,false),(wrap("Tail",array),wrap("Tail",slice),true),(wrap("Repeated",array),wrap("Repeated",slice),false)] {
        let goal = Goal { param_env:ParamEnvId::EMPTY,predicate:collect::trait_predicate(named(&p,"Unsize"),vec![GenericArg::Type(a),GenericArg::Type(b)]) };
        assert_eq!(infcx.evaluate_root_goal(&p,goal,None).is_ok_and(|r| r.certainty.is_yes()),yes);
    }
}
#[test]
fn round12_coherence_distinguishes_upstream_downstream_and_local_ownership() {
    let (mut p,t) = program("trait Foreign<T> {} struct Local; struct Container<T>(T);", "coherence");
    // Model the per-crate definition identities of a collected dependency graph.
    let local=named(&p,"Local"); p.defs[local.0 as usize].krate=1;
    let mut infcx=InferCtxt::new(&t,TypingMode::Coherence, Default::default()); infcx.coherence_crate=Some(1);
    let local_ty=t.tys.intern(TyKind::Adt(local,t.tys.mk_list(&[]),crate::types::RegionArgsId::EMPTY));
    let container=t.tys.intern(TyKind::Adt(named(&p,"Container"),t.tys.mk_list(&[local_ty]),crate::types::RegionArgsId::EMPTY));
    let unknown=infcx.next_ty_var();
    for (ty,expected) in [(t.tys.bool_,Err(coherence::Conflict::Upstream)),(local_ty,Ok(())),(container,Err(coherence::Conflict::Upstream)),(unknown,Err(coherence::Conflict::Downstream))] {
        let tr=TraitRef { def_id:named(&p,"Foreign"),args:vec![GenericArg::Type(ty),GenericArg::Type(t.tys.bool_)] };
        let actual=infcx.enter_root(&p,|ecx| coherence::trait_ref_is_knowable(ecx,&ParamEnvId::EMPTY,&tr,1)).unwrap();
        assert_eq!(actual,expected);
    }
    let tr=TraitRef { def_id:named(&p,"Foreign"),args:vec![GenericArg::Type(unknown),GenericArg::Type(local_ty)] };
    let result=coherence::orphan_check_trait_ref(&t,&p,&tr,1,coherence::InCrate::Local { mode: coherence::OrphanCheckMode::Proper },&mut |ty| { let ty = infcx.shallow_resolve(ty); (ty, Ok(ty)) }).unwrap();
    assert!(matches!(result,Err(coherence::OrphanCheckErr::UncoveredTyParams { local_ty:Some(t),.. }) if t==local_ty));
}
#[test]
fn round33_orphan_check_compat_mode_accepts_uncovered_projection() {
    // rustc_hir_analysis/src/coherence/orphan.rs:20-50,305-385 + rustc_next_trait_solver
    // coherence.rs (Alias arm): an uncovered type parameter inside a rigid projection fails in
    // Proper mode and passes in Compat mode (future-compat lint); a bare `T` fails in both.
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    let dir = std::env::temp_dir().join(format!("relax-r33-orphan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (dep, local) = (dir.join("dep.rs"), dir.join("local.rs"));
    std::fs::write(&dep, "#![no_std] pub trait Foreign<T> {} #[fundamental] pub struct Fund<T: ?Sized>(T); pub struct Plain<T: ?Sized>(T);").unwrap();
    std::fs::write(&local, "#![no_std] trait Tr { type Assoc; } struct Local; impl<T: Tr> dep::Foreign<Local> for <T as Tr>::Assoc {} impl<T> dep::Foreign<Local> for T {} impl dep::Foreign<u8> for dep::Fund<Local> {} impl dep::Foreign<u8> for dep::Plain<Local> {}").unwrap();
    prog.load_crate("dep", dep.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    let dep_name = prog.syms.intern("dep");
    prog.load_crate("local", local.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![(dep_name, 0)]).unwrap();
    prog.resolve_imports();
    std::fs::remove_file(&dep).unwrap(); std::fs::remove_file(&local).unwrap(); std::fs::remove_dir(&dir).unwrap();
    let mut t = Tcx::new(); t.collect(&prog);
    let impls: Vec<DefId> = (0..prog.defs.len() as u32).map(DefId).filter(|&d| prog.def(d).kind == crate::program::DefKind::Impl).collect();
    assert_eq!(impls.len(), 4);
    // coherence.rs Adt arm: a #[fundamental] type is looked through, a plain foreign one is not.
    assert!(t.orphan_check_impl(&prog, impls[2], Proper).is_ok());
    assert!(matches!(t.orphan_check_impl(&prog, impls[3], Proper), Err(OrphanCheckErr::NonLocalInputType(_))));
    use coherence::{OrphanCheckMode::{Proper, Compat}, OrphanCheckErr};
    assert!(matches!(t.orphan_check_impl(&prog, impls[0], Proper), Err(OrphanCheckErr::UncoveredTyParams { .. })));
    assert!(t.orphan_check_impl(&prog, impls[0], Compat).is_ok());
    for mode in [Proper, Compat] {
        assert!(matches!(t.orphan_check_impl(&prog, impls[1], mode), Err(OrphanCheckErr::UncoveredTyParams { .. })), "{mode:?}");
    }
    // The collection-time check reports E0210 for the bare parameter only and warns for the projection.
    assert_eq!(t.errors.iter().filter(|e| e.contains("E0210")).count(), 1, "{:?}", t.errors);
    assert_eq!(t.errors.iter().filter(|e| e.contains("E0117")).count(), 1, "{:?}", t.errors);
    let (_, warnings) = t.dcx.drain_reported();
    assert!(warnings.iter().any(|w| w.message.contains("uncovered_param_in_projection")));
}
#[test]
fn round33_elaboration_dedups_alpha_equivalent_supertraits() {
    // rustc_type_ir/src/elaborate.rs:101-112: supertraits differing only in bound-variable
    // names (`for<'x> B<'x>` / `for<'y> B<'y>`) are one elaborated predicate.
    let (p,t) = program("trait B<'a> {} trait A: for<'x> B<'x> + for<'y> B<'y> {}", "elab_anon");
    let a = named(&p, "A"); let b = named(&p, "B");
    let initial = collect::trait_predicate(a, vec![GenericArg::Type(t.tys.u32_)]);
    let elaborated = t.supertraits(&p, initial);
    let count = elaborated.iter().filter(|pred| matches!(&pred.kind, PredicateKind::Clause(ClauseKind::Trait(tr)) if tr.trait_ref.def_id == b)).count();
    assert_eq!(count, 1, "{elaborated:?}");
}
#[test]
fn round12_unsize_to_object_and_supertrait_upcast() {
    let (p,t)=program("#[lang = \"sized\"] trait Sized {} #[lang = \"unsize\"] trait Unsize<T: ?Sized> {} trait Parent {} trait Child: Parent {} impl Parent for u32 {} impl Child for u32 {}", "dyn_unsize");
    let infcx=InferCtxt::new(&t,TypingMode::PostAnalysis, Default::default());
    let make_dyn=|name| t.tys.intern(TyKind::Dyn(named(&p,name),crate::types::List::EMPTY,crate::types::List::EMPTY,crate::types::RegionArgsId::EMPTY,t.tys.intern_region(RegionKind::ReStatic),0,crate::types::List::EMPTY));
    let parent=make_dyn("Parent"); let child=make_dyn("Child");
    for (a,b,yes) in [(t.tys.u32_,child,true),(child,parent,true),(parent,child,false),(t.tys.bool_,child,false)] {
        let goal=Goal { param_env:ParamEnvId::EMPTY,predicate:collect::trait_predicate(named(&p,"Unsize"),vec![GenericArg::Type(a),GenericArg::Type(b)]) };
        assert_eq!(infcx.evaluate_root_goal(&p,goal,None).is_ok_and(|r| r.certainty.is_yes()),yes);
    }
}

#[test]
fn closure_uses_minimum_capture_types() {
    let (p,t) = program("auto trait Auto {} struct NotAuto { value: u64 } impl !Auto for NotAuto {} struct Pair { good: u32, bad: NotAuto } fn owner(pair: Pair) { let c = move || pair.good; }", "capture_storage");
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&p, &t,crate::tcx::TypingEnv::fully_monomorphized(&p),&state);
    let body = crate::typeck::instantiate_body(&cx,&p,&t,named(&p,"owner"),&[]);
    assert!(body.errors.is_empty(),"{:?}",body.errors);
    let expr = body.closures[0];
    let closure = body.ty(crate::ast::ExprId(expr));
    let captures = &body.min_captures[&expr];
    assert_eq!(captures.captures.len(),1);
    assert_eq!(captures.captures[0].place.ty(),t.tys.u32_);
    let TyKind::Closure(_, _, args) = t.tys.kind(closure) else { panic!() };
    let semantic = args.tupled_upvars_ty;
    assert!(t.tys.kind(semantic) == &TyKind::Tuple(t.tys.mk_list(&[t.tys.u32_])));
    let layout = crate::layout::ClosureStorage::new(&t, &body, expr);
    assert_eq!(layout.fields.len(), 1);
    assert_ne!(layout.fields[0], t.tys.u32_);
    let infcx = InferCtxt::new(&t,TypingMode::PostAnalysis, Default::default());
    let goal = |ty| Goal { param_env:ParamEnvId::EMPTY,predicate:collect::trait_predicate(named(&p,"Auto"),vec![GenericArg::Type(ty)]) };
    assert!(infcx.evaluate_root_goal(&p,goal(semantic),None).unwrap().certainty.is_yes());
    let result = infcx.evaluate_root_goal(&p,goal(closure),None);
    assert!(result.is_ok_and(|r| r.certainty.is_yes()),"native closure auto trait must use minimum captures");
}

#[test]
fn closure_kind_and_captures_change_canonical_input() {
    let (p,t) = program("#[lang = \"fn\"] trait Call<Args> {} auto trait Auto {} fn owner() {}", "closure_args");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let owner = t.tys.intern(TyKind::FnDef(named(&p,"owner"),t.tys.mk_list(&[]),crate::types::RegionArgsId::EMPTY));
    let kind = infcx.next_ty_var(); let upvars = infcx.next_ty_var();
    let sig = t.tys.intern(TyKind::FnPtr(t.tys.mk_list(&[]),t.tys.unit,crate::types::FnHeader::RUST,0));
    let closure = t.tys.intern(TyKind::Closure(0,0,crate::types::ClosureArgs { parent_args:owner, closure_kind_ty:kind, closure_sig_as_fn_ptr_ty:sig, tupled_upvars_ty:upvars }));
    let env = ParamEnvId::EMPTY;
    let call = Goal { param_env:env.clone(), predicate:collect::trait_predicate(named(&p,"Call"),vec![GenericArg::Type(closure), GenericArg::Type(t.tys.unit)]) };
    let auto = Goal { param_env:env.clone(), predicate:collect::trait_predicate(named(&p,"Auto"),vec![GenericArg::Type(closure)]) };
    let before_call = infcx.evaluate_root_goal(&p,call.clone(),None).unwrap();
    let before_auto = infcx.evaluate_root_goal(&p,auto.clone(),None).unwrap();
    assert!(!before_call.certainty.is_yes()); assert!(!before_auto.certainty.is_yes());
    assert!(infcx.at(&p, env.clone()).eq(kind,t.tys.from_closure_kind(crate::upvar::ClosureKind::Fn)).unwrap().is_empty());
    assert!(infcx.at(&p, env).eq(upvars,t.tys.intern(TyKind::Tuple(t.tys.mk_list(&[t.tys.u32_])))).unwrap().is_empty());
    assert!(infcx.evaluate_root_goal(&p,call,before_call.stalled_on.as_ref()).unwrap().certainty.is_yes());
    assert!(infcx.evaluate_root_goal(&p,auto,before_auto.stalled_on.as_ref()).unwrap().certainty.is_yes());
}

#[test]
fn closure_clone_requires_captured_places_not_storage_roots() {
    let (p,t) = program("#[lang = \"clone\"] trait Clone { fn clone(&self) -> Self; } struct Good(u32); impl Good { fn read(&self) -> u32 { self.0 } } impl Clone for Good { fn clone(&self) -> Self { Good(self.0 + 1) } } struct NotClone(u32); struct Pair { good: Good, bad: NotClone } fn owner(pair: Pair) { let closure = move || pair.good.read(); }", "closure_clone");
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&p, &t,crate::tcx::TypingEnv::fully_monomorphized(&p),&state);
    let body = crate::typeck::instantiate_body(&cx,&p,&t,named(&p,"owner"),&[]);
    assert!(body.errors.is_empty(),"{:?}",body.errors);
    let expr = body.closures[0]; let closure = body.ty(crate::ast::ExprId(expr));
    let TyKind::Closure(_,_,args) = t.tys.kind(closure) else { panic!() };
    let TyKind::Tuple(upvars) = t.tys.kind(args.tupled_upvars_ty) else { panic!() };
    assert_eq!(t.tys.list(*upvars).len(),1);
    assert!(matches!(t.tys.kind(t.tys.list(*upvars)[0]),TyKind::Adt(d,..) if *d == named(&p,"Good")));
    let storage = crate::layout::ClosureStorage::new(&t,&body,expr);
    assert!(matches!(t.tys.kind(storage.fields[0]),TyKind::Adt(d,..) if *d == named(&p,"Pair")));
    let infcx = InferCtxt::new(&t,TypingMode::PostAnalysis, Default::default());
    let goal = |ty| Goal { param_env:ParamEnvId::EMPTY, predicate:collect::trait_predicate(named(&p,"Clone"),vec![GenericArg::Type(ty)]) };
    assert!(infcx.evaluate_root_goal(&p,goal(closure),None).unwrap().certainty.is_yes());
    assert!(infcx.evaluate_root_goal(&p,goal(storage.fields[0]),None).is_err());
}

#[test]
fn closure_storage_layout_keeps_root_slots_and_drop_flags() {
    let (p,t) = program("struct Good(u32); struct Pair { good: Good, bad: u64 } fn owner(pair: Pair) { let closure = move || pair.good; }", "storage_layout");
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&p, &t,crate::tcx::TypingEnv::fully_monomorphized(&p),&state);
    let body = crate::typeck::instantiate_body(&cx,&p,&t,named(&p,"owner"),&[]);
    assert!(body.errors.is_empty(),"{:?}",body.errors);
    let expr = body.closures[0]; let closure = body.ty(crate::ast::ExprId(expr));
    let storage = crate::layout::ClosureStorage::new(&t,&body,expr);
    assert!(storage.drop_flags); assert_eq!(storage.fields.len(),2);
    assert_eq!(storage.fields[1],t.tys.u8_);
    assert_eq!(storage.owned_fields,vec![Some(vec![0])]);
    let layouts = crate::layout::Layouts::new(); let lcx = crate::layout::LayoutCx::new(&p);
    let record = layouts.closure_storage_layout(&lcx,&t,closure);
    let tuple = t.tys.intern(TyKind::Tuple(t.tys.mk_list(&storage.fields)));
    let old = layouts.of(&lcx,&t,tuple);
    assert_eq!((record.size,record.align,&record.fields,&record.leaves),(old.size,old.align,&old.fields,&old.leaves));
    assert!(record.leaves.is_some());
    assert!(layouts.of(&lcx,&t,closure).leaves.is_none());
}

#[test]
fn round15_sizedness_uses_definition_constraints() {
    use super::sizedness::SizedTraitKind;
    let (p, t) = program("#[lang = \"sized\"] trait Sized {} #[lang = \"meta_sized\"] trait MetaSized {} struct Tail<T: ?Sized>(T); struct Nested<T: ?Sized>(Tail<T>); struct Bounded<T>(Tail<T>); struct SliceTail([u8]);", "sizedness");
    let param = t.tys.intern(TyKind::Param(0));
    for name in ["Tail", "Nested"] {
        assert_eq!(t.adt_sizedness_constraint(&p, named(&p,name), SizedTraitKind::Sized).unwrap(), Some(param));
    }
    assert_eq!(t.adt_sizedness_constraint(&p, named(&p,"Bounded"), SizedTraitKind::Sized).unwrap(), None);
    assert_eq!(t.adt_sizedness_constraint(&p, named(&p,"SliceTail"), SizedTraitKind::MetaSized).unwrap(), None);
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    for (arg, yes) in [(t.tys.u32_,true), (t.tys.intern(TyKind::Slice(t.tys.u8_)),false)] {
        let ty = t.tys.intern(TyKind::Adt(named(&p,"Nested"),t.tys.mk_list(&[arg]),crate::types::RegionArgsId::EMPTY));
        let result = infcx.evaluate_root_goal(&p,Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p,"Sized"),vec![GenericArg::Type(ty)]) },None);
        assert_eq!(result.is_ok_and(|r|r.certainty.is_yes()),yes);
    }
}

#[test]
fn round15_representability_tracks_only_stored_parameters() {
    for (source, yes) in [
        ("struct S { next: *const S }",true),
        ("struct Indirect<T>(*const T); struct S(Indirect<S>);",true),
        ("struct Repr<T>(T); struct S(Repr<S>);",false),
        ("struct S { next: S }",false),
        ("struct S { next: [S; 0] }",false),
        ("struct Repr<T>(T); struct S { ptr: *const S, next: Repr<S> }",false),
    ] {
        let (p,t) = program(source,"representability");
        assert_eq!(t.check_representability(&p,named(&p,"S")).is_ok(),yes,"{source}");
    }
}

#[test]
fn round15_sizedness_queries_invalidate_on_collection() {
    use super::sizedness::SizedTraitKind;
    let (p,mut t) = program("struct S(u32);", "sizedness_invalidation");
    let def = named(&p,"S");
    assert_eq!(t.adt_sizedness_constraint(&p,def,SizedTraitKind::Sized).unwrap(),None);
    let slice = t.tys.intern(TyKind::Slice(t.tys.u8_));
    t.adts.get_mut(&def).unwrap().variants[0].fields[0].ty = slice;
    t.publish_collected();
    assert_eq!(t.adt_sizedness_constraint(&p,def,SizedTraitKind::Sized).unwrap(),Some(slice));
}

#[test]
fn round15_normalization_defers_failed_projections_to_fulfillment() {
    let (p,t) = program("trait Tr { type Out; }", "deferred_normalization");
    let infcx = InferCtxt::new(&t,TypingMode::PostAnalysis, Default::default());
    let alias = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p,"Out") }, &[GenericArg::Type(t.tys.u32_)]);
    let mut ocx = fulfill::ObligationCtxt::new(&infcx,&p);
    let ty = ocx.normalize_ty(&ParamEnvId::EMPTY,alias);
    assert!(matches!(t.tys.kind(ty),TyKind::Infer(_)));
    assert!(ocx.engine.has_pending_obligations());
    assert!(ocx.evaluate_obligations_error_on_ambiguity().is_err());
}

#[test]
fn round15_failed_higher_ranked_normalization_keeps_bound_alias() {
    let (p,t) = program("trait Tr<'a> { type Out; }", "deferred_hr_normalization");
    let infcx = InferCtxt::new(&t,TypingMode::PostAnalysis, Default::default());
    let region = t.tys.intern_region(RegionKind::ReBound(0,crate::types::BoundVar(0)));
    let alias = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p,"Out") }, &[GenericArg::Type(t.tys.u32_),GenericArg::Lifetime(region)]);
    let sig = t.tys.intern(TyKind::FnPtr(t.tys.mk_list(&[]),alias,crate::types::FnHeader::RUST,1));
    let mut ocx = fulfill::ObligationCtxt::new(&infcx,&p);
    assert_eq!(ocx.normalize_ty(&ParamEnvId::EMPTY,sig),sig);
    assert_eq!(ocx.engine.pending_obligations().len(),1);
    assert!(ocx.evaluate_obligations_error_on_ambiguity().is_err());
}


#[test]
fn round16_selection_replays_impl_args_after_cached_evaluation() {
    let (p, t) = program("trait Marker {} impl Marker for bool {} struct Wrap<T>(T); trait Trait {} impl<T: Marker> Trait for Wrap<T> {}", "select_cached");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let ty = t.tys.intern(TyKind::Adt(named(&p, "Wrap"), t.tys.mk_list(&[t.tys.bool_]), crate::types::RegionArgsId::EMPTY));
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p, "Trait"), vec![GenericArg::Type(ty)]) };
    assert!(infcx.evaluate_root_goal(&p, goal.clone(), None).unwrap().certainty.is_yes());
    for _ in 0..2 {
        let selected = infcx.select_in_new_trait_solver(&p, goal.clone()).unwrap().unwrap();
        let select::ImplSource::UserDefined { impl_def_id, args, nested } = selected else { panic!("impl candidate expected") };
        assert_eq!(t.impl_trait[&impl_def_id], named(&p, "Trait"));
        assert_eq!(args, vec![GenericArg::Type(t.tys.bool_)]); assert!(nested.is_empty());
    }
    assert_eq!(infcx.num_open_snapshots(), 0);
}

#[test]
fn round16_selection_preserves_lifetime_and_const_impl_args() {
    let (p, t) = program("trait Trait {} struct Wrap<'a, T, const N: usize>(&'a T); impl<'a, T, const N: usize> Trait for Wrap<'a, T, N> {}", "select_args");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let lifetime = t.tys.intern_region(RegionKind::ReStatic);
    let ct = t.tys.const_usize(7);
    let ty = t.tys.intern(TyKind::Adt(named(&p, "Wrap"), t.tys.mk_list(&[t.tys.bool_, t.tys.intern(TyKind::Const(ct))]), t.tys.intern_region_args(&[lifetime])));
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p, "Trait"), vec![GenericArg::Type(ty)]) };
    let select::ImplSource::UserDefined { args, .. } = infcx.select_in_new_trait_solver(&p, goal).unwrap().unwrap() else { panic!("impl candidate expected") };
    assert_eq!(args, vec![GenericArg::Lifetime(lifetime), GenericArg::Type(t.tys.bool_), GenericArg::Const(ct)]);
}

#[test]
fn round16_selection_retains_nested_ambiguity_and_rejects_failed_candidate() {
    let (p, t) = program("trait Marker {} impl Marker for bool {} struct Wrap<T>(T); trait Trait {} impl<T: Marker> Trait for Wrap<T> {}", "select_pending");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let var = infcx.next_ty_var();
    let ty = t.tys.intern(TyKind::Adt(named(&p, "Wrap"), t.tys.mk_list(&[var]), crate::types::RegionArgsId::EMPTY));
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p, "Trait"), vec![GenericArg::Type(ty)]) };
    let select::ImplSource::UserDefined { args, nested, .. } = infcx.select_in_new_trait_solver(&p, goal.clone()).unwrap().unwrap() else { panic!("impl candidate expected") };
    assert_eq!(args.len(), 1); assert!(!nested.is_empty());
    infcx.at(&p, ParamEnvId::EMPTY).eq(var, t.tys.u32_).unwrap();
    assert!(infcx.select_in_new_trait_solver(&p, goal).is_err());
    assert_eq!(infcx.num_open_snapshots(), 0);
}

#[test]
fn round16_selection_unknown_self_is_ambiguous_and_proof_cache_invalidates() {
    let (p, mut t) = program("trait Trait {} impl Trait for bool {}", "select_invalidation");
    let tr = named(&p, "Trait");
    {
        let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
        let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(tr, vec![GenericArg::Type(infcx.next_ty_var())]) };
        assert!(infcx.select_in_new_trait_solver(&p, goal).unwrap().is_none());
    }
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(tr, vec![GenericArg::Type(t.tys.bool_)]) };
    let engine = &t.queries.engine;
    let (_, first) = engine.with_anon_task(&mut || assert!(InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default()).select_in_new_trait_solver(&p, goal.clone()).unwrap().is_some()));
    let (_, hit) = engine.with_anon_task(&mut || assert!(InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default()).select_in_new_trait_solver(&p, goal.clone()).unwrap().is_some()));
    assert_eq!(first, hit);
    t.trait_impls.clear(); t.publish_collected();
    assert!(InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default()).select_in_new_trait_solver(&p, goal).is_err());
}

#[test]
fn round16_selection_alias_self_and_global_bound_precedence() {
    let (p, t) = program("trait Iter { type Item; } impl Iter for bool { type Item = u32; } trait Marker {} impl Marker for u32 {}", "select_alias");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let assoc = named(&p, "Item"); let marker = named(&p, "Marker");
    let ty = t.alias_ty_with_args(AliasTyKind::Projection { def_id: assoc }, &[GenericArg::Type(t.tys.bool_)]);
    let bound = collect::trait_predicate(marker, vec![GenericArg::Type(t.tys.u32_)]);
    let goal = Goal { param_env: t.intern_param_env(ParamEnv { clauses: vec![bound].into() }), predicate: collect::trait_predicate(marker, vec![GenericArg::Type(ty)]) };
    assert!(matches!(infcx.select_in_new_trait_solver(&p, goal).unwrap(), Some(select::ImplSource::UserDefined { .. })));
}

#[test]
fn round16_selection_ambiguous_impls_do_not_choose_one() {
    let (p, t) = program("trait Trait<T> {} impl Trait<bool> for u32 {} impl Trait<u32> for u32 {}", "select_multi");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let var = infcx.next_ty_var();
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p, "Trait"), vec![GenericArg::Type(t.tys.u32_), GenericArg::Type(var)]) };
    assert!(infcx.select_in_new_trait_solver(&p, goal.clone()).unwrap().is_none());
    assert_eq!(infcx.shallow_resolve(var), var);
    infcx.at(&p, ParamEnvId::EMPTY).eq(var, t.tys.bool_).unwrap();
    assert!(matches!(infcx.select_in_new_trait_solver(&p, goal).unwrap(), Some(select::ImplSource::UserDefined { .. })));
}

#[test]
fn round16_codegen_selection_rejects_unconstrained_impl_parameter() {
    let (p, t) = program("trait Trait {} impl<T> Trait for bool {}", "select_unconstrained");
    let result = t.codegen_select_candidate(&p, crate::tcx::TypingEnv::fully_monomorphized(&p), TraitRef { def_id: named(&p, "Trait"), args: vec![GenericArg::Type(t.tys.bool_)] });
    assert!(matches!(result, Err(codegen::CodegenObligationError::UnconstrainedParam)));
}

#[test]
fn round16_codegen_erasure_preserves_generic_arity_and_bound_regions() {
    let (p, t) = program("trait Tr {} struct Outer<T>(T); struct Inner<'a>(&'a u32); impl<T> Tr for Outer<T> {}", "codegen_erasure");
    let region = t.tys.intern_region(RegionKind::ReStatic);
    let inner = t.tys.intern(TyKind::Adt(named(&p, "Inner"), t.tys.mk_list(&[]), t.tys.intern_region_args(&[region])));
    let bound = t.tys.intern_region(RegionKind::ReBound(0, crate::types::BoundVar(0)));
    let reference = t.tys.intern(TyKind::Ref(false, t.tys.u32_, bound));
    let signature = t.tys.intern(TyKind::FnPtr(t.tys.mk_list(&[reference]), reference, crate::types::FnHeader::RUST, 1));
    for argument in [inner, signature] {
        let self_ty = t.tys.intern(TyKind::Adt(named(&p, "Outer"), t.tys.mk_list(&[argument]), crate::types::RegionArgsId::EMPTY));
        let erased = t.tys.erase_regions(self_ty);
        let selected = t.codegen_select_candidate(&p, crate::tcx::TypingEnv::fully_monomorphized(&p), TraitRef { def_id: named(&p, "Tr"), args: vec![GenericArg::Type(erased)] }).unwrap();
        let select::ImplSource::UserDefined { args, nested, .. } = selected else { panic!("expected impl") };
        assert!(nested.is_empty());
        assert_eq!(args, vec![GenericArg::Type(t.tys.erase_regions(argument))]);
        if argument == inner {
            let GenericArg::Type(ty) = args[0] else { unreachable!() };
            let TyKind::Adt(def, compact, regions) = t.tys.kind(ty) else { unreachable!() };
            assert_eq!(t.complete_args(&p, *def, t.tys.list(*compact), *regions), vec![GenericArg::Lifetime(crate::types::RegionId::ERASED)]);
        } else {
            assert_eq!(args, vec![GenericArg::Type(signature)]);
        }
    }
}

#[test]
fn round17_codegen_instance_const_and_lifetime_arguments() {
    let (p, t) = program("trait Trait { fn call(&self); } impl<'a, const N: usize> Trait for &'a [u8; N] { fn call(&self) {} }", "instance_args");
    let trait_def = named(&p, "Trait");
    let method = p.traits[p.def(trait_def).sub as usize][0];
    let array = t.tys.intern(TyKind::Array(t.tys.u8_, t.tys.const_usize(3)));
    let region = t.tys.intern_region(RegionKind::ReVar(100));
    let receiver = t.tys.intern(TyKind::Ref(false, array, region));
    let instance = t.resolve_instance_raw(&p, crate::tcx::TypingEnv::fully_monomorphized(&p), method, &[receiver]).unwrap();
    assert_ne!(instance.def, crate::instance::InstanceKind::Item(method));
    let args = instance.compact_args(&t);
    assert_eq!(args, vec![t.tys.intern(TyKind::Const(t.tys.const_usize(3)))]);
}

#[test]
fn round17_dyn_receiver_uses_library_dispatch_impl() {
    let (p, t) = program(r#"
        #[lang = "sized"] trait Sized: MetaSized {}
        #[lang = "meta_sized"] trait MetaSized: PointeeSized {}
        #[lang = "pointee_sized"] trait PointeeSized {}
        #[lang = "unsize"] trait Unsize<T: PointeeSized>: PointeeSized {}
        #[lang = "dispatch_from_dyn"] trait DispatchFromDyn<T>: Sized {}
        impl<'a, T: PointeeSized + Unsize<U>, U: PointeeSized> DispatchFromDyn<&'a mut U> for &'a mut T {}
        trait Write { fn write(&mut self, text: &str); }
    "#, "dyn_receiver");
    assert!(t.dyn_compatibility_violations(&p, named(&p, "Write")).is_empty());
    let trait_def = named(&p, "Write");
    let object = t.tys.intern(TyKind::Dyn(trait_def, crate::types::List::EMPTY, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY, t.tys.intern_region(RegionKind::ReStatic), 0, crate::types::List::EMPTY));
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let goal = Goal { param_env: Default::default(), predicate: collect::trait_predicate(trait_def, vec![GenericArg::Type(object)]) };
    assert!(infcx.evaluate_root_goal(&p, goal, None).unwrap().certainty.is_yes());
}

#[test]
fn round17_property_query_preserves_uncertainty_and_rolls_back() {
    use evaluate_obligation::EvaluationResult::*;
    let (p, t) = program("trait Bound {} impl Bound for u32 {} trait Outer {} impl<T: Bound> Outer for [T; 2] {}", "property_query");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let variable = infcx.next_ty_var();
    let tr = |ty| TraitRef { def_id: named(&p, "Outer"), args: vec![GenericArg::Type(ty)] };
    assert_eq!(infcx.type_implements_trait(&p, ParamEnvId::EMPTY, tr(variable)), EvaluatedToAmbig);
    assert_eq!(infcx.shallow_resolve(variable), variable);
    for (element, expected) in [(t.tys.u32_, true), (t.tys.bool_, false)] {
        let array = t.tys.intern(TyKind::Array(element, t.tys.const_usize(2)));
        let result = t.type_implements_trait(&p, crate::tcx::TypingEnv::fully_monomorphized(&p), tr(array));
        assert_eq!(result.must_apply_modulo_regions(), expected);
        assert_eq!(result.may_apply(), expected);
    }
}

#[test]
fn round17_normalization_retains_constraints_for_borrowck() {
    let (p, t) = program("trait Get<'b> { type Out; } impl<'a: 'b, 'b> Get<'b> for &'a u32 { type Out = &'b u32; }", "normalization_constraints");
    let infcx = InferCtxt::new(&t, TypingMode::PostTypeckUntilBorrowck { defining_opaque_types: Default::default() }, Default::default());
    let a = infcx.next_region_var();
    let b = infcx.next_region_var();
    let input = t.tys.intern(TyKind::Ref(false, t.tys.u32_, a));
    let projection = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p, "Out") }, &[GenericArg::Type(input), GenericArg::Lifetime(b)]);
    infcx.probe(|snapshot| {
        let mut ocx = fulfill::ObligationCtxt::new(&infcx, &p);
        let result = ocx.deeply_normalize_ty(&ParamEnvId::EMPTY, projection).unwrap();
        assert_eq!(result, t.tys.intern(TyKind::Ref(false, t.tys.u32_, b)));
        assert!(infcx.region_constraints_added_in_snapshot(snapshot));
    });
}

/// Brief addendum (M2b round 3): typeck's normalized `<Ready<&'a u32> as Future>::Output`
/// (reached through IntoFuture's blanket impl at `.await`) keeps the late-bound 'a.
#[test]
fn n01b_await_output_keeps_late_param_region() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    let core_root = concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/core/lib.rs");
    prog.load_crate("core", core_root, 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    let core = prog.syms.intern("core");
    let dir = std::env::temp_dir().join(format!("relax-n01b-await-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap(); let path = dir.join("lib.rs");
    std::fs::write(&path, "async fn probe<'a>(x: core::future::Ready<&'a u32>) -> &'a u32 { x.await }").unwrap();
    prog.load_crate("awaittest", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0)]).unwrap();
    prog.prelude_crate = Some(0);
    prog.resolve_imports();
    std::fs::remove_file(&path).unwrap(); std::fs::remove_dir(&dir).unwrap();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let mut t = Tcx::new(); t.collect(&prog);
    // Round-28 D6: the typeck result keeps regions; instantiate_body is the erased codegen view.
    let probe = named(&prog, "probe");
    let b = t.typeck(&prog, probe);
    assert!(b.errors.is_empty(), "{:?}", b.errors);
    let ast = &prog.files[b.file as usize].ast;
    let mut awaits = 0;
    for i in 0..b.expr_ty.len() {
        if !matches!(ast.expr(b.expr_at(i)).kind, crate::ast::ExprKind::Await(_)) { continue; }
        awaits += 1;
        let TyKind::Ref(false, inner, r) = t.tys.kind(b.expr_ty[i]).clone() else { panic!("await output is not a shared reference") };
        assert_eq!(inner, t.tys.u32_);
        assert!(matches!(t.tys.region_kind(r), RegionKind::ReLateParam(crate::types::RegionScope::Item(d), _) if d == probe), "{:?}", t.tys.region_kind(r));
    }
    assert_eq!(awaits, 1);
}

#[test]
fn n01c_fully_perform_op_converts_normalization_outlives_for_region_inference() {
    use crate::borrowck::constraints::ConstraintCategory;
    use crate::mir::Span;
    use crate::borrowck::infer_ctxt::BorrowckInferCtxt;
    use crate::borrowck::region_infer::NllRegionVariableOrigin;
    use crate::borrowck::renumber::RegionCtxt;
    use crate::borrowck::type_check::{Locations, MirTypeckRegionConstraints, canonical::fully_perform_op_raw};
    use crate::borrowck::universal_regions::{RegionClassification, UniversalRegions};
    use crate::next_solver::type_op::{Normalize, ParamEnvAnd};
    let (p, t) = program("trait Get<'b> { type Out; } impl<'a: 'b, 'b> Get<'b> for &'a u32 { type Out = (); } fn root() {}", "n01c_conversion");
    // Borrowck inference always belongs to a body (its environment is the body's param_env query).
    let infcx = BorrowckInferCtxt::new(&t, &p, named(&p, "root"), named(&p, "root"), &[]);
    let vid = |r| match t.tys.region_kind(r) { RegionKind::ReVar(v) => v as usize, _ => unreachable!() };
    let fr_static = vid(infcx.next_nll_region_var(NllRegionVariableOrigin::FreeRegion, || RegionCtxt::Unknown));
    let a = infcx.next_nll_region_var(NllRegionVariableOrigin::FreeRegion, || RegionCtxt::Unknown);
    let b = infcx.next_nll_region_var(NllRegionVariableOrigin::FreeRegion, || RegionCtxt::Unknown);
    let mut universal_regions = UniversalRegions::new(fr_static, None,
        vec![(fr_static, RegionClassification::Global), (vid(a), RegionClassification::Local), (vid(b), RegionClassification::Local)]);
    universal_regions.indices.push((t.tys.intern_region(RegionKind::ReStatic), fr_static));
    let input = t.tys.intern(TyKind::Ref(false, t.tys.u32_, a));
    let projection = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p, "Out") }, &[GenericArg::Type(input), GenericArg::Lifetime(b)]);
    let mut constraints = MirTypeckRegionConstraints::new(0);
    let op = ParamEnvAnd { param_env: ParamEnvId::EMPTY, value: Normalize { value: projection } };
    let span = Span { file: 0, lo: 1, hi: 2 };
    let output = fully_perform_op_raw(&infcx, &universal_regions, &[], &[], &mut constraints, Locations::All(crate::mir::Span { file: 0, lo: span.lo, hi: span.hi }), span, ConstraintCategory::Internal, op).unwrap();
    assert_eq!(output, t.tys.unit);
    // The impl's `'a: 'b` reaches region inference as `'a: 'b` at every point.
    assert!(constraints.outlives_constraints.outlives.iter().any(|c|
        c.sup == vid(a) && c.sub == vid(b) && c.location.is_none() && c.category == ConstraintCategory::Internal && c.span == span),
        "{:?}", constraints.outlives_constraints.outlives);
    assert!(infcx.take_and_reset_region_constraints().constraints.is_empty());
}

// Producer-side counterparts of conform-nll5/type_check_tests.rs: real native
// inference, with the same analysis-MIR locations and constraint categories.
fn round19_program(source: &str, label: &str) -> (Program, Tcx) {
    program(&format!("{source} fn seam_test_body() {{}}"), label)
}
fn round19_body(t: &Tcx, p: &Program) -> crate::mir::Body {
    use crate::mir::*;
    let source_info = SourceInfo { span: Span { file: 7, lo: 21, hi: 29 }, ..SourceInfo::default() };
    Body { phase: MirPhase::Analysis(AnalysisPhase::Initial), source: MirSource { owner: named(p, "seam_test_body"), kind: BodyKind::Fn },
        basic_blocks: vec![BasicBlockData { statements: vec![Statement { kind: StatementKind::Nop, source_info }],
            terminator: Some(Terminator { kind: TerminatorKind::Return, source_info }), is_cleanup: false }],
        local_decls: vec![LocalDecl { ty: t.tys.unit, mutability: Mutability::Mut, source_info, user_ty: vec![], local_info: LocalInfo::Boring, kind: LocalKind::Return }],
        source_scopes: vec![SourceScopeData { span: source_info.span, parent_scope: None }], user_type_annotations: vec![],
        var_debug_info: vec![], arg_count: 0, spread_arg: None, promoted: vec![], coroutine: None }
}
fn round19_owner<'a>(p: &'a Program, t: &'a Tcx, body: &'a crate::mir::Body) -> crate::borrowck::infer_ctxt::BorrowckInferCtxt<'a> {
    use crate::borrowck::{region_infer::NllRegionVariableOrigin as Origin, renumber::RegionCtxt, universal_regions::{UniversalRegions, RegionClassification}};
    let mut cx = crate::borrowck::infer_ctxt::BorrowckInferCtxt::new(t, p, body.source.owner, body.source.owner, &[]);
    let r = cx.next_nll_region_var(Origin::FreeRegion, || RegionCtxt::Unknown);
    let RegionKind::ReVar(v) = t.tys.region_kind(r) else { unreachable!() };
    let mut universals = UniversalRegions::new(v as usize, None, vec![(v as usize, RegionClassification::Global)]);
    universals.indices.push((t.tys.intern_region(RegionKind::ReStatic), v as usize));
    cx.configure_mir(body, universals, vec![]); cx
}
#[test]
fn round19_borrowck_type_ops_normalization_location_and_taint() {
    use crate::borrowck::{type_check::{BorrowckTypeOps, Locations, MirTypeckRegionConstraints}, constraints::ConstraintCategory as Cat};
    let (p,t) = round19_program("#[lang = \"copy\"] trait Copy {} impl<'a,T> Copy for &'a T {} trait Get<'b> { type Out; } impl<'a: 'b, 'b> Get<'b> for &'a u32 { type Out = &'b u32; }", "seam_normalize");
    let body = round19_body(&t, &p); let mut cx = round19_owner(&p, &t, &body);
    let a = cx.infcx.next_region_var(); let b = cx.infcx.next_region_var();
    let input = t.tys.intern(TyKind::Ref(false, t.tys.u32_, a));
    let projection = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p,"Out") }, &[GenericArg::Type(input), GenericArg::Lifetime(b)]);
    let at = crate::mir::Location { block: crate::mir::BasicBlock(0), statement_index: 1 };
    let mut out = MirTypeckRegionConstraints::new(2);
    assert_eq!(BorrowckTypeOps::normalize(&mut cx, &mut out, projection, Locations::Single(at)), t.tys.intern(TyKind::Ref(false,t.tys.u32_,b)));
    let vid = |r| match t.tys.region_kind(r) { RegionKind::ReVar(v) => v as usize, _ => unreachable!() };
    assert!(out.outlives_constraints.outlives.iter().any(|c| c.sup == vid(a) && c.sub == vid(b) && c.location == Some(1) && c.category == Cat::Boring && c.span.lo == 21));
    assert!(!cx.tainted_by_errors.get());
    let shared_projection = t.tys.intern(TyKind::Ref(false,projection,a));
    cx.prove_trait_ref(&mut out,crate::lang_items::LangItem::Copy,&[GenericArg::Type(shared_projection)],Locations::Single(at),Cat::Predicate(crate::mir::Span::default(), named(&p, "Copy")));
    cx.prove_wf(&mut out,shared_projection,Locations::Single(at),Cat::Boring);
    assert!(!cx.tainted_by_errors.get());
    let invalid = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p,"Out") }, &[GenericArg::Type(t.tys.bool_),GenericArg::Lifetime(b)]);
    assert_eq!(BorrowckTypeOps::normalize(&mut cx, &mut out, invalid, Locations::Single(at)), invalid);
    assert!(cx.tainted_by_errors.get());
    assert!(!cx.encountered_re_error());
    assert_eq!(cx.to_region_vid(t.tys.intern_region(RegionKind::ReError)), 0);
    assert!(cx.encountered_re_error());
}
#[test]
fn round19_borrowck_type_ops_generalization_and_placeholders() {
    use crate::borrowck::{type_check::{BorrowckTypeOps, MirTypeckRegionConstraints}, region_infer::NllRegionVariableOrigin as Origin};
    use crate::types::{UniverseIndex, BoundVar, variance::V};
    let (p,t) = round19_program("", "seam_generalize"); let body = round19_body(&t, &p); let mut cx = round19_owner(&p,&t,&body);
    let source_region = cx.infcx.next_region_var();
    let source = t.tys.intern(TyKind::Ref(false,t.tys.u32_,source_region));
    let target = cx.next_ty_var(crate::mir::Span::default());
    let TyKind::Infer(vid) = *t.tys.kind(target) else { unreachable!() };
    let generalized = cx.generalize_ty_var(vid,V::Co,source,crate::mir::Span::default()).unwrap();
    assert_eq!(cx.shallow_resolve(target), generalized);
    let TyKind::Ref(false,inner,r) = *t.tys.kind(generalized) else { unreachable!() };
    assert_eq!(inner,t.tys.u32_); assert_ne!(r,source_region);
    assert!(cx.infcx.take_and_reset_region_constraints().constraints.is_empty(), "the MIR relation owns location-aware edges");
    let u = cx.create_next_universe(); assert!(u > UniverseIndex::ROOT);
    let r = BorrowckTypeOps::next_nll_region_var(&mut cx,Origin::Existential,u);
    assert_eq!(cx.infcx.inner.borrow_mut().unwrap_region_constraints().universe(&t.tys,r),u);
    let placeholder = t.tys.intern_region(RegionKind::RePlaceholder { universe:u,var:BoundVar(0) });
    let mut out = MirTypeckRegionConstraints::new(2);
    let first = out.placeholder_region(&mut cx,&t,placeholder);
    assert_eq!(first,out.placeholder_region(&mut cx,&t,placeholder));
    assert_eq!(out.placeholder_index_to_region.len(),1);
}
#[test]
fn round19_borrowck_type_ops_full_args_fields_and_clauses() {
    use crate::borrowck::type_check::{BorrowckTypeOps, ClauseOwner, Locations, MirTypeckRegionConstraints};
    use crate::mir::{PlaceTy, FieldIdx, AggregateKind, VariantIdx, Location, BasicBlock};
    let (p,t) = round19_program("struct S<'a, const N: usize> { field: &'a [u8; N] } fn take<'a: 'static, const N: usize>(x: &'a [u8; N]) {} fn bound<'a: 'b, 'b>() {}", "seam_args");
    let body = round19_body(&t, &p); let mut cx = round19_owner(&p,&t,&body); let a = cx.infcx.next_region_var();
    let args = vec![GenericArg::Lifetime(a),GenericArg::Const(t.tys.const_usize(4))];
    let sig = cx.fn_def_sig(named(&p,"take"), &[t.tys.intern(TyKind::Const(t.tys.const_usize(4)))], t.tys.intern_region_args(&[a]));
    let expected = t.tys.intern(TyKind::Ref(false,t.tys.mk_array(t.tys.u8_,t.tys.const_usize(4)),a));
    assert_eq!(sig.bound_regions,0, "signature {:?}; declaration {:?}", sig, t.resolve_bound_vars(&p, named(&p, "take"))); assert_eq!(sig.sig.inputs,vec![expected]);
    let d = named(&p,"S"); let adt = t.tys.intern(TyKind::Adt(d,t.tys.mk_list(&[t.tys.intern(TyKind::Const(t.tys.const_usize(4)))]),t.tys.intern_region_args(&[a])));
    assert_eq!(cx.field_ty(PlaceTy { ty:adt, variant_index:None },FieldIdx(0)),Some(expected));
    let mut out = MirTypeckRegionConstraints::new(2); let at = Location { block:BasicBlock(0),statement_index:0 };
    assert_eq!(cx.aggregate_field_ty(&mut out,&AggregateKind::Adt(d,VariantIdx(0),args,None,None),FieldIdx(0),at),Ok(expected));
    let b = cx.infcx.next_region_var();
    cx.normalize_and_prove_instantiated_clauses(&mut out,ClauseOwner::Def(named(&p,"bound")),&[GenericArg::Lifetime(a),GenericArg::Lifetime(b)],Locations::Single(at));
    assert!(!cx.tainted_by_errors.get());
    assert!(out.outlives_constraints.outlives.iter().any(|c| c.sup == cx.to_region_vid(a) && c.sub == cx.to_region_vid(b) && c.location == Some(0)));
}
#[test]
fn round19_borrowck_type_ops_opaque_storage_and_failure_rollback() {
    use crate::borrowck::type_check::{BorrowckTypeOps, Locations, MirTypeckRegionConstraints};
    use crate::borrowck::constraints::ConstraintCategory;
    let (p,t) = round19_program("trait Marker {} impl Marker for bool {} fn f() -> impl Marker { true }", "seam_opaque");
    let def = named(&p,"f"); let state = crate::tcx::EvalState::new();
    let ecx = crate::tcx::EvalCx::new(&p, &t,crate::tcx::TypingEnv::fully_monomorphized(&p),&state);
    let checked = crate::typeck::instantiate_body(&ecx,&p,&t,def,&[]); assert!(checked.errors.is_empty(),"{:?}",checked.errors);
    let opaque = p.defs.iter().position(|d| d.kind == crate::program::DefKind::OpaqueTy).map(|i| DefId(i as u32)).unwrap();
    let alias = t.alias_ty_with_args(AliasTyKind::Opaque { def_id:opaque },&[]);
    let body = round19_body(&t, &p); let mut cx = round19_owner(&p,&t,&body);
    cx.infcx.typing_mode = TypingMode::borrowck(&t,&p,def);
    let mut out = MirTypeckRegionConstraints::new(2); let loc = Locations::All(crate::mir::Span::default());
    cx.relate_opaque(&mut out,alias,crate::types::variance::V::Inv,t.tys.bool_,loc,ConstraintCategory::Assignment).unwrap();
    assert!(!cx.infcx.inner.borrow_mut().opaque_types().is_empty());
    assert!(cx.relate_opaque(&mut out,alias,crate::types::variance::V::Inv,t.tys.u32_,loc,ConstraintCategory::Assignment).is_err());
    let entries = cx.infcx.inner.borrow_mut().opaque_types().iter_lookup_table().to_vec();
    assert_eq!(entries.len(),1); assert_eq!(cx.infcx.deeply_resolve_ignoring_regions(entries[0].1.ty),t.tys.bool_);
    assert!(cx.tainted_by_errors.get());
}
#[test]
fn round19_borrowck_type_ops_inhabitedness_and_static_pointer_types() {
    use crate::borrowck::type_check::BorrowckTypeOps;
    let (p,t) = round19_program("enum Void {} struct Wrap { field: Void } union U { field: u32 } static S: u32 = 0; static mut M: u32 = 0; extern \"C\" { static F: u32; }", "seam_properties");
    let body = round19_body(&t, &p); let mut cx = round19_owner(&p,&t,&body);
    let ty = |name| t.tys.intern(TyKind::Adt(named(&p,name),t.tys.mk_list(&[]),crate::types::RegionArgsId::EMPTY));
    assert!(cx.is_privately_uninhabited(ty("Void"))); assert!(cx.is_privately_uninhabited(ty("Wrap"))); assert!(!cx.is_privately_uninhabited(ty("U")));
    assert!(!cx.is_privately_uninhabited(t.tys.mk_array(ty("Void"),t.tys.const_usize(0))));
    assert!(cx.is_privately_uninhabited(t.tys.mk_array(ty("Void"),t.tys.const_usize(1))));
    for (name,expected) in [("S",t.tys.intern(TyKind::Ref(false,t.tys.u32_,t.tys.intern_region(RegionKind::ReStatic)))),("M",t.tys.intern(TyKind::Ptr(true,t.tys.u32_))),("F",t.tys.intern(TyKind::Ptr(false,t.tys.u32_)))] {
        assert_eq!(cx.rvalue_query_ty(&body,&crate::mir::Rvalue::ThreadLocalRef(named(&p,name))),expected);
    }
}

#[test]
fn round19_codegen_opens_only_the_item_signature_binder() {
    let (p,t) = program("trait Get<'b> { type Out; } impl<'a: 'b, 'b> Get<'b> for &'a u32 { type Out = (); } fn f<'a, 'b>(x: &'a u32, y: <&'a u32 as Get<'b>>::Out, nested: for<'c> fn(&'c u32)) {}", "codegen_binder");
    let sig = t.fn_item_sig(&p,named(&p,"f")).unwrap();
    let env = crate::tcx::TypingEnv::fully_monomorphized(&p);
    assert_eq!(t.normalize_erasing_late_bound_regions(&p,env,sig.params[1]),t.tys.unit);
    assert_eq!(t.normalize_erasing_late_bound_regions(&p,env,sig.params[2]),sig.params[2]);
    let TyKind::FnPtr(ps,_,_,1) = t.tys.kind(sig.params[2]) else { panic!("nested fn binder") };
    let TyKind::Ref(_,_,r) = t.tys.kind(t.tys.list(*ps)[0]) else { unreachable!() };
    assert!(matches!(t.tys.region_kind(*r),RegionKind::ReBound(0,_)));
}

#[test]
fn round19_mir_renumber_keeps_native_const_and_lifetime_arguments() {
    use crate::mir::*;
    use crate::consts::{AliasConst, AliasConstKind, ConstKind, IsRigid};
    let (p,t) = round19_program("const N: usize = 4;", "seam_const_args");
    let region = t.tys.intern_region(RegionKind::ReEarlyParam(0));
    let reference = t.tys.intern(TyKind::Ref(false,t.tys.u32_,region));
    let ct = t.tys.intern_const(ConstKind::Alias(IsRigid::No,AliasConst { kind:AliasConstKind::Free { def_id:named(&p,"N") },args:vec![GenericArg::Lifetime(region),GenericArg::Type(reference)].into() }));
    let mut body = round19_body(&t, &p);
    body.basic_blocks[0].statements[0].kind = StatementKind::Assign(Place::local(Local(0)),Rvalue::Aggregate(Box::new(AggregateKind::Closure(ClosureId { file:0,expr:0 },vec![GenericArg::Lifetime(region),GenericArg::Const(ct)])),vec![]));
    let owner = named(&p,"seam_test_body");
    let cx = crate::borrowck::infer_ctxt::BorrowckInferCtxt::new(&t,&p,owner,owner,&[]);
    crate::borrowck::renumber::renumber_mir(&cx,&mut body,&mut []);
    let StatementKind::Assign(_,Rvalue::Aggregate(kind,_)) = &body.basic_blocks[0].statements[0].kind else { unreachable!() };
    let AggregateKind::Closure(_,args) = &**kind else { unreachable!() };
    let GenericArg::Lifetime(r) = args[0] else { unreachable!() };
    assert!(matches!(t.tys.region_kind(r),RegionKind::ReVar(_)));
    let GenericArg::Const(c) = args[1] else { unreachable!() };
    let ConstKind::Alias(_,alias) = t.tys.const_kind(c) else { unreachable!() };
    let GenericArg::Lifetime(r) = alias.args[0] else { unreachable!() };
    assert!(matches!(t.tys.region_kind(r),RegionKind::ReVar(_)));
    let GenericArg::Type(ty) = alias.args[1] else { unreachable!() };
    let TyKind::Ref(_,_,r) = t.tys.kind(ty) else { unreachable!() };
    assert!(matches!(t.tys.region_kind(*r),RegionKind::ReVar(_)));
}

#[test]
fn round19_self_projection_keeps_defining_supertrait_lifetimes() {
    let (p,t) = program("trait Bound {} trait Parent<'a> where Self::Out: Bound { type Out; fn get(self, x: &'a u32) -> Self::Out; } trait Child<'b>: Parent<'b> { fn other(self) -> Self::Out; }", "self_projection_regions");
    for name in ["get","other"] {
        let sig = t.fn_item_sig(&p,named(&p,name)).unwrap();
        let TyKind::Alias(_,a) = t.tys.kind(sig.ret) else { panic!("Self projection") };
        let full = t.complete_args(&p,a.kind.def_id(),t.tys.list(a.args),a.regions);
        assert_eq!(full.len(),2);
        assert!(matches!(full[1],GenericArg::Lifetime(r) if matches!(t.tys.region_kind(r),RegionKind::ReEarlyParam(1))));
    }
}

#[test]
fn round22_foreign_types_are_unsized_with_thin_pointers() {
    let (p,t) = program("extern \"C\" { type Opaque; } #[repr(C)] struct Tail { head: u32, tail: Opaque } fn input(_: *const Opaque, _: *const Tail) {}", "foreign22");
    let foreign = named(&p, "Opaque");
    assert_eq!(p.def(foreign).kind, crate::program::DefKind::ForeignTy);
    let ty = t.tys.intern(TyKind::Foreign(foreign));
    let lay = crate::layout::Layouts::new();
    let cx = crate::layout::LayoutCx::new(&p);
    assert!(lay.is_unsized(&cx, &t, ty));
    let sig = t.fn_sig(&p, named(&p,"input")).unwrap();
    for &ptr in &sig.params { assert_eq!(lay.of(&cx, &t, ptr).size, 8); }
    assert_eq!(t.adt_sizedness_constraint(&p,named(&p,"Tail"),sizedness::SizedTraitKind::Sized).unwrap(),Some(ty));
    assert_eq!(t.adt_sizedness_constraint(&p,named(&p,"Tail"),sizedness::SizedTraitKind::MetaSized).unwrap(),Some(ty));
}

#[test]
fn round23_instance_item_default_intrinsic_and_param_branches() {
    use crate::instance::InstanceKind;
    use crate::next_solver::codegen::CodegenObligationError;
    let (p, t) = program(r#"
        trait Call<A> { fn invoke<B>(&self, _: A, _: B) -> u32 { 7 } }
        impl<T> Call<T> for [T; 2] {}
        fn ordinary() {}
        extern "relax-intrinsic" { fn unreachable() -> !; }
        fn generic<T: Call<u8>>() {}
    "#, "instance_branches23");
    let env = crate::tcx::TypingEnv::fully_monomorphized(&p);
    let ordinary = named(&p, "ordinary");
    assert_eq!(t.resolve_instance_raw(&p, env, ordinary, &[]).unwrap().def, InstanceKind::Item(ordinary));
    let intrinsic = named(&p, "unreachable");
    assert_eq!(t.resolve_instance_raw(&p, env, intrinsic, &[]).unwrap().def, InstanceKind::Intrinsic(intrinsic));
    let method = named(&p, "invoke");
    let receiver = t.tys.mk_array(t.tys.u8_, t.tys.const_usize(2));
    let instance = t.resolve_instance_raw(&p, env, method, &[receiver, t.tys.u8_, t.tys.bool_]).unwrap();
    assert_eq!(instance.def, InstanceKind::Item(method));
    assert_eq!(instance.compact_args(&t), vec![receiver, t.tys.u8_, t.tys.bool_]);
    let generic = named(&p, "generic");
    let env = crate::tcx::TypingEnv { limit: Default::default(), param_env: t.intern_param_env(ParamEnv { clauses: t.clauses_of(&p, generic).into() }), mode: TypingMode::non_body_analysis() };
    let param = t.tys.intern(TyKind::Param(0));
    assert_eq!(t.resolve_instance_raw(&p, env, method, &[param, t.tys.u8_, t.tys.bool_]), Err(CodegenObligationError::Ambiguity));
}

#[test]
fn round23_instance_rejects_incompatible_impl_signature() {
    let (p, t) = program("trait Call { fn invoke(&self, x: u32) -> u32; } impl Call for u32 { fn invoke(&self, x: bool) -> u32 { 0 } }", "invalid_signature23");
    let tr = named(&p, "Call");
    let method = p.traits[p.def(tr).sub as usize][0];
    assert!(t.resolve_instance_raw(&p, crate::tcx::TypingEnv::fully_monomorphized(&p), method, &[t.tys.u32_]).is_err());
}

#[test]
fn round23_instance_rejects_incompatible_const_parameter_type() {
    let (p, t) = program("trait Call { fn invoke<const N: usize>(&self) -> u32; } impl Call for u32 { fn invoke<const N: u8>(&self) -> u32 { 0 } }", "invalid_const_parameter23");
    let tr = named(&p, "Call");
    let method = p.traits[p.def(tr).sub as usize][0];
    let arg = t.tys.intern(TyKind::Const(t.tys.const_usize(1)));
    assert!(t.resolve_instance_raw(&p, crate::tcx::TypingEnv::fully_monomorphized(&p), method, &[t.tys.u32_, arg]).is_err());
}

#[test]
fn round25_dyn_callable_impl_signatures_preserve_lifetimes() {
    // rustc_hir_analysis/src/hir_ty_lowering/dyn_trait.rs:352-419 keeps
    // the principal and Output projection binders when removing Self.
    for (label, trait_arg, impl_arg, accepted) in [
        ("dyn_hr", "&dyn for<'a> Fn(&'a u32) -> &'a u32", "&dyn for<'b> Fn(&'b u32) -> &'b u32", true),
        ("dyn_elided", "&dyn Fn(&u32) -> &u32", "&dyn Fn(&u32) -> &u32", true),
        ("dyn_outer", "&dyn Fn(&'x u32) -> &'x u32", "&dyn Fn(&'x u32) -> &'x u32", true),
        ("dyn_static", "&dyn for<'a> Fn(&'a u32) -> &'a u32", "&dyn Fn(&'static u32) -> &'static u32", true),
        ("dyn_stronger", "&dyn Fn(&'static u32) -> &'static u32", "&dyn for<'a> Fn(&'a u32) -> &'a u32", false),
    ] {
        let source = format!("#[lang = \"fn_once\"] trait FnOnce<T> {{ #[lang = \"fn_once_output\"] type Output; }} #[lang = \"fn\"] trait Fn<T>: FnOnce<T> {{}} trait Tr {{ fn apply<'x>(x: &'x u32, f: {trait_arg}); }} struct S; impl Tr for S {{ fn apply<'x>(x: &'x u32, f: {impl_arg}) {{}} }}");
        let (p, t) = program(&source, label);
        let method = *p.trait_item_def_id.keys().find(|&&d| p.name(d) == "apply").unwrap();
        assert_eq!(t.compare_impl_item(&p, method).is_ok(), accepted, "{label}");
    }
}

#[test]
fn round25_vtable_binder_erasure_retains_nested_binders() {
    use crate::types::{BoundVar, RegionId, FnHeader};
    let t = Tcx::new();
    let bound = t.tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
    let reference = t.tys.intern(TyKind::Ref(false, t.tys.u32_, bound));
    let nested = t.tys.intern(TyKind::FnPtr(t.tys.mk_list(&[reference]), reference, FnHeader::RUST, 1));
    let value = t.instantiate_bound_regions_with_erased(TraitRef {
        def_id: DefId(0), args: vec![GenericArg::Type(t.tys.u32_), GenericArg::Lifetime(bound), GenericArg::Type(reference), GenericArg::Type(nested)],
    });
    assert_eq!(value.args[1], GenericArg::Lifetime(RegionId::ERASED));
    assert_eq!(value.args[2], GenericArg::Type(t.tys.intern(TyKind::Ref(false, t.tys.u32_, RegionId::ERASED))));
    assert_eq!(value.args[3], GenericArg::Type(nested));
}

#[test]
fn round26_borrowck_const_param_env_copy() {
    // N:solve/mod.rs:280-319: a canonical const placeholder obtains its type
    // from the body's ConstArgHasType clause, including const-only generics.
    let (p, mut t) = program("#[lang = \"copy\"] trait Copy {} impl Copy for u8 {} impl<T: Copy, const N: usize> Copy for [T; N] {} fn probe<const N: usize>(x: [u8; N]) {}", "const_borrowck_env");
    t.copy_trait = Some(named(&p, "Copy"));
    let d = named(&p, "probe");
    let body = t.typeck(&p, d);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let env = crate::tcx::TypingEnv { limit: Default::default(), param_env: body.param_env.unwrap(), mode: TypingMode::borrowck(&t, &p, d) };
    let state = crate::tcx::EvalState::new();
    let tx = crate::borrowck::tyx::Tx::new(&p, &t, env, &state);
    let n = t.tys.intern_const(crate::consts::ConstKind::Param(crate::consts::ParamConst { index: 0 }));
    assert!(tx.is_copy(t.tys.mk_array(t.tys.u8_, n)));
    assert!(!tx.is_copy(t.tys.mk_array(t.tys.str_, n)));
}

#[test]
fn round26_callee_signature_uses_its_const_param_env() {
    let (p, t) = program("struct Buf<const N: usize>; trait Array { type A; } impl<const N: usize> Array for Buf<N> { type A = [u8; N]; } fn callee<const N: usize>(x: &<Buf<N> as Array>::A) {} fn caller() {}", "callee_const_env");
    let def = named(&p, "callee");
    let state = crate::tcx::EvalState::new();
    let env = crate::tcx::TypingEnv { limit: Default::default(), param_env: Default::default(), mode: crate::tcx::TypingMode::borrowck(&t, &p, named(&p, "caller")) };
    let tx = crate::borrowck::tyx::Tx::new(&p, &t, env, &state);
    let rty = crate::borrowck::rty::Rty::new(&tx);
    let sig = crate::borrowck::rty::signature::signature_template(&rty, def, &[t.tys.intern(TyKind::Const(t.tys.const_usize(3)))]);
    assert_eq!(sig.inputs.len(), 1);
}

#[test]
fn round28_identity_response_preserves_kinds_and_subroots() {
    use canonical::{CanonicalVarKind as K, response_no_constraints_raw};
    use crate::types::{UniverseIndex as U, BoundVar};
    let t = Tcx::new();
    let kinds = vec![K::Ty { ui: U(2), sub_root: BoundVar(0) }, K::Ty { ui: U(2), sub_root: BoundVar(0) }, K::Int, K::Float, K::Region(U(1)), K::Const(U(2))];
    let response = response_no_constraints_raw(&t.tys, U(2), &kinds, Certainty::AMBIGUOUS);
    assert_eq!(response.var_kinds, kinds);
    assert_eq!(response.max_universe, U(2));
    assert!(has_no_inference_or_external_constraints(&t.tys, &response));
    assert_ne!(response.value.var_values[0], response.value.var_values[1]);
}

#[test]
fn round28_at_failure_rollback_is_owned_by_caller() {
    let (p, t) = program("", "r28_at");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let var = infcx.next_ty_var();
    let a = t.tys.intern(TyKind::Tuple(t.tys.mk_list(&[var, t.tys.bool_])));
    let b = t.tys.intern(TyKind::Tuple(t.tys.mk_list(&[t.tys.u32_, t.tys.u32_])));
    assert!(infcx.at(&p, ParamEnvId::EMPTY).eq(a, b).is_err());
    assert_eq!(infcx.shallow_resolve(var), t.tys.u32_);
    let var = infcx.next_ty_var();
    let a = t.tys.intern(TyKind::Tuple(t.tys.mk_list(&[var, t.tys.bool_])));
    assert!(infcx.commit_if_ok(|_| infcx.at(&p, ParamEnvId::EMPTY).eq(a, b)).is_err());
    assert_eq!(infcx.shallow_resolve(var), var);
}

#[test]
fn round28_unused_binder_does_not_allocate_universe() {
    let (p, t) = program("trait Tr {}", "r28_binder");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let mut predicate = collect::trait_predicate(named(&p, "Tr"), vec![GenericArg::Type(t.tys.u32_)]);
    predicate.bound_vars = vec![BoundVariableKind::Region(crate::types::BoundRegionKind::Anon)].into();
    assert!(!predicate.has_bound_vars(&t.tys));
    infcx.enter_forall_predicate_and_leak_universe(&predicate);
    assert_eq!(infcx.universe(), crate::types::UniverseIndex::ROOT);
}

#[test]
fn round28_const_method_index_after_lifetime() {
    let (p, t) = program("trait Tr { fn f<const N: usize>(); } impl<'a> Tr for &'a u8 { fn f<const N: usize>() {} }", "r28_const_method");
    let method = p.defs.iter().enumerate().find(|(_, d)| p.syms.str(d.name) == "f" && p.def(d.parent).kind == crate::program::DefKind::Impl).map(|(i, _)| DefId(i as u32)).unwrap();
    assert!(t.compare_impl_item(&p, method).is_ok());
}

#[test]
fn round28_elaborates_compound_outlives() {
    let (p, t) = program("", "r28_elaborate");
    let a = t.tys.intern_region(RegionKind::ReEarlyParam(0));
    let b = t.tys.intern_region(RegionKind::ReEarlyParam(1));
    let param = t.tys.intern(TyKind::Param(0));
    let reference = t.tys.intern(TyKind::Ref(false, param, a));
    let out = t.supertraits(&p, Predicate::dummy(PredicateKind::Clause(ClauseKind::TypeOutlives(reference, b))));
    assert!(out.contains(&Predicate::dummy(PredicateKind::Clause(ClauseKind::TypeOutlives(param, b)))));
    assert!(out.contains(&Predicate::dummy(PredicateKind::Clause(ClauseKind::RegionOutlives(a, b)))));
    let bound = t.tys.intern_region(RegionKind::ReBound(0, crate::types::BoundVar(0)));
    let out = t.supertraits(&p, Predicate::dummy(PredicateKind::Clause(ClauseKind::TypeOutlives(reference, bound))));
    assert_eq!(out.len(), 1);
}

#[test]
fn round28_negative_trait_does_not_elaborate_supertraits() {
    let (p, t) = program("trait A {} trait B: A {}", "r28_negative");
    let mut pred = collect::trait_predicate(named(&p, "B"), vec![GenericArg::Type(t.tys.u32_)]);
    let PredicateKind::Clause(ClauseKind::Trait(clause)) = &mut pred.kind else { unreachable!() };
    clause.polarity = ClausePolarity::Negative;
    assert_eq!(t.supertraits(&p, pred).len(), 1);
}

#[test]
fn round28_sizedness_rejects_foreign_and_infinite_tail() {
    let (p, t) = program("#[lang = \"sized\"] trait Sized {} #[lang = \"meta_sized\"] trait MetaSized {} extern \"C\" { type Foreign; } struct Infinite { tail: Infinite }", "r28_sized");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let foreign = t.tys.intern(TyKind::Foreign(named(&p, "Foreign")));
    let infinite = t.tys.intern(TyKind::Adt(named(&p, "Infinite"), t.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
    for ty in [foreign, infinite] { for item in ["Sized", "MetaSized"] {
        let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p, item), vec![GenericArg::Type(ty)]) };
        assert!(infcx.evaluate_root_goal(&p, goal, None).is_err());
    } }
}

#[test]
fn round28_wf_normalization_keeps_original_alias() {
    let (p, t) = program("trait Tr { type Out; } impl Tr for u32 { type Out = bool; }", "r28_wf");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let alias = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p, "Out") }, &[GenericArg::Type(t.tys.u32_)]);
    let predicate = Predicate::dummy(PredicateKind::Clause(ClauseKind::WellFormed(Term::Ty(alias))));
    let (normalized, goals) = infcx.normalize_predicate(&p, &ParamEnvId::EMPTY, &predicate);
    assert_eq!(normalized, predicate);
    assert!(goals.is_empty());
}

#[test]
fn round28_builtin_metadata_and_discriminant() {
    let (p, t) = program("#[lang = \"pointee_trait\"] trait Pointee { #[lang = \"metadata_type\"] type Metadata; } #[lang = \"discriminant_kind\"] trait DiscriminantKind { #[lang = \"discriminant_type\"] type Discriminant; } #[lang = \"destruct\"] trait Destruct {} #[repr(u16)] enum E { A, B }", "r28_builtins");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let env = ParamEnvId::EMPTY;
    let e = t.tys.intern(TyKind::Adt(named(&p, "E"), t.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
    for (name, ty, expected) in [("Metadata", t.tys.u32_, t.tys.unit), ("Metadata", t.tys.intern(TyKind::Slice(t.tys.u8_)), t.tys.usize_), ("Discriminant", t.tys.bool_, t.tys.u8_), ("Discriminant", e, t.tys.prim(crate::program::Prim::U16))] {
        let alias = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p, name) }, &[GenericArg::Type(ty)]);
        assert_eq!(fulfill::ObligationCtxt::new(&infcx, &p).deeply_normalize_ty(&env, alias).unwrap(), expected);
    }
    let goal = Goal { param_env: env, predicate: collect::trait_predicate(named(&p, "Destruct"), vec![GenericArg::Type(t.tys.intern(TyKind::Slice(t.tys.u8_)))]) };
    assert!(infcx.evaluate_root_goal(&p, goal, None).unwrap().certainty.is_yes());
}

#[test]
fn round28_collected_program_dependency_tracks_revision() {
    let mut t = Tcx::new();
    let before = t.queries.read_declarations().unwrap();
    t.queries.collected(42, None);
    assert_ne!(t.queries.read_declarations().unwrap(), before);
}

#[test]
fn round28_dyn_method_self_where_clause_is_incompatible() {
    let (p, t) = program("trait Other {} trait Object { fn method(&self) where Self: Other; }", "r28_dyn_where");
    assert!(!t.dyn_compatibility_violations(&p, named(&p, "Object")).is_empty());
}

#[test]
fn round28_lifetime_only_param_env_is_not_global() {
    let (p, t) = program("trait Tr<'a> {} impl<'a> Tr<'a> for u32 {}", "r28_lifetime_env");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let lifetime = t.tys.intern_region(RegionKind::ReEarlyParam(0));
    let predicate = collect::trait_predicate(named(&p, "Tr"), vec![GenericArg::Type(t.tys.u32_), GenericArg::Lifetime(lifetime)]);
    let goal = Goal { param_env: t.intern_param_env(ParamEnv { clauses: vec![predicate.clone()].into() }), predicate };
    let tree = infcx.evaluate_root_goal_for_proof_tree(&p, goal);
    let inspect = inspect::InspectGoal::new(&infcx, &p, tree);
    assert!(inspect.candidates().iter().any(|c| matches!(c.kind, inspect::ProbeKind::TraitCandidate { source: assembly::CandidateSource::ParamEnv(false), .. })));
}

#[test]
fn round28_projection_assumption_requires_item_where_clause() {
    let (p, t) = program("trait Marker {} trait G { type A where Self: Marker; } struct X;", "r28_gat_wf");
    let infcx = InferCtxt::new(&t, TypingMode::PostAnalysis, Default::default());
    let x = t.tys.intern(TyKind::Adt(named(&p, "X"), t.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
    let alias = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p, "A") }, &[GenericArg::Type(x)]);
    let env = t.intern_param_env(ParamEnv { clauses: vec![
        collect::trait_predicate(named(&p, "G"), vec![GenericArg::Type(x)]),
        Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(alias), term: Term::Ty(t.tys.u32_) }))),
    ].into() });
    let goal = Goal { param_env: env, predicate: Predicate::dummy(PredicateKind::NormalizesTo(NormalizesTo { alias: AliasTerm::Ty(alias), term: Term::Ty(t.tys.u32_) })) };
    assert!(infcx.evaluate_root_goal(&p, goal, None).is_err());
}

#[test]
fn round28_coherence_unknowable_candidate_proves_supertraits() {
    let (mut p, t) = program("trait Impossible {} trait Foreign: Impossible {} struct X;", "r28_unknown");
    let impossible = named(&p, "Impossible");
    p.defs[impossible.0 as usize].krate = 1;
    let x = t.tys.intern(TyKind::Adt(named(&p, "X"), t.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY));
    let mut infcx = InferCtxt::new(&t, TypingMode::Coherence, Default::default());
    infcx.coherence_crate = Some(1);
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p, "Foreign"), vec![GenericArg::Type(x)]) };
    assert!(infcx.evaluate_root_goal(&p, goal, None).is_err());
}

#[test]
fn round29_param_env_query_normalizes_and_reports_once() {
    let (p, t) = program("#![no_std] trait P { type A; } impl P for u8 { type A = u32; } trait Q<T> {} fn good<T>() where T: Q<<u8 as P>::A>, <u8 as P>::A: 'static {} fn bad<T>() where T: Q<<bool as P>::A> {} fn const_only<const N: usize>() {}", "r29_env");
    let good = named(&p, "good");
    let result = t.param_env(&p, good);
    assert!(t.dcx.drain_reported().0.is_empty());
    assert!(result.clauses.iter().any(|p| matches!(&p.kind, PredicateKind::Clause(ClauseKind::Trait(tr)) if tr.trait_ref.args.contains(&GenericArg::Type(t.tys.u32_)))));
    let bcx = crate::borrowck::infer_ctxt::BorrowckInferCtxt::new(&t, &p, good, good, &[t.tys.intern(TyKind::Param(0))]);
    assert_eq!(bcx.param_env, result);
    let const_env = t.param_env(&p, named(&p, "const_only"));
    assert!(const_env.clauses.iter().any(|p| matches!(p.kind, PredicateKind::Clause(ClauseKind::ConstArgHasType(..)))));
    for i in 0..2 {
        let _bad = t.param_env(&p, named(&p, "bad"));
        let errors = t.dcx.drain_reported().0;
        assert_eq!(errors.is_empty(), i != 0);
    }
}

#[test]
fn round29_recursion_limit_belongs_to_analysis_owner() {
    let dir = std::env::temp_dir().join(format!("relax-r29-recursion-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut p = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let chain = "trait A {} trait B {} trait C {} trait D {} trait E {} impl<T: B> A for T {} impl<T: C> B for T {} impl<T: D> C for T {} impl<T: E> D for T {} impl E for u32 {}";
    for (name, limit) in [("low", 1), ("high", 64)] {
        let path = dir.join(format!("{name}.rs"));
        std::fs::write(&path, format!("#![no_std]\n#![recursion_limit = \"{limit}\"]\n{chain}\nfn {name}() {{}}" )).unwrap();
        let cfg = crate::cfg::CfgSet::host(&p.session, &[]);
        p.load_crate(name, path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(dir).unwrap();
    let mut t = Tcx::new(); t.collect(&p);
    let high = InferCtxt::for_item(&t, &p, TypingMode::non_body_analysis(), named(&p, "high"));
    let low = InferCtxt::for_item(&t, &p, TypingMode::non_body_analysis(), named(&p, "low"));
    assert_eq!(high.recursion_limit(&p), 64); assert_eq!(low.recursion_limit(&p), 1);
    let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: collect::trait_predicate(named(&p, "A"), vec![GenericArg::Type(t.tys.u32_)]) };
    assert!(high.evaluate_root_goal(&p, goal.clone(), None).unwrap().certainty.is_yes());
    assert!(low.evaluate_root_goal(&p, goal.clone(), None).unwrap().certainty.is_overflow(), "deep cached proof cannot bypass owner's depth limit");
    assert!(low.evaluate_root_goal_for_proof_tree(&p, goal.clone()).result.unwrap().value.certainty.is_overflow());
    assert!(high.evaluate_root_goal_for_proof_tree(&p, goal).result.unwrap().value.certainty.is_yes());
}

#[test]
fn round29_missing_projection_definition_is_error_or_rigid_unsized() {
    for is_unsized in [false, true] {
        let source = if is_unsized { "#![no_std] trait Tr { type Out; } impl Tr for str {}" } else { "#![no_std] trait Tr { type Out; } impl Tr for u32 {}" };
        let (p, t) = program(source, if is_unsized { "r29_missing_unsized" } else { "r29_missing_item" });
        let infcx = InferCtxt::for_item(&t, &p, TypingMode::non_body_analysis(), named(&p, "Tr"));
        let self_ty = if is_unsized { t.tys.str_ } else { t.tys.u32_ };
        let ty = t.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&p, "Out") }, &[GenericArg::Type(self_ty)]);
        let mut ocx = fulfill::ObligationCtxt::new(&infcx, &p);
        let normalized = ocx.deeply_normalize_ty(&ParamEnvId::EMPTY, ty).unwrap();
        if is_unsized {
            assert!(matches!(t.tys.kind(normalized), TyKind::Alias(crate::consts::IsRigid::Yes, _)));
            assert!(infcx.errors.drain_reported().0.is_empty());
        } else {
            assert_eq!(normalized, t.tys.error);
            assert!(infcx.tainted_by_errors());
            // Missing required items are diagnosed by impl well-formedness.
            infcx.errors.report(infcx.cause, "missing required associated item".into());
            assert_eq!(infcx.errors.drain_reported().0.len(), 1);
        }
    }
}

#[test]
fn round30_typeck_children_share_root_param_env() {
    let (p, t) = program("trait Marker {} fn owner<T: Marker>(x: T) { let _ = || { let _ = || {}; }; let _ = const { 1u8 }; }", "child_env");
    let owner = named(&p, "owner");
    let root = t.param_env(&p, owner);
    assert!(!root.clauses.is_empty());
    let mut children = 0;
    for (&def, &(_, node)) in &p.def_id_to_node_id {
        if matches!(node, relax_front::def_collector::NodeId::Closure(_) | relax_front::def_collector::NodeId::InlineConst(_)) {
            assert_eq!(t.param_env(&p, def), root);
            children += 1;
        }
    }
    assert_eq!(children, 3);
}

#[test]
fn round30_struct_wf_uses_final_path_arguments() {
    let (p, t) = program("trait Bound {} impl Bound for u8 {} struct Wrap<T: Bound> { value: T } type Alias<T> = Wrap<T>; fn explicit<T: Bound>(value: T) -> Wrap<T> { Wrap::<T> { value } } fn alias<T: Bound>(value: T) -> Alias<T> { Alias::<T> { value } } fn inferred<T: Bound>(value: T) -> Wrap<T> { Wrap { value } } fn bad() { let _ = Wrap::<bool> { value: true }; }", "struct_wf");
    for name in ["explicit", "alias", "inferred"] {
        let body = t.typeck(&p, named(&p, name));
        assert!(body.errors.is_empty(), "{name}: {:?}", body.errors);
    }
    let body = t.typeck(&p, named(&p, "bad"));
    assert!(!body.errors.is_empty(), "the final path must still prove its declared bounds");
}

#[test]
fn round31_dyn_receiver_reports_missing_meta_sized_to_root() {
    let (p, t) = program(r#"
        #[lang = "unsize"] trait Unsize<T> {}
        #[lang = "dispatch_from_dyn"] trait DispatchFromDyn<T> {}
        trait Object { fn call(&self); }
    "#, "missing_meta_sized");
    let errors = t.dcx.clone();
    assert!(!t.dyn_compatibility_violations(&p, named(&p, "Object")).is_empty());
    assert!(errors.has_errors());
    let (reported, _) = errors.drain_reported();
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].cause.body_id, named(&p, "call"));
    assert!(reported[0].message.contains("meta_sized"));
}

#[test]
fn round32_invalid_upcast_delays_bug_until_session_end() {
    let (prog, tcx) = program("trait Source {} trait Target {}", "invalid_upcast_root");
    let dyn_ty = |name| tcx.tys.intern(TyKind::Dyn(named(&prog, name), crate::types::List::EMPTY, crate::types::List::EMPTY,
        crate::types::RegionArgsId::EMPTY, crate::types::RegionId::ERASED, 0, crate::types::List::EMPTY));
    for report_error in [false, true] {
        let errors = tcx.dcx.clone();
        assert_eq!(tcx.supertrait_vtable_slot(&prog, dyn_ty("Source"), dyn_ty("Target")), None);
        if report_error { errors.report(Default::default(), "invalid upcast".into()); }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| errors.flush_delayed()));
        assert_eq!(result.is_ok(), report_error);
    }
}

#[test]
fn round32_solver_cache_taints_roots_and_flushes_delayed_once() {
    let (prog, tcx) = program("trait Tr { type Out; } impl Tr for u8 {} fn first() {} fn second() {}", "session_solver_diagnostic");
    let alias = tcx.alias_ty_with_args(AliasTyKind::Projection { def_id: named(&prog, "Out") }, &[GenericArg::Type(tcx.tys.u8_)]);
    for owner in ["first", "second"] {
        let infcx = InferCtxt::for_item(&tcx, &prog, TypingMode::PostAnalysis, named(&prog, owner));
        assert!(!infcx.tainted_by_errors());
        let mut ocx = fulfill::ObligationCtxt::new(&infcx, &prog);
        assert_eq!(ocx.deeply_normalize_ty(&ParamEnvId::EMPTY, alias).unwrap(), tcx.tys.error);
        assert!(infcx.tainted_by_errors(), "{owner}: cached error type must taint its consumer");
        assert_eq!(tcx.dcx.err_count(), 0);
    }
    for _ in 0..2 {
        assert_eq!(tcx.try_normalize_erasing_regions(&prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), alias).unwrap(), tcx.tys.error);
    }
    // One delayed bug for the whole session (deduplicated), flushed once at its end.
    let flushed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tcx.dcx.flush_delayed())).unwrap_err();
    let message = flushed.downcast_ref::<String>().unwrap();
    assert_eq!(message.matches("missing associated item definition").count(), 1, "{message}");
    tcx.dcx.flush_delayed();
}

#[test]
fn round33_taint_follows_the_contexts_own_handle() {
    // rustc_infer/src/infer/mod.rs:724,1088 + rustc_errors/src/lib.rs:566-571,1362-1371: an
    // InferCtxt is tainted by errors emitted through its own handle (also deduplicated ones)
    // and by error types it consumes -- not by errors other roots or threads emit meanwhile.
    let (prog, tcx) = program("fn first() {} fn second() {}", "session_taint_handle");
    let first = InferCtxt::for_item(&tcx, &prog, TypingMode::PostAnalysis, named(&prog, "first"));
    let second = InferCtxt::for_item(&tcx, &prog, TypingMode::PostAnalysis, named(&prog, "second"));
    std::thread::scope(|s| { s.spawn(|| tcx.dcx.report(Default::default(), "another body's error".into())); });
    tcx.dcx.report(first.cause, "session-level error".into());
    assert!(!first.tainted_by_errors() && !second.tainted_by_errors());
    first.errors.report(first.cause, "first error".into());
    assert!(first.tainted_by_errors() && !second.tainted_by_errors());
    second.errors.report(first.cause, "first error".into());
    assert!(second.tainted_by_errors(), "a deduplicated emission still taints its emitter");
    assert_eq!(tcx.dcx.err_count(), 3);
    let third = InferCtxt::for_item(&tcx, &prog, TypingMode::PostAnalysis, named(&prog, "second"));
    assert!(!third.tainted_by_errors());
    third.deeply_resolve_ignoring_regions(tcx.tys.error);
    assert!(third.tainted_by_errors());
    tcx.dcx.delayed_bug(third.cause, "recovery after earlier session error".into());
    tcx.dcx.flush_delayed();
}

#[test]
fn round36_inherited_projection_bounds_keep_parent_lifetimes() {
    // collect/clauses_of.rs:911-925 lowers inherited bounds in the parent ItemCtxt.
    let (prog, tcx) = program("trait Cursor<'a> { type RefMut; } struct Pair<A, B>(A, B); trait Output<'a> { type Out; } impl<'a, A: Cursor<'a>, B> Output<'a> for Pair<A, B> where B: Cursor<'a> { type Out = (A::RefMut, B::RefMut); }", "parent_projection_lifetimes");
    let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "Out" && prog.def(d.parent).kind == crate::program::DefKind::Impl).map(|(i, _)| DefId(i as u32)).unwrap();
    let out = tcx.type_of(&prog, def).unwrap();
    let TyKind::Tuple(parts) = tcx.tys.kind(out) else { panic!("tuple projection") };
    assert_eq!(tcx.tys.list(*parts).len(), 2);
    for &part in tcx.tys.list(*parts) {
        let TyKind::Alias(_, alias) = tcx.tys.kind(part) else { panic!("projection") };
        let regions = tcx.tys.region_args(alias.regions);
        assert_eq!(regions.len(), 1);
        assert_eq!(tcx.tys.region_kind(regions[0]), RegionKind::ReEarlyParam(0));
    }
}

#[test]
fn round37_type_length_counts_shared_subtrees_and_const_arguments() {
    let t = Tcx::new();
    let mut ty = t.tys.u32_;
    for _ in 0..6 { ty = t.tys.intern(TyKind::Tuple(t.tys.mk_list(&[ty, ty]))); }
    assert_eq!(crate::instance::type_length(&t, &[ty]), 127);
    assert_eq!(crate::instance::type_length(&t, &[ty, ty]), 254);
    let ct = t.tys.const_usize(4);
    assert_eq!(crate::instance::type_length(&t, &[t.tys.intern(TyKind::Const(ct))]), 2);
}

#[test]
fn round37_excessive_ambiguous_instance_emits_fatal_diagnostic() {
    let (p, t) = program("#![type_length_limit=\"0\"] trait Trait { fn method(&self); } fn generic<T: Trait>() {}", "type_length");
    let param = t.tys.intern(TyKind::Param(0));
    let env = crate::tcx::TypingEnv { limit: Default::default(), param_env: t.intern_param_env(ParamEnv { clauses: t.clauses_of(&p, named(&p, "generic")).into() }), mode: TypingMode::PostAnalysis };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::instance::Instance::expect_resolve(&t, &p, env, named(&p, "method"), &[param])));
    assert!(result.unwrap_err().is::<crate::infer::diagnostics::FatalError>());
    let (errors, _) = t.dcx.drain_reported();
    assert!(errors.iter().any(|e| e.message.contains("type-length limit")), "{errors:?}");
}

#[test]
fn round37_method_query_retains_parameters_and_snapshots_assignments() {
    use canonical::method_response::*;
    let (prog, tcx) = program("fn owner<T>() {}", "method_query");
    let infcx = InferCtxt::for_item(&tcx, &prog, TypingMode::non_body_analysis(), named(&prog, "owner"));
    let var = infcx.next_ty_var();
    let param = tcx.tys.intern(TyKind::Param(0));
    let error_region = tcx.tys.intern_region(RegionKind::ReError);
    let error_ref = tcx.tys.intern(TyKind::Ref(false, param, error_region));
    let (err_goal, _) = canonicalize_method_autoderef_goal(&infcx, ParamEnvId::EMPTY, error_ref);
    assert_eq!(err_goal.value.self_ty, error_ref);
    let receiver = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[param, var])));
    let (goal, original) = canonicalize_method_autoderef_goal(&infcx, ParamEnvId::EMPTY, receiver);
    let query = InferCtxt::new(&tcx, TypingMode::non_body_analysis(), Default::default());
    let (goal, values) = instantiate_method_autoderef_goal(&query, &goal);
    let TyKind::Tuple(parts) = tcx.tys.kind(goal.self_ty) else { panic!() };
    let parts = tcx.tys.list(*parts);
    assert_eq!(parts[0], param);
    let previous = query.inner.borrow_mut().opaque_types().num_entries();
    let before = make_query_response_ignoring_pending_obligations(&query, &values, goal.self_ty, previous);
    query.at(&prog, ParamEnvId::EMPTY).eq(parts[1], tcx.tys.u32_).unwrap();
    let after = make_query_response_ignoring_pending_obligations(&query, &values, goal.self_ty, previous);
    let (ty, goals) = probe_instantiate_query_response(&infcx, &prog, &ParamEnvId::EMPTY, &original, &before).unwrap();
    assert!(goals.is_empty());
    assert!(tcx.tys.has_infer(infcx.deeply_resolve_ignoring_regions(ty)));
    assert!(tcx.tys.has_infer(infcx.shallow_resolve(var)));
    let (ty, _) = probe_instantiate_query_response(&infcx, &prog, &ParamEnvId::EMPTY, &original, &after).unwrap();
    assert_eq!(infcx.deeply_resolve_ignoring_regions(ty), tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[param, tcx.tys.u32_]))));
}

/// Round 41: preemption must leave a definition query available for a later check.
/// rustc_query_impl/src/execution.rs:149-159 poisons unwound jobs; resuming the
/// same session after Preempted needs a lead-designed cancellation contract.
#[test]
fn round41_preempted_typeck_can_be_checked_later() {
    let (prog, tcx) = program("fn target() { let x = 1u32; }", "preempt_retry");
    let def = named(&prog, "target");
    let writer = crate::preempt::WriterWaiting::new();
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _on = crate::preempt::Preemptible::on();
        tcx.typeck(&prog, def)
    }));
    assert!(first.is_err_and(|p| crate::preempt::is_preempted(&*p)));
    drop(writer);
    let body = tcx.typeck(&prog, def);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
}

#[test]
fn round42_borrowck_forwards_preemption_after_typeck() {
    let (prog, mut tcx) = program("#[lang = \"copy\"] trait Copy {} fn target<T>(x: T) { let y = x; }", "borrowck_preempt");
    tcx.copy_trait = Some(named(&prog, "Copy"));
    let def = named(&prog, "target");
    let body = tcx.typeck(&prog, def);
    assert!(body.errors.is_empty());
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
    let writer = crate::preempt::WriterWaiting::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _p = crate::preempt::Preemptible::on();
        crate::borrowck::check_body(&cx, &prog, &tcx, &body)
    }));
    assert!(result.is_err_and(|p| crate::preempt::is_preempted(&*p)));
    drop(writer);
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
    let verdict = crate::borrowck::check_body(&cx, &prog, &tcx, &body);
    assert!(verdict.errors.is_empty());
    assert!(verdict.unchecked.is_empty(), "{:?}", verdict.unchecked);
}

#[test]
fn round50_resolve_weak_candidate_pairing() {
    // Nightly rustc_next_trait_solver/src/solve/eval_ctxt/probe.rs:33-49
    // @8d1a764 pairs every candidate through the probe. Exercise the actual
    // unicode-bidi body and its inferred Iterator/Index/
    // Clone/TextSource obligations. Test assertions stay enabled in release.
    let session = std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(&session);
    let std = concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std");
    let mut deps = Vec::new();
    for name in ["core", "alloc"] {
        let file = format!("{std}/{name}/lib.rs");
        let krate = prog.load_crate(name, &file, 2021, crate::cfg::CfgSet::host(&session, &[]), deps.clone()).unwrap();
        deps.push((prog.syms.intern(name), krate));
    }
    let file = std::env::var("MAKEPAD_RUN_BIDI_SOURCE").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../libs/unicode/unicode-bidi/src/lib.rs").into());
    prog.load_crate("unicode_bidi", &file, 2018, crate::cfg::CfgSet::host(&session, &[]), deps).unwrap();
    prog.prelude_crate = Some(0);
    prog.resolve_imports();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    let owner = named(&prog, "resolve_weak");
    search_graph::CANDIDATE_PAIRS.with(|pairs| pairs.set((0, 0)));
    let body = tcx.typeck(&prog, owner);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let (entered, finished) = search_graph::CANDIDATE_PAIRS.with(|pairs| pairs.get());
    assert!(entered > 0, "the native solver must evaluate candidates");
    assert_eq!(entered, finished, "unpaired native solver candidates");
    eprintln!("resolve_weak: {entered} candidate entries, {finished} finishes");
}

#[test]
fn drop_glue_of_a_type_parameter_is_too_generic_not_unmet() {
    // rustc_ty_utils instance.rs: a type that may need drop but is no ADT/tuple/closure/... (a
    // parameter, a projection) resolves to Ok(None) -- too generic -- not an unmet obligation
    // (borrowck's resolve_trait_method turned Unimplemented into a delayed bug: the
    // `resolving core::ptr::drop_in_place failed` ICE on every generic drop_in_place::<T>)
    use crate::next_solver::codegen::CodegenObligationError;
    let (p, t) = program(r#"#[lang = "drop_glue"] fn drop_in_place<T>(_: *mut T) {} fn generic<T>() {}"#, "dropglue_param");
    let glue = named(&p, "drop_in_place");
    assert_eq!(t.lang_items.get(crate::lang_items::LangItem::DropGlue), Some(glue));
    let generic = named(&p, "generic");
    let env = crate::tcx::TypingEnv { limit: Default::default(), param_env: t.intern_param_env(ParamEnv { clauses: t.clauses_of(&p, generic).into() }), mode: TypingMode::non_body_analysis() };
    let param = t.tys.intern(TyKind::Param(0));
    assert_eq!(t.resolve_instance_raw(&p, env, glue, &[param]), Err(CodegenObligationError::Ambiguity));
    // a type without drop glue still resolves (the trivial glue)
    assert!(t.resolve_instance_raw(&p, crate::tcx::TypingEnv::fully_monomorphized(&p), glue, &[t.tys.u8_]).is_ok());
}
