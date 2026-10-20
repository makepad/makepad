use super::*;
use crate::types::{BoundVar, RegionKind};

// Diagnostic for the merged selection/callable migration. A callable predicate
// must retain its binder and its relation must not certify a placeholder leak.
// rustc_trait_selection/src/traits/select/{mod.rs:545,confirmation.rs:602}.
#[test]
fn arch_t2b1_callable_general_pointer_is_not_static_pointer() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    let bound = tcx.tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
    let static_ = tcx.tys.intern_region(RegionKind::ReStatic);
    let a = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, bound));
    let b = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, static_));
    let general = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[a]), tcx.tys.unit, crate::types::FnHeader::RUST, 1));
    let specific = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[b]), tcx.tys.unit, crate::types::FnHeader::RUST, 0));
    assert_ne!(general, specific);
    assert!(!fcx.eq(general, specific) || fcx.infcx.leak_check(crate::types::UniverseIndex::ROOT, None).is_err(), "higher-ranked equality must fail the leak check");
}

#[test]
fn arch_t2b1_callable_placeholder_relation_is_not_proof() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    let universe = fcx.infcx.create_next_universe();
    let placeholder = tcx.tys.intern_region(RegionKind::RePlaceholder { universe, var: BoundVar(0) });
    let static_ = tcx.tys.intern_region(RegionKind::ReStatic);
    let a = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, placeholder));
    let b = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, static_));
    let a = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[a])));
    let b = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[b])));
    let mark = fcx.start_snapshot();
    let accepted = fcx.eq(a, b);
    let recorded_effects = fcx.infcx.region_constraints_added_in_snapshot(&mark.inference);
    fcx.rollback_to(mark);
    assert!(!accepted || recorded_effects,
        "Fn Args relation returns success without recording the placeholder = static constraint");
}

#[test]
fn arch_t2b1_callable_same_binder_control() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    let bound = tcx.tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
    let a = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u32_, bound));
    let general = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[a]), tcx.tys.unit, crate::types::FnHeader::RUST, 1));
    assert!(fcx.eq(general, general));
}

#[test]
fn arch_t2b1_callable_rustc_oracle() {
    let root = std::env::temp_dir().join(format!("arch-t2b1-callable-oracle-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let source = root.join("probe.rs");
    let metadata = root.join("probe.rmeta");
    for (input, accepted) in [("for<'a> fn(&'a u32)", true), ("fn(&'static u32)", false)] {
        let text = format!("fn require<F: for<'a> Fn(&'a u32)>(_: F) {{}}\npub fn probe(f: {input}) {{ require(f); }}\n");
        std::fs::write(&source, text).unwrap();
        let result = std::process::Command::new("rustc")
            .args(["--edition=2021", "--crate-type=lib", "--emit=metadata"])
            .arg(&source).arg("-o").arg(&metadata).output().unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.success(), accepted, "{input}: {stderr}");
        if !accepted { assert!(stderr.contains("not general enough"), "{stderr}"); }
    }
    std::fs::remove_file(source).unwrap();
    if metadata.exists() { std::fs::remove_file(metadata).unwrap(); }
    std::fs::remove_dir(root).unwrap();
}

fn context<'a>(prog: &'a Program, tcx: &'a Tcx, state: &'a crate::tcx::EvalState) -> Fcx<'a> {
    let infcx = crate::infer::InferCtxt::new(tcx, crate::tcx::TypingMode::PostAnalysis, Default::default());
    Fcx {
        cx: crate::tcx::EvalCx::new(prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), state),
        prog,
        tcx,
        file: 0,
        module: 0,
        env: GenEnv::default(),
        b: Body::empty(NO_DEF, Vec::new(), 0, tcx.tys.unit),
        scopes: Vec::new(),
        local_hyg: Vec::new(),
        fulfillment_cx: crate::next_solver::fulfill::FulfillmentCtxt::new(&infcx),
        suspended_fulfillment: Vec::new(),
        infcx,
        param_env: Default::default(),
        loops: Vec::new(),
        in_loop_condition: false,
        diverges: false,
        ret_stack: Vec::new(),
        closure_depth: Vec::new(),
        in_const: false,
        inline_const_floor: Vec::new(),
        in_anon_const: false,
        async_depth: 0,
        await_count: 0,
        cur_binop: 0,
        keep_literal_vars: true,
        keep_user_types: true,
        diverging: Vec::new(),
        deferred_dyn: Vec::new(),
        deferred_casts: Vec::new(),
        cast_checks: Vec::new(),
        pat_nest: 0,
        bind_mode: 0,
        pat_depth: 0,
        pat_path_pre: None,
        pending_captures: Vec::new(),
        deferred_call_resolutions: Vec::new(),
        deferred_sized_obligations: Vec::new(),
        pat_decl_irrefutable: false,
        item_blocks: Vec::new(),
        checked_opaque_types_storage_entries: None,
        expected_closure_kinds: HashMap::default(),
        async_expected_inner: None,
        async_body_expected: HashMap::default(),
        extra_arg: None,
        soft: false,
        last_lo: 0,
        known_method_args: None,
        value_path_lifetimes: Vec::new(),
        value_path_enum_lifetimes: Vec::new(),
        value_path_parent_regions: Vec::new(),
        deferred_repeats: Vec::new(),
        stalled_coroutine_goals: Vec::new(),
        forced_autoref: None,
        method_expected: None,
        arm_attrs: Vec::new(),
    }
}

