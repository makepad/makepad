use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use crate::consts::GenericArg;
use crate::next_solver::{ParamEnv, Predicate, PredicateKind, ClauseKind, ProjectionClause, AliasTerm, Term, TraitRef};
use crate::next_solver::evaluate_obligation::EvaluationResult;
use super::*;

#[test]
fn inference_diagnostics_do_not_mask_body_error() {
    let (prog, tcx) = program(br#"#![no_std]
        #[lang = "fn_once"] trait FnOnce<Args> {
            #[lang = "fn_once_output"] type Output;
        }
        struct Choice<T>(T);
        impl<T> Choice<T> {
            fn accepts<F: FnOnce(T) -> bool>(self, _f: F) -> bool { true }
        }
        trait Property {}
        impl Property for u8 {}
        fn consume<T: Property>(_: fn() -> T) {}
        fn broken() {
            let _ = Choice(1u8).accepts(|value| value == 1);
            consume(missing);
        }
        fn unknown<T: Property>() {}
        fn ambiguous() { unknown(); }
    "#);
    let state = EvalState::new();
    let cx = EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
    let body = crate::typeck::instantiate_body(&cx, &prog, &tcx, named(&prog, "broken"), &[]);
    assert!(body.errors.iter().any(|e| e.contains("cannot find value `missing`")), "{:?}", body.errors);
    assert!(!body.errors.iter().any(|e| e.contains("E0283")), "{:?}", body.errors);
    assert!(!body.errors.iter().any(|e| e.contains("ambiguous obligation")), "{:?}", body.errors);
    // The ambiguity is reported by the fulfillment loop (E0283, type annotations needed).
    let body = crate::typeck::instantiate_body(&cx, &prog, &tcx, named(&prog, "ambiguous"), &[]);
    assert!(body.errors.iter().any(|e| e.contains("E0283")), "{:?}", body.errors);
}

fn program(source: &[u8]) -> (Program, Tcx) {
    let dir = std::env::temp_dir().join(format!("relax-typing-env-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, source).unwrap();
    let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    prog.load_crate("typing", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&prog.session, &[]), Vec::new()).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);

    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (prog, tcx)
}

fn named(prog: &Program, name: &str) -> DefId {
    for (i, _) in prog.defs.iter().enumerate() {
        let d = DefId(i as u32);
        if prog.name(d) == name { return d; }
    }
    panic!("missing {name}")
}

#[test]
fn projection_fields_do_not_cross_environments() {
    let (prog, tcx) = program(b"#![no_std]\ntrait Tr { type A; } struct S<T: Tr> { value: T::A }");
    let assoc = named(&prog, "A");
    let param = tcx.tys.intern(TyKind::Param(0));
    let projection = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: assoc }, args: tcx.tys.mk_list(&[param]), regions: crate::types::RegionArgsId::EMPTY }));
    let tr = named(&prog, "Tr");
    // the environment proves the projection's trait goal, as an elaborated one does
    let env = |self_ty: TyId, alias: TyId, value: TyId| ParamEnv { clauses: vec![
        crate::next_solver::collect::trait_predicate(tr, vec![GenericArg::Type(self_ty)]),
        Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Ty(alias), term: Term::Ty(value) }))),
    ].into() };
    let a = env(param, projection, tcx.tys.bool_);
    let b = env(param, projection, tcx.tys.char_);
    let a_state = crate::tcx::EvalState::new();
    let a = EvalCx::new(&prog, &tcx, TypingEnv { limit: Default::default(), param_env: tcx.intern_param_env(a), mode: TypingMode::Typeck { defining_opaque_types_and_generators: Default::default() } }, &a_state);
    let b_state = crate::tcx::EvalState::new();
    let b = EvalCx::new(&prog, &tcx, TypingEnv { limit: Default::default(), param_env: tcx.intern_param_env(b), mode: TypingMode::PostTypeckUntilBorrowck { defining_opaque_types: Default::default() } }, &b_state);
    for _ in 0..2 {
        assert_eq!(tcx.field_ty(&prog, a.typing_env, named(&prog, "S"), 0, 0, &[param]), tcx.tys.bool_);
        assert_eq!(tcx.field_ty(&prog, b.typing_env, named(&prog, "S"), 0, 0, &[param]), tcx.tys.char_);
    }
    // Concrete projections are memoized too; a predicate environment remains an input.
    let concrete = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Projection { def_id: assoc }, args: tcx.tys.mk_list(&[tcx.tys.bool_]), regions: crate::types::RegionArgsId::EMPTY }));
    let aenv = env(tcx.tys.bool_, concrete, tcx.tys.bool_);
    let benv = env(tcx.tys.bool_, concrete, tcx.tys.char_);
    let a_state = crate::tcx::EvalState::new();
    let a = EvalCx::new(&prog, &tcx, TypingEnv { limit: Default::default(), param_env: tcx.intern_param_env(aenv), mode: TypingMode::PostAnalysis }, &a_state);
    let b_state = crate::tcx::EvalState::new();
    let b = EvalCx::new(&prog, &tcx, TypingEnv { limit: Default::default(), param_env: tcx.intern_param_env(benv), mode: TypingMode::PostAnalysis }, &b_state);
    for _ in 0..2 {
        assert_eq!(tcx.normalize_erasing_regions(&prog, a.typing_env, concrete), tcx.tys.bool_);
        assert_eq!(tcx.normalize_erasing_regions(&prog, b.typing_env, concrete), tcx.tys.char_);
    }
}

