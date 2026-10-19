use super::*;
use crate::consts::ValTree;

fn program(source: &[u8]) -> (Program, Tcx) {
    let dir = std::env::temp_dir().join(format!("relax-const-query-{}-{}", std::process::id(), crate::unique_test_suffix()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, source).unwrap();
    let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    prog.load_crate("const_query", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&prog.session, &[]), Vec::new()).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (prog, tcx)
}
fn named(prog: &Program, name: &str) -> DefId {
    for i in 0..prog.defs.len() { let d = DefId(i as u32); if prog.name(d) == name { return d; } }
    panic!("missing {name}")
}
fn leaf(result: Result<ValTree, ErrorHandled>, size: u8) -> Result<u128, ErrorHandled> {
    result.map(|value| value.try_to_leaf().expect("expected scalar valtree").to_bits(size))
}

// Folding keeps the declared integer width and signedness.
#[test]
fn arch_n01_evaluator_wide_literal_control() {
    let (prog, tcx) = program(br#"#![no_std]
        const WIDE: u128 = 170141183460469231731687303715884105728u128;
    "#);
    let value = tcx.const_eval_resolve(&prog, TypingEnv::fully_monomorphized(&prog), named(&prog, "WIDE"), &[]);
    assert_eq!(leaf(value, 16), Ok(1u128 << 127));
}

#[test]
fn arch_n01_evaluator_unsigned_division_preserves_value() {
    let (prog, tcx) = program(br#"#![no_std]
        const HALF: u128 = 170141183460469231731687303715884105728u128 / 2u128;
    "#);
    let value = tcx.const_eval_resolve(&prog, TypingEnv::fully_monomorphized(&prog), named(&prog, "HALF"), &[]);
    assert_eq!(leaf(value, 16), Ok(1u128 << 126), "constant folding must preserve unsigned arithmetic");
}

#[test]
fn round34_mcg_anon_const_has_no_parent_generics() {
    use crate::consts::ConstKind;
    use crate::types::TyKind;
    let (mut prog, tcx) = program(br#"#![no_std]
        struct Buffer<'a, T, const N: usize> { value: &'a T, bytes: [u8; N + 1] }
    "#);
    prog.resolve_imports();
    assert!(prog.errors.iter().any(|e| e.contains("generic parameters may not be used in const operations")), "{:?}", prog.errors);
    let adt = &tcx.adts[&named(&prog, "Buffer")];
    let TyKind::Array(_, len) = tcx.tys.kind(adt.variants[0].fields[1].ty) else { panic!() };
    let ConstKind::Alias(_, alias) = tcx.tys.const_kind(*len) else { panic!() };
    assert!(alias.args.is_empty());
    let def = alias.kind.def_id();
    assert!(tcx.generic_indices_of(&prog, def).types_consts.is_empty());
    assert!(tcx.generic_indices_of(&prog, def).lifetimes.is_empty());
    assert!(tcx.clauses_of(&prog, def).is_empty());
}

#[test]
fn round34_unused_const_default_is_checked() {
    let (mut prog, tcx) = program(br#"#![no_std]
        struct BadDefault<const N: u8 = "bad">;
    "#);
    prog.resolve_imports();
    tcx.check_signature_errors(&prog, 0);
    let roots = crate::borrowck::body::signature_anon_consts(&prog, &tcx, 0);
    assert_eq!(roots.len(), 1, "default's body was not lowered");
    assert!(!tcx.typeck(&prog, roots[0]).errors.is_empty());
}

#[test]
fn round8_provider_reports_errors_and_evaluates_assoc_const() {
    let (prog, tcx) = program(br#"#![no_std]
        struct S;
        impl S { const COUNT: usize = 40; }
        const BAD: usize = 1 / 0;
        const BOOLEAN: bool = true;
        const CHAR: char = 'a';
    "#);
    let env = TypingEnv::fully_monomorphized(&prog);
    assert_eq!(leaf(tcx.const_eval_resolve(&prog, env, named(&prog, "COUNT"), &[]), 8), Ok(40));
    assert!(matches!(tcx.const_eval_resolve(&prog, env, named(&prog, "BAD"), &[]), Err(ErrorHandled::Reported(_))));
    assert_eq!(leaf(tcx.const_eval_resolve(&prog, env, named(&prog, "BOOLEAN"), &[]), 1), Ok(1));
    assert_eq!(leaf(tcx.const_eval_resolve(&prog, env, named(&prog, "CHAR"), &[]), 4), Ok(97));
}

#[test]
fn round8_unused_generic_argument_does_not_make_value_ambiguous() {
    use crate::consts::ConstKind;
    let (prog, tcx) = program(br#"#![no_std]
        const N: usize = 7;
        const VALUE: usize = N + 1;
        struct S<const N: usize> { field: [u8; VALUE] }
    "#);
    let TyKind::Array(_, len) = tcx.tys.kind(tcx.adts[&named(&prog, "S")].variants[0].fields[0].ty) else { panic!() };
    let ConstKind::Alias(_, alias) = tcx.tys.const_kind(*len) else { panic!() };
    assert_eq!(leaf(tcx.const_eval_resolve(&prog, TypingEnv::fully_monomorphized(&prog), alias.kind.def_id(), &alias.args), 8), Ok(8));
}

#[test]
fn round31_array_length_uses_const_query() {
    use crate::consts::ConstKind;
    let (prog, tcx) = program(br#"#![no_std]
        const LIMBS: usize = 40;
        struct Big { base: [u32; LIMBS] }
    "#);
    let TyKind::Array(_, len) = tcx.tys.kind(tcx.adts[&named(&prog, "Big")].variants[0].fields[0].ty) else { panic!() };
    let ConstKind::Alias(_, alias) = tcx.tys.const_kind(*len) else { panic!() };
    assert_eq!(leaf(tcx.const_eval_resolve(&prog, TypingEnv::fully_monomorphized(&prog), alias.kind.def_id(), &alias.args), 8), Ok(40));
}

#[test]
fn round15_anon_const_expected_type_reaches_evaluation_and_solver() {
    use relax_front::def_collector::NodeId;
    use crate::consts::{ConstKind, IsRigid};
    use crate::next_solver::{AliasTerm, Goal, ParamEnvId, Predicate, PredicateKind, ClauseKind, ProjectionClause, Term};
    let (prog,tcx) = program(b"struct S<const B: bool>; fn owner() { let _: S<{true}>; }");
    let def = named(&prog,"owner");
    let (file,expr) = prog.node_id_to_def_id.iter().find_map(|(&(file,node),_)| match node { NodeId::AnonConst(e) => Some((file,e)), _ => None }).unwrap();
    let env = tcx.item_env(&prog,def);
    let ct = tcx.lower_const_arg(&prog,file,&env,expr,tcx.tys.bool_);
    let ConstKind::Alias(IsRigid::No,alias) = tcx.tys.const_kind(ct) else { panic!("anonymous body must remain unevaluated") };
    assert_eq!(tcx.type_of(&prog,alias.kind.def_id()).unwrap(),tcx.tys.bool_);
    assert_eq!(leaf(tcx.const_eval_resolve(&prog,TypingEnv::fully_monomorphized(&prog),alias.kind.def_id(),&alias.args),1),Ok(1));
    let infcx = crate::infer::InferCtxt::new(&tcx,crate::tcx::TypingMode::PostAnalysis, Default::default());
    let result = infcx.next_const_var();
    assert!(infcx.evaluate_root_goal(&prog,Goal { param_env: ParamEnvId::EMPTY, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause { projection_term: AliasTerm::Const(alias),term: Term::Const(result) }))) },None).unwrap().certainty.is_yes());
    let ConstKind::Value(value) = tcx.tys.const_kind(infcx.shallow_resolve_const(result)) else { panic!() };
    assert_eq!(value.ty,tcx.tys.bool_);
}

#[test]
fn round30_fold_checked_scalar_operations() {
    let (prog, tcx) = program(br#"#![no_std]
        const ADD: u8 = 255u8 + 1u8;
        const SUB: u8 = 0u8 - 1u8;
        const MUL: i8 = 64i8 * 2i8;
        const SHIFT: u8 = 1u8 << 8u8;
        const REM: i8 = -128i8 % -1i8;
        const GOOD: i8 = -128i8;
        const BOOL: bool = !true;
        const WIDE: u128 = 170141183460469231731687303715884105728u128 / 2u128;
        const BAD_WIDE: u128 = 170141183460469231731687303715884105728u128 * 2u128;
    "#);
    let env = TypingEnv::fully_monomorphized(&prog);
    for name in ["ADD", "SUB", "MUL", "SHIFT", "REM", "BAD_WIDE"] {
        assert!(matches!(tcx.const_eval_resolve(&prog, env, named(&prog, name), &[]), Err(ErrorHandled::Reported(_))), "{name}");
    }
    assert_eq!(leaf(tcx.const_eval_resolve(&prog, env, named(&prog, "GOOD"), &[]), 1), Ok(128));
    assert_eq!(leaf(tcx.const_eval_resolve(&prog, env, named(&prog, "BOOL"), &[]), 1), Ok(0));
    assert_eq!(leaf(tcx.const_eval_resolve(&prog, env, named(&prog, "WIDE"), &[]), 16), Ok(1u128 << 126));
}

#[test]
fn round31_discriminants_query_explicit_relative_signed_and_wide() {
    use crate::tcx::VariantDiscr;
    let (prog, tcx) = program(br#"#![no_std]
        #[repr(i8)] enum Signed { A = -2, B, C = 126, D }
        #[repr(u128)] enum Wide { A = 340282366920938463463374607431768211454u128, B }
        #[repr(i128)] enum SignedWide { A = -170141183460469231731687303715884105728i128, B }
    "#);
    let signed = &tcx.adts[&named(&prog, "Signed")];
    assert!(matches!(signed.variants[0].discr, VariantDiscr::Explicit(_)));
    assert_eq!(signed.variants[1].discr, VariantDiscr::Relative(1));
    assert_eq!(signed.variants[3].discr, VariantDiscr::Relative(1));
    assert_eq!(signed.discriminants(&tcx, &prog).iter().map(|(_,d)| d.signed_value()).collect::<Vec<_>>(), vec![-2, -1, 126, 127]);
    for (i, d) in signed.discriminants(&tcx, &prog) { assert_eq!(signed.discriminant_for_variant(&tcx, &prog, i), Some(d)); }
    assert!(signed.check_discriminants(&tcx, &prog).is_empty());
    let wide = &tcx.adts[&named(&prog, "Wide")];
    assert_eq!(wide.discriminant_for_variant(&tcx, &prog, 1).unwrap().val, u128::MAX);
    assert!(wide.check_discriminants(&tcx, &prog).is_empty());
    let signed_wide = &tcx.adts[&named(&prog, "SignedWide")];
    assert_eq!(signed_wide.discriminant_for_variant(&tcx, &prog, 1).unwrap().signed_value(), i128::MIN + 1);
}

#[test]
fn round31_discriminant_errors_preserve_recovery_and_diagnostics() {
    let (prog, tcx) = program(br#"#![no_std]
        #[repr(u8)] enum Overflow { A = 255, B }
        #[repr(i8)] enum Signed { A = 127, B }
        #[repr(u8)] enum Duplicate { A = 2, B = 2, C = 2 }
        #[repr(u8)] enum Eval { A = 255u8 + 1u8, B }
        #[repr(u8)] enum Reset { A = 255, B = 4, C }
    "#);
    for name in ["Overflow", "Signed"] {
        let errors = tcx.adts[&named(&prog, name)].check_discriminants(&tcx, &prog);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("E0370"));
    }
    let errors = tcx.adts[&named(&prog, "Duplicate")].check_discriminants(&tcx, &prog);
    assert_eq!(errors.len(), 1); assert!(errors[0].contains("E0081"));
    let adt = &tcx.adts[&named(&prog, "Eval")];
    assert!(adt.check_discriminants(&tcx, &prog)[0].contains("E0080"));
    assert_eq!(adt.discriminant_for_variant(&tcx, &prog, 1).unwrap().val, 1);
    assert!(tcx.adts[&named(&prog, "Reset")].check_discriminants(&tcx, &prog).is_empty());
}