pub(super) fn program(source: &str) -> (Program, Tcx) {
    let dir = std::env::temp_dir().join(format!("relax-native-fcx-{}-{}", std::process::id(), crate::unique_test_suffix()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("lib.rs"); std::fs::write(&path, source).unwrap();
    let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let cfg = crate::cfg::CfgSet::host(&prog.session, &[]);
    prog.load_crate("native_fcx", path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
    // the checks that report only in a checked crate (casts, ...) run in this crate
    for c in prog.crates.iter_mut() { c.validate = true; }
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap(); (prog, tcx)
}

#[test]
fn native_fcx_const_inference_and_writeback() {
    let (prog, tcx) = program("#![no_std]\nfn id<const N: usize>(x: [u8; N]) -> [u8; N] { x } fn probe() -> [u8; 3] { let a: [u8; _] = id::<_>([1u8; _]); a } fn pattern() { let a = [1u8; _]; let [x, y, z] = a; } struct Buf<const N: usize> { data: [u8; N] } impl<const N: usize> Buf<N> { fn new() -> Self { Self { data: [0; N] } } } fn inferred() -> Buf<3> { Buf::new() } type Arr<const N: usize> = [u8; N]; fn alias() -> Arr<3> { let a: Arr<_> = [1u8; 3]; a } type Buffer<const N: usize> = Buf<N>; fn alias_ctor() -> Buf<3> { Buffer { data: [0; 3] } } const YES: bool = true; struct Flag<const B: bool>; fn named() -> Flag<YES> { Flag::<YES> }");
    for name in ["probe", "pattern", "inferred", "alias", "alias_ctor", "named"] {
        let def = prog.defs.iter().enumerate().find(|(_,d)| prog.syms.str(d.name) == name).map(|(i,_)| DefId(i as u32)).unwrap();
        let body = (*tcx.typeck(&prog, def)).clone();
        assert!(body.errors.is_empty(), "{name}: {:?}", body.errors);
        for ty in body.expr_ty { assert!(!tcx.tys.has_infer(ty)); }
    }
}

#[test]
fn native_fcx_rollback_discards_allocations_and_local_obligations() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new(); let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    let outer = fcx.fresh(0);
    let snapshot = fcx.start_snapshot();
    let local = fcx.fresh(0);
    let ct = fcx.infcx.next_const_var(); let _ = fcx.fresh_region();
    assert!(fcx.eq(outer, tcx.tys.intern(TyKind::Array(local, ct))));
    fcx.register_goal(Goal { param_env: Default::default(), predicate: Predicate::dummy(PredicateKind::Ambiguous) }, 0);
    fcx.rollback_to(snapshot);
    assert_eq!(fcx.infcx.shallow_resolve(outer), outer);
    assert!(!fcx.fulfillment_cx.has_pending_obligations());
    let mut inner = fcx.infcx.inner.borrow_mut();
    assert_eq!(inner.type_variables().num_vars(), 1);
    assert_eq!(inner.const_unification_table().len(), 0);
    assert_eq!(inner.unwrap_region_constraints().num_region_vars(), 0);
}

#[test]
fn native_fcx_normalization_preserves_body_lifetime() {
    let (prog, tcx) = program("#![no_std]\ntrait Future { type Output; } struct Ready<T>(T); impl<T> Future for Ready<T> { type Output = T; } fn probe<'a>(_: Ready<&'a u32>) -> &'a u32 { loop {} }");
    let mut def = NO_DEF; let mut output = NO_DEF;
    for (i, d) in prog.defs.iter().enumerate() {
        match prog.syms.str(d.name) { "probe" => def = DefId(i as u32), "Output" if d.kind == DefKind::AssocTy && prog.def(d.parent).kind == DefKind::Trait => output = DefId(i as u32), _ => {} }
    }
    let state = crate::tcx::EvalState::new(); let mut fcx = context(&prog, &tcx, &state);
    fcx.b.def = def;
    let (params, ret) = fcx.body_signature(tcx.fn_sig(&prog, def).unwrap(), &[]);
    let alias = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: output }, args: tcx.tys.mk_list(&[params[0]]), regions: crate::types::RegionArgsId::EMPTY }));
    let ty = fcx.normalize_body_type(alias); fcx.retry_goals(true);
    let ty = fcx.infcx.deeply_resolve_ignoring_regions(ty);
    assert_eq!(ty, ret);
    let TyKind::Ref(_, _, region) = tcx.tys.kind(ty) else { panic!("expected reference output") };
    assert!(matches!(tcx.tys.region_kind(*region), RegionKind::ReLateParam(_, _)));
    assert!(fcx.b.errors.is_empty(), "{:?}", fcx.b.errors);
}