#[test]
fn selection_cycles_leave_other_roots_usable() {
    let (prog, tcx) = program(b"#![no_std]\ntrait Tr {} trait Good {} struct S; impl<T: Tr> Tr for T {} impl Good for S {}");
    let tr = named(&prog, "Tr");
    let good = named(&prog, "Good");
    let s = tcx.tys.intern(TyKind::Adt(named(&prog, "S"), crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
    let a = TypingEnv::fully_monomorphized(&prog);
    let b = TypingEnv { limit: Default::default(), param_env: Default::default(), mode: TypingMode::Typeck { defining_opaque_types_and_generators: Default::default() } };
    // a cycle through an ordinary impl is ambiguous (rustc_next_trait_solver
    // solve/search_graph.rs:45-60 initial_provisional_result) and stays local to its root
    let goal = |td: DefId, ty: TyId| TraitRef { def_id: td, args: vec![GenericArg::Type(ty)] };
    assert_eq!(tcx.type_implements_trait(&prog, a, goal(tr, tcx.tys.bool_)), EvaluationResult::EvaluatedToAmbig);
    assert_eq!(tcx.type_implements_trait(&prog, b, goal(good, s)), EvaluationResult::EvaluatedToOk);
    assert_eq!(tcx.type_implements_trait(&prog, b, goal(tr, tcx.tys.bool_)), EvaluationResult::EvaluatedToAmbig);
    assert_eq!(tcx.type_implements_trait(&prog, a, goal(good, s)), EvaluationResult::EvaluatedToOk);
}

#[test]
fn empty_analysis_and_nested_body_inputs_are_explicit() {
    let (prog, tcx) = program(b"#![no_std]\ntrait Tr<A> {} impl Tr<u8> for bool {} fn inner() -> u8 { 1 }");
    let tr = named(&prog, "Tr");
    let param = tcx.tys.intern(TyKind::Param(0));
    let mono_state = crate::tcx::EvalState::new();
    let mono = EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &mono_state);
    let analysis_state = crate::tcx::EvalState::new();
    let analysis = EvalCx::new(&prog, &tcx, TypingEnv { limit: Default::default(), param_env: Default::default(), mode: TypingMode::Typeck { defining_opaque_types_and_generators: Default::default() } }, &analysis_state);
    let goal = TraitRef { def_id: tr, args: vec![GenericArg::Type(tcx.tys.bool_), GenericArg::Type(param)] };
    assert_eq!(tcx.type_implements_trait(&prog, mono.typing_env, goal.clone()), EvaluationResult::EvaluatedToErr);
    assert_eq!(tcx.type_implements_trait(&prog, analysis.typing_env, goal.clone()), EvaluationResult::EvaluatedToErr);
    let body = crate::typeck::instantiate_body(&analysis, &prog, &tcx, named(&prog, "inner"), &[]);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    assert_eq!(tcx.type_implements_trait(&prog, analysis.typing_env, goal), EvaluationResult::EvaluatedToErr);
}

#[test]
fn environments_are_interned_across_threads_and_growth() {
    let tcx = Tcx::new();
    assert_eq!(tcx.intern_param_env(ParamEnv::default()), ParamEnvId::EMPTY);
    assert_eq!(TypingEnv::fully_monomorphized(&Program::new(&std::sync::Arc::new(crate::Session::default()))).param_env, ParamEnvId::EMPTY);
    let clause = |def: u32, ty: TyId| crate::next_solver::collect::trait_predicate(DefId(def), vec![GenericArg::Type(ty)]);
    let list = |v: Vec<Predicate>| ParamEnv { clauses: v.into() };
    let env = list(vec![clause(1, tcx.tys.bool_), clause(1, tcx.tys.char_), clause(1, tcx.tys.u32_)]);
    let id = tcx.intern_param_env(env.clone());
    let retained = tcx.param_env_by_id(id);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let tcx = &tcx;
            let env = &env;
            scope.spawn(move || {
                for i in 0..1024 {
                    let other = list(vec![clause(i + 2, tcx.tys.bool_), clause(1, tcx.tys.char_), clause(1, tcx.tys.u32_)]);
                    let other_id = tcx.intern_param_env(other.clone());
                    assert_eq!(tcx.param_env_by_id(other_id), &other);
                    assert_eq!(tcx.intern_param_env(other), other_id);
                    assert_eq!(tcx.intern_param_env(env.clone()), id);
                }
            });
        }
    });
    assert!(std::ptr::eq(retained, tcx.param_env_by_id(id)));
    assert_eq!(retained, &env);
    assert_ne!(tcx.intern_param_env(list(vec![clause(1, tcx.tys.bool_), clause(1, tcx.tys.u8_), clause(1, tcx.tys.u32_)])), id);
    assert_ne!(tcx.intern_param_env(list(vec![clause(1, tcx.tys.bool_), clause(1, tcx.tys.char_), clause(1, tcx.tys.u8_)])), id);
    let owners = tcx.intern_defining_opaques(vec![DefId(1), DefId(2)]);
    assert_eq!(tcx.intern_defining_opaques(vec![DefId(1), DefId(2)]), owners);
    assert_ne!(tcx.intern_defining_opaques(vec![DefId(1)]), owners);
    assert_eq!(tcx.defining_opaques(owners), &[DefId(1), DefId(2)]);
    assert!(tcx.defining_opaques(Default::default()).is_empty());
}