#[test]
fn round18_native_opaque_definition_and_rigid_use() {
    let (prog, tcx) = program("#![no_std]\ntrait Value { fn value(&self) -> u32; } impl Value for u32 { fn value(&self) -> u32 { *self } } fn produce() -> impl Value { 42u32 } fn use_it() -> u32 { produce().value() } fn generic<T: Value>(x: T) -> impl Value { x }");
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
    let named = |name: &str| prog.defs.iter().enumerate().find(|(_,d)| prog.syms.str(d.name) == name).map(|(i,_)| DefId(i as u32)).unwrap();
    let owner = named("produce");
    let defs = tcx.defining_opaques(tcx.opaque_types_defined_by(&prog, owner));
    assert_eq!(defs.len(), 1); assert_eq!(prog.def(defs[0]).kind, DefKind::OpaqueTy);
    let body = tcx.typeck(&prog, owner);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    assert_eq!(body.hidden_types[&defs[0]], tcx.tys.u32_);
    assert_eq!(tcx.type_of(&prog, defs[0]), Ok(tcx.tys.u32_));
    let body = tcx.typeck(&prog, named("use_it"));
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    assert!(body.expr_ty.iter().any(|t| matches!(tcx.tys.kind(*t), TyKind::Alias(crate::consts::IsRigid::Yes, _))));
    let body = instantiate_body(&cx, &prog, &tcx, named("use_it"), &[]);
    assert!(!body.expr_ty.iter().any(|t| tcx.tys.has_opaque(*t)));
    let generic = named("generic");
    let defs = tcx.defining_opaques(tcx.opaque_types_defined_by(&prog, generic));
    let ty = tcx.type_of(&prog, defs[0]).unwrap();
    assert!(matches!(tcx.tys.kind(ty), TyKind::Param(0)));
}

#[test]
fn round28_codegen_and_borrowck_consumer_boundaries() {
    let (prog, tcx) = program("#![no_std] trait Value {} impl Value for u8 {} fn opaque() -> impl Value { 1u8 } fn mono<'a>(x: &'a u8) -> &'a u8 { x } fn generic<'a, T>(x: &'a T) -> &'a T { x }");
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
    for name in ["mono", "generic", "opaque"] {
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == name).map(|(i, _)| DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(body.errors.is_empty(), "{name}: {:?}", body.errors);
        let inst = instantiate_body(&cx, &prog, &tcx, def, &body.args);
        assert!(inst.errors.is_empty(), "{name}: {:?}", inst.errors);
        if name == "opaque" {
            assert!(tcx.tys.has_opaque(tcx.fn_sig(&prog, def).unwrap().ret));
            assert!(!body.hidden_types.is_empty());
            assert_eq!(body.ret, tcx.tys.u8_);
            assert_eq!(inst.ret, tcx.tys.prim(crate::program::Prim::U8));
        } else {
            let TyKind::Ref(_, _, kept) = tcx.tys.kind(body.ret) else { panic!("reference return") };
            let TyKind::Ref(_, _, erased) = tcx.tys.kind(inst.ret) else { panic!("reference instance return") };
            assert_ne!(*kept, crate::types::RegionId::ERASED);
            assert_eq!(*erased, crate::types::RegionId::ERASED);
        }
        assert!(std::sync::Arc::ptr_eq(&body, &tcx.typeck(&prog, def)));
        assert!(inst.param_env.is_none());
    }
}

#[test]
fn round28_constructor_and_literal_predicates_match_rustc() {
    for expr in ["W(false)", "S { value: false }"] {
        let source = format!("#![no_std] trait Bound {{}} impl Bound for u8 {{}} struct W<T: Bound>(T); struct S<T: Bound> {{ value: T }} fn probe() {{ let _ = {expr}; }}");
        let (prog, tcx) = program(&source);
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "probe").map(|(i, _)| DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(!body.errors.is_empty(), "{expr} must register its declaration predicates");
        let root = std::env::temp_dir().join(format!("round28-w2-oracle-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("probe.rs");
        std::fs::write(&file, &source).unwrap();
        let result = std::process::Command::new("rustc").args(["--edition=2021", "--crate-type=lib", "--emit=metadata"]).arg(&file).arg("-o").arg(root.join("probe.rmeta")).output().unwrap();
        assert!(!result.status.success(), "rustc must reject {expr}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("E0277"));
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}

#[test]
fn round28_closure_pointer_kind_and_async_expectation() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new(); let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    let pointer = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[]), tcx.tys.unit, FnHeader::RUST, 0));
    assert_eq!(fcx.deduce_closure_signature(pointer, false), (Some(pointer), Some(crate::upvar::ClosureKind::Fn)));
    assert_eq!(fcx.deduce_closure_signature(pointer, true), (None, None));
}

#[test]
fn round28_snapshot_commit_publishes_obligations() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new(); let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    let snapshot = fcx.start_snapshot();
    fcx.register_goal(Goal { param_env: Default::default(), predicate: Predicate::dummy(PredicateKind::Ambiguous) }, 0);
    fcx.commit_from(snapshot);
    assert!(fcx.fulfillment_cx.has_pending_obligations());
}

#[test]
fn round28_cast_fallback_matches_rustc() {
    for (body, accepted) in [("let x = 0; let _ = &x as *const u8;", false), ("let x = [0u8]; let _ = &x as *const u8;", true)] {
        let source = format!("#![no_std] fn probe() {{ {body} }}");
        let (prog, tcx) = program(&source);
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "probe").map(|(i, _)| DefId(i as u32)).unwrap();
        let result = tcx.typeck(&prog, def);
        assert_eq!(result.errors.is_empty(), accepted, "{:?}", result.errors);
        let root = std::env::temp_dir().join(format!("round28-w2-cast-oracle-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("probe.rs"); let metadata = root.join("probe.rmeta");
        std::fs::write(&file, source).unwrap();
        let result = std::process::Command::new("rustc").args(["--edition=2021", "--crate-type=lib", "--emit=metadata"]).arg(&file).arg("-o").arg(&metadata).output().unwrap();
        assert_eq!(result.status.success(), accepted, "{}", String::from_utf8_lossy(&result.stderr));
        std::fs::remove_file(file).unwrap();
        if metadata.exists() { std::fs::remove_file(metadata).unwrap(); }
        std::fs::remove_dir(root).unwrap();
    }
}

#[test]
fn round28_default_trait_opaque_environment() {
    let (prog, tcx) = program("#![no_std] trait Value {} impl Value for u8 {} trait DefaultBody { fn produce(&self) -> impl Value { 1u8 } }");
    let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "produce").map(|(i, _)| DefId(i as u32)).unwrap();
    let body = tcx.typeck(&prog, def);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    assert!(!body.hidden_types.is_empty());
}