#[test]
fn copying_typing_env_and_borrowing_nested_state_preserves_keys() {
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    let state = EvalState::new();
    let env = TypingEnv { limit: Default::default(), param_env: ParamEnvId::EMPTY, mode: TypingMode::Typeck { defining_opaque_types_and_generators: tcx.intern_defining_opaques(vec![DefId(1)]) } };
    let copied = env;
    let mut memo = crate::fxhash::HashMap::default();
    memo.insert(env, true);
    assert_eq!(memo.get(&copied), Some(&true));
    // ParamEnvId + TypingMode + the crate's RecursionLimit (rustc_interface limits.rs: usize).
    assert_eq!(std::mem::size_of::<TypingEnv>(), 24);
    let cx = EvalCx::new(&prog, &tcx, copied, &state);
    let nested = cx.body();
    assert!(std::ptr::eq(cx.state, nested.state));
    nested.state.tc_depth.set(7);
    assert_eq!(cx.state.tc_depth.get(), 7);
}

#[test]
fn native_fresh_args_cover_inherited_lifetimes_types_and_consts() {
    use crate::consts::GenericArg;
    let (prog, tcx) = program(br#"#![no_std]
        trait Family<'a, T, const N: usize> {
            fn member<'b, U, const M: usize>(&'b self);
        }
        struct Data<'a, T, const N: usize>(&'a T);
    "#);
    let infcx = crate::infer::InferCtxt::new(&tcx, TypingMode::PostAnalysis, Default::default());
    let tr = named(&prog, "Family");
    let member = named(&prog, "member");
    let data = named(&prog, "Data");
    let family = infcx.fresh_args_for_item(&prog, tr);
    assert!(matches!(&family[..], [GenericArg::Type(_), GenericArg::Lifetime(_), GenericArg::Type(_), GenericArg::Const(_)]));
    let args = infcx.fresh_args_for_item(&prog, member);
    assert!(matches!(&args[..], [GenericArg::Type(_), GenericArg::Lifetime(_), GenericArg::Type(_), GenericArg::Const(_), GenericArg::Type(_), GenericArg::Const(_)]));
    let args = infcx.fresh_args_for_item(&prog, data);
    assert!(matches!(&args[..], [GenericArg::Lifetime(_), GenericArg::Type(_), GenericArg::Const(_)]));
}