#[test]
fn round28_writeback_normalizes_resolved_projections() {
    let (prog, tcx) = program("#![no_std] trait Project { type Output; } impl Project for bool { type Output = u8; }");
    let output = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "Output" && prog.def(d.parent).kind == DefKind::Trait).map(|(i, _)| DefId(i as u32)).unwrap();
    let state = crate::tcx::EvalState::new(); let mut fcx = context(&prog, &tcx, &state);
    let ty = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: output }, args: tcx.tys.mk_list(&[tcx.tys.bool_]), regions: crate::types::RegionArgsId::EMPTY }));
    fcx.b.ret = ty;
    fcx.resolve_type_vars_in_body();
    assert!(fcx.b.errors.is_empty(), "{:?}", fcx.b.errors);
    assert_eq!(fcx.b.ret, tcx.tys.prim(crate::program::Prim::U8));
}

#[test]
fn round28_rigid_projection_identity_and_dependencies() {
    let (prog, tcx) = program("#![no_std] trait Project { type Output; } struct Payload;");
    let named = |name: &str| prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == name).map(|(i, _)| DefId(i as u32)).unwrap();
    let payload = named("Payload");
    let args = vec![tcx.tys.intern(TyKind::Adt(payload, tcx.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY))];
    let alias = AliasTy { kind: AliasTyKind::Projection { def_id: named("Output") }, args: tcx.tys.mk_list(&args), regions: crate::types::RegionArgsId::EMPTY };
    let rigid = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::Yes, alias.clone()));
    let nonrigid = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, alias));
    assert_eq!(ty_to_string(true, &prog, &tcx, rigid), ty_to_string(true, &prog, &tcx, nonrigid));
    let mut deps = Vec::new();
    type_dependencies(&tcx, rigid, &mut deps);
    assert!(deps.contains(&payload));
}

#[test]
#[should_panic(expected = "unexpected cyclic definition")]
fn round28_writeback_rejects_new_opaque_definitions() {
    let (prog, tcx) = program("#![no_std] trait Value {} fn probe() -> impl Value { loop {} }");
    let def = prog.defs.iter().enumerate().find(|(_, d)| d.kind == DefKind::OpaqueTy).map(|(i, _)| DefId(i as u32)).unwrap();
    let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    fcx.checked_opaque_types_storage_entries = Some(fcx.infcx.inner.borrow_mut().opaque_types().num_entries());
    fcx.infcx.inner.borrow_mut().opaque_types().register(
        crate::infer::opaque_types::OpaqueTypeKey { def_id: def, args: Vec::new().into() },
        crate::infer::opaque_types::ProvisionalHiddenType::new(tcx.tys.u32_));
    fcx.detect_opaque_types_added_during_writeback();
}

#[test]
fn round29_obligation_cause_survives_fulfillment_and_snapshot() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new(); let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    let snapshot = fcx.start_snapshot();
    fcx.register_goal(Goal { param_env: Default::default(), predicate: Predicate::dummy(PredicateKind::Ambiguous) }, 73);
    fcx.commit_from(snapshot);
    fcx.last_lo = 999;
    let errors = fcx.fulfillment_cx.evaluate_obligations_error_on_ambiguity(&fcx.infcx, &prog);
    let crate::next_solver::fulfill::NextSolverError::Ambiguity(obligation) = &errors[0] else { panic!() };
    assert_eq!(obligation.cause.span, 73);
    assert_eq!(obligation.cause.body_id, fcx.b.def);
}

#[test]
fn round29_callable_lub_and_reference_arms() {
    let (prog, tcx) = program("#![no_std] fn a(x: u8) -> u8 { x } fn b(x: u8) -> u8 { x } fn probe(c: bool) -> fn(u8) -> u8 { if c { a } else { b } } fn refs<'a, 'b>(c: bool, a: &'a u8, b: &'b u8) { let _ = if c { a } else { b }; }");
    for name in ["probe", "refs"] {
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == name).map(|(i, _)| DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(body.errors.is_empty(), "{name}: {:?}", body.errors);
    }
}

#[test]
fn round32_failed_coercion_does_not_emit_or_taint() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let state = crate::tcx::EvalState::new();
    let mut fcx = context(&prog, &tcx, &state);
    assert!(!fcx.try_coerce(tcx.tys.bool_, tcx.tys.u8_, None));
    assert!(!fcx.infcx.tainted_by_errors());
    assert_eq!(tcx.dcx.err_count(), 0);
    assert!(fcx.b.errors.is_empty());
    assert!(fcx.try_coerce(tcx.tys.u8_, tcx.tys.u8_, None));
    assert!(!fcx.infcx.tainted_by_errors());
}