#[test]
fn native_definition_aliases_keep_owner_and_arguments() {
    use crate::consts::{AliasConstKind, ConstKind};
    let (mut prog, tcx) = program(br#"#![no_std]
        trait Tr { type Item; }
        struct S<'a, T, const N: usize> { r: &'a T, array: [T; N + 1] }
        fn f<'a, const N: usize>(_: &'a u8) -> impl Tr<Item = impl Tr> { loop {} }
        async fn g<const N: usize>() -> impl Tr { loop {} }
    "#);
    prog.resolve_imports();
    let s = named(&prog, "S");
    let adt = tcx.adts.get(&s).unwrap();
    let TyKind::Array(_, n) = tcx.tys.kind(adt.variants[0].fields[1].ty) else { panic!() };
    let ConstKind::Alias(crate::consts::IsRigid::No, alias) = tcx.tys.const_kind(*n) else { panic!() };
    let AliasConstKind::Anon { def_id } = alias.kind else { panic!() };
    assert_eq!(prog.def(def_id).kind, DefKind::AnonConst);
    assert_eq!(prog.def(prog.def(def_id).parent).kind, DefKind::Field);
    assert_eq!(prog.def(prog.def(prog.def(def_id).parent).parent).name, prog.def(s).name);
    assert!(alias.args.is_empty());
    assert!(prog.errors.iter().any(|e| e.contains("generic parameters may not be used in const operations")));
    let f = named(&prog, "f");
    let TyKind::Alias(_, opaque) = tcx.tys.kind(tcx.fn_sig(&prog, f).unwrap().ret) else { panic!() };
    assert_eq!(prog.def(opaque.kind.def_id()).kind, DefKind::OpaqueTy);
    assert_eq!(prog.opaque_owner(opaque.kind.def_id()), f);
    assert!(matches!(tcx.tys.kind(tcx.tys.list(opaque.args)[0]), TyKind::Const(_)));
    let g = named(&prog, "g");
    let TyKind::Alias(_, future) = tcx.tys.kind(tcx.fn_sig(&prog, g).unwrap().ret) else { panic!() };
    assert_eq!(prog.opaque_owner(future.kind.def_id()), g);
    assert_eq!(prog.def(future.kind.def_id()).kind, DefKind::OpaqueTy);
}

// Round-7 diagnostic: t15's float formatting uses precisely this named array
// length (compiler/std/core/fmt/float/bignum.rs:6,13). This is an acceptance
// requirement, not a test that treats TooGeneric as an evaluated value.
#[test]
fn round7_named_array_length_requires_ctfe_provider() {
    use crate::consts::{AliasConstKind, ConstKind};
    let (prog, tcx) = program(br#"#![no_std]
        const LIMBS: usize = 40;
        struct Big { base: [u32; LIMBS] }
    "#);
    let adt = tcx.adts.get(&named(&prog, "Big")).unwrap();
    let TyKind::Array(_, n) = tcx.tys.kind(adt.variants[0].fields[0].ty) else { panic!() };
    let ConstKind::Alias(crate::consts::IsRigid::No, alias) = tcx.tys.const_kind(*n) else { panic!("nonliteral length must keep its definition") };
    let AliasConstKind::Anon { def_id } = alias.kind else { panic!() };
    assert_eq!(prog.def(def_id).kind, DefKind::AnonConst);
    assert!(alias.args.is_empty());
    let result = tcx.const_eval_resolve(&prog, TypingEnv::fully_monomorphized(&prog), def_id, &alias.args);
    assert_eq!(result.map(|v| v.try_to_leaf().unwrap().to_bits(8)), Ok(40));
}

#[test]
fn round29_borrowck_retains_non_generic_and_region_only_clauses() {
    let (prog, tcx) = program(br#"#![no_std]
        trait Tr {} impl Tr for u8 {}
        fn plain() where u8: Tr {}
        fn regions<'a, 'b: 'a>(_: &'a u8, _: &'b u8) where u8: Tr {}
    "#);
    for name in ["plain", "regions"] {
        let def = named(&prog, name);
        let infcx = crate::borrowck::infer_ctxt::BorrowckInferCtxt::new(&tcx, &prog, def, def, &[]);
        assert!(!infcx.param_env.clauses.is_empty());
        assert!(infcx.param_env.clauses.iter().any(|p| matches!(&p.kind,
            PredicateKind::Clause(ClauseKind::Trait(tr)) if tr.trait_ref.def_id == named(&prog, "Tr"))));
        if name == "regions" {
            assert!(infcx.param_env.clauses.iter().any(|p| matches!(p.kind,
                PredicateKind::Clause(ClauseKind::RegionOutlives(..)))));
        }
        let state = EvalState::new();
        let env = TypingEnv { limit: Default::default(), param_env: tcx.intern_param_env(infcx.param_env.clone()), mode: TypingMode::borrowck(&tcx, &prog, def) };
        let tx = crate::borrowck::tyx::Tx::new(&prog, &tcx, env, &state);
        tx.polymorphic.set(true);
        tx.reset_env();
        assert_eq!(*tx.param_env.borrow(), infcx.param_env);
    }
}

#[test]
fn round29_borrowck_opaque_list_and_rigid_skeleton() {
    let (prog, tcx) = program(br#"#![no_std]
        trait Tr {} impl Tr for u8 {}
        fn opaque<T>(_: T) -> impl Tr { 1u8 }
        fn ordinary() {}
    "#);
    let owner = named(&prog, "opaque");
    let mode = TypingMode::borrowck(&tcx, &prog, owner);
    let TypingMode::PostTypeckUntilBorrowck { defining_opaque_types } = mode else { panic!("missing defining scope") };
    let opaque = prog.opaque_types_defined_by[&owner][0];
    assert!(tcx.defining_opaques(defining_opaque_types).contains(&opaque));
    assert!(!tcx.defining_opaques(defining_opaque_types).contains(&owner));
    assert!(matches!(TypingMode::borrowck(&tcx, &prog, named(&prog, "ordinary")), TypingMode::Typeck { .. }));
    let state = EvalState::new();
    let tx = crate::borrowck::tyx::Tx::new(&prog, &tcx, TypingEnv { limit: Default::default(), param_env: Default::default(), mode }, &state);
    let rty = crate::borrowck::rty::Rty::new(&tx);
    let arg = tcx.tys.intern(TyKind::Ref(false, tcx.tys.u8_, tcx.tys.intern_region(crate::types::RegionKind::ReStatic)));
    let alias = AliasTy { kind: AliasTyKind::Opaque { def_id: opaque }, args: tcx.tys.mk_list(&[arg]), regions: crate::types::RegionArgsId::EMPTY };
    let rigid = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::Yes, alias.clone()));
    let nonrigid = tcx.tys.intern(TyKind::Alias(crate::consts::IsRigid::No, alias));
    assert_eq!(rty.skel(rigid), rty.skel(nonrigid));
    assert!(!rty.skel(rigid).is_empty());
}

#[test]
fn round32_body_taint_ignores_untyped_arena_slots() {
    let (prog, tcx) = program(br#"#![no_std]
        fn identity<const N: usize>(x: u8) -> u8 { x }
        fn good() -> u8 { identity::<{ 2 + 3 }>(4) }
        fn bad() -> u8 { missing }
    "#);
    let good = tcx.typeck(&prog, named(&prog, "good"));
    assert!(good.expr_typed.iter().any(|typed| !typed));
    assert!(good.errors.is_empty(), "{:?}", good.errors);
    let bad = tcx.typeck(&prog, named(&prog, "bad"));
    assert!(!bad.errors.is_empty());
    assert!(bad.expr_ty.iter().zip(bad.expr_typed.iter()).any(|(ty, typed)| typed && tcx.tys.has_error(*ty)));
    // A cached body retains its taint without emitting the same diagnostic twice.
    let count = tcx.dcx.err_count();
    assert!(!tcx.typeck(&prog, named(&prog, "bad")).errors.is_empty());
    assert_eq!(tcx.dcx.err_count(), count);
}

#[test]
fn round32_self_variant_must_not_taint_valid_body() {
    let (prog, tcx) = program(br#"#![no_std]
        enum E { A, B(u8) }
        impl E {
            fn valid(&self) -> bool {
                match self { Self::A => true, Self::B(_) => false }
            }
        }
    "#);
    let body = tcx.typeck(&prog, named(&prog, "valid"));
    assert!(body.errors.is_empty(), "valid Self::Variant path tainted: {:?}", body.errors);
    assert_eq!(tcx.dcx.err_count(), 0);
}

#[test]
fn round32_failed_required_coercion_reports_once() {
    let (prog, tcx) = program(b"#![no_std] fn bad() -> bool { 1u8 }");
    let body = tcx.typeck(&prog, named(&prog, "bad"));
    assert!(body.errors.iter().any(|e| e.contains("mismatched types")), "{:?}", body.errors);
    assert_eq!(tcx.dcx.err_count(), 1);
    assert!(!tcx.typeck(&prog, named(&prog, "bad")).errors.is_empty());
    assert_eq!(tcx.dcx.err_count(), 1);
}

#[test]
fn round33_bound_traits_are_candidates_only_for_their_parameter() {
    // method/probe.rs:926-928 (param candidates) vs :1092-1175 (extension candidates = traits in
    // scope): `T: m::Hidden` makes `x.hidden()` resolve on the parameter, but not `y.hidden()` on
    // a concrete `u8` when `Hidden` is not imported (rustc E0599).
    let (prog, tcx) = program(br#"#![no_std]
        mod m { pub trait Hidden { fn hidden(&self) -> u8 { 1 } } impl Hidden for u8 {} }
        fn on_param<T: m::Hidden>(x: T) -> u8 { x.hidden() }
        fn on_concrete<T: m::Hidden>(x: T, y: u8) -> u8 { y.hidden() }
        struct L;
        impl m::Hidden for L { fn hidden(&self) -> u8 { in_impl(self) } }
        fn in_impl(_: &L) -> u8 { 0 }
        struct K;
        impl m::Hidden for K { fn hidden(&self) -> u8 { 2u8.hidden() } }
    "#);
    assert!(tcx.typeck(&prog, named(&prog, "on_param")).errors.is_empty());
    assert!(!tcx.typeck(&prog, named(&prog, "on_concrete")).errors.is_empty());
    // rustc_resolve late.rs:5515-5522: inside `impl m::Hidden for K` the trait itself is in scope.
    let in_k = (0..prog.defs.len() as u32).map(DefId).filter(|&d| prog.name(d) == "hidden" && prog.def(d).kind == DefKind::AssocFn).last().unwrap();
    assert!(tcx.typeck(&prog, in_k).errors.is_empty(), "{:?}", tcx.typeck(&prog, in_k).errors);
}

#[test]
fn round39_field_view_failed_instantiation_is_unknown() {
    let (prog, tcx) = program(br#"#![no_std]
        trait Maker { type Item; fn make() -> Self::Item; }
        impl Maker for u8 { type Item = u32; fn make() -> u32 { 0 } }
        struct Pair { left: u32, right: u32 }
        fn projected<T: Maker>(pair: &mut Pair) {
            let _value = T::make();
            pair.left = 1;
        }
        fn forwarded<T: Maker>(pair: &mut Pair) { projected::<T>(pair); }
    "#);
    let state = EvalState::new();
    let env = TypingEnv::fully_monomorphized(&prog);
    let cx = EvalCx::new(&prog, &tcx, env, &state);
    let tx = crate::borrowck::tyx::Tx::new(&prog, &tcx, env, &state);
    let def = named(&prog, "projected");
    let body = tcx.typeck(&prog, def);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let args = tcx.generic_indices_of(&prog, def).identity_types_consts(&tcx.tys);
    assert!(crate::typeck::try_instantiate_body(&cx, &prog, &tcx, def, &args).is_err());
    let projection = body.expr_ty.iter().copied().find(|ty| tcx.tys.may_project(*ty)).unwrap();
    assert!(crate::typeck::try_instantiate_body(&cx, &prog, &tcx, def, &[projection]).is_err());
    for _ in 0..2 {
        // Failure (including a cached summary) must never mean an empty Parts set.
        assert!(crate::borrowck::fv::summaries(&prog, &tcx, &tx, def, &args, 0).is_empty());
        let forwarded = crate::borrowck::fv::summaries(&prog, &tcx, &tx, named(&prog, "forwarded"), &args, 0);
        assert_eq!(*forwarded, vec![crate::borrowck::fv::Summary::Whole]);
    }
    let concrete = crate::typeck::try_instantiate_body(&cx, &prog, &tcx, def, &[tcx.tys.u8_]).unwrap();
    assert!(concrete.errors.is_empty(), "{:?}", concrete.errors);
    let summary = crate::borrowck::fv::summaries(&prog, &tcx, &tx, def, &[tcx.tys.u8_], 0);
    assert_eq!(summary.len(), 1);
    assert!(matches!(&summary[0], crate::borrowck::fv::Summary::Parts(parts) if parts.len() == 1 && parts[0].1));
}
