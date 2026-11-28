use crate::const_eval::{ConstEvalProvider, EvalToValTreeResult};
use crate::consts::{Const, GenericArg, ScalarInt, ValTree};
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingEnv};
use std::sync::atomic::{AtomicUsize, Ordering};

fn program(source: &[u8]) -> (Program, Tcx) {
    let dir = std::env::temp_dir().join(format!("relax-pattern-analysis-{}-{}", std::process::id(), crate::unique_test_suffix()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, source).unwrap();
    let mut prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    prog.load_crate("pattern_analysis", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&prog.session, &[]), Vec::new()).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (prog, tcx)
}

fn named(prog: &Program, name: &str) -> DefId {
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        if prog.name(d) == name { return d; }
    }
    panic!("missing {name}")
}

/// The pattern errors of fn `f` (the messages after the location).
fn errors(source: &str, f: &str) -> Vec<String> {
    let mut src = String::from("#![no_std]\n");
    src.push_str(source);
    let (prog, tcx) = program(src.as_bytes());
    body_errors(&prog, &tcx, f)
}

fn body_errors(prog: &Program, tcx: &Tcx, f: &str) -> Vec<String> {
    let body = tcx.typeck(prog, named(prog, f));
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let r = super::check_body(prog, tcx, &body);
    r.errors.into_iter().chain(r.at_patterns).map(|e| e.splitn(4, ':').nth(3).unwrap_or("").trim().to_string()).collect()
}

fn with_const_eval_map<R>(
    tcx: Tcx,
    entries: Vec<(DefId, EvalToValTreeResult)>,
    check: impl FnOnce(&Tcx, &AtomicUsize) -> R,
) -> R {
    struct Provider<'a> {
        entries: &'a [(DefId, EvalToValTreeResult)],
        hits: &'a AtomicUsize,
    }
    fn evaluate(context: usize, _: TypingEnv, def: DefId, _: &[GenericArg]) -> EvalToValTreeResult {
        // The shared entries and counter outlive the Tcx using this callback.
        let provider = unsafe { &*(context as *const Provider<'_>) };
        provider.hits.fetch_add(1, Ordering::Relaxed);
        let (_, value) = provider.entries.iter().find(|(id, _)| *id == def)
            .unwrap_or_else(|| panic!("unexpected evaluation of {def:?}"));
        value.clone()
    }
    let hits = AtomicUsize::new(0);
    let provider = Provider { entries: &entries, hits: &hits };
    let mut tcx = tcx;
    tcx.const_eval_provider = Some(ConstEvalProvider { context: (&provider as *const Provider<'_>) as usize, evaluate });
    check(&tcx, &hits)
}

const ENUM: &str = "pub enum E { A, B(bool), C { x: u8 } }\n";

const EQUALITY_TRAITS: &str = r#"#![no_std]
    #[lang = "structural_peq"] trait StructuralPartialEq {}
    #[lang = "eq"] trait PartialEq<Rhs = Self> { fn eq(&self, other: &Rhs) -> bool; }
"#;

#[test]
fn exh_const_enum_with_unused_pointer_variant() {
    use crate::mir::build::pattern::tests::{scalar_branch, with_const_eval};
    let source = "pub enum State { Idle, Ptr(*const u8) }
        impl StructuralPartialEq for State {}
        impl PartialEq for State { fn eq(&self, _: &Self) -> bool { true } }
        pub const IDLE: State = State::Idle;
        pub fn f(s: State) -> u8 { match s { IDLE => 0, State::Ptr(_) => 1 } }";
    let (prog, tcx) = program(format!("{EQUALITY_TRAITS}{source}").as_bytes());
    let errors = with_const_eval(tcx, |tcx| Ok(scalar_branch(tcx, &[(crate::program::Prim::U32, 0)])), |tcx| body_errors(&prog, tcx, "f"));
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn const_enum_active_raw_ptr_rejected() {
    use crate::mir::build::pattern::tests::{unsupported_pointer, with_const_eval};
    let source = "pub enum Key { Number(u64), Ptr(*const u8) }
        impl StructuralPartialEq for Key {}
        impl PartialEq for Key { fn eq(&self, _: &Self) -> bool { true } }
        pub const BAD: Key = Key::Ptr(&0u8 as *const u8);
        pub fn f(k: Key) -> u8 { match k { BAD => 0, _ => 1 } }";
    let (prog, tcx) = program(format!("{EQUALITY_TRAITS}{source}").as_bytes());
    with_const_eval(tcx, unsupported_pointer, |tcx| {
        let errors = body_errors(&prog, tcx, "f");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("function pointers and raw pointers not derived from integers in patterns behave unpredictably and should not be relied upon"), "{errors:?}");
        assert_eq!(tcx.dcx.err_count(), 0, "unexpected constant evaluation diagnostic");
    });
}

#[test]
fn exh_enum_missing_variant() {
    let e = errors(&format!("{ENUM}pub fn f(e: E) -> u8 {{ match e {{ E::A => 0, E::B(true) => 1, E::C {{ .. }} => 2 }} }}"), "f");
    assert_eq!(e.len(), 1, "{:?}", e);
    assert!(e[0].starts_with("E0004: non-exhaustive patterns: `E::B(false)` not covered"), "{}", e[0]);
}

#[test]
fn exh_enum_complete() {
    let e = errors(&format!("{ENUM}pub fn f(e: &E) -> u8 {{ match e {{ E::A => 0, E::B(_) => 1, E::C {{ x: 0..=9 }} => 2, E::C {{ x: 10.. }} => 3 }} }}"), "f");
    assert!(e.is_empty(), "{:?}", e);
}

#[test]
fn exh_struct_field_witness() {
    let e = errors(&format!("{ENUM}pub fn f(e: E) -> u8 {{ match e {{ E::A | E::B(_) => 0, E::C {{ x: 1..=255 }} => 2 }} }}"), "f");
    assert_eq!(e.len(), 1, "{:?}", e);
    assert!(e[0].starts_with("E0004: non-exhaustive patterns: `E::C { x: 0_u8 }` not covered"), "{}", e[0]);
}

#[test]
fn exh_integer_ranges() {
    let e = errors("pub fn f(x: i32) -> u8 { match x { 1 => 0, 4 => 1 } }", "f");
    assert_eq!(e.len(), 1, "{:?}", e);
    assert!(e[0].starts_with("E0004: non-exhaustive patterns: `i32::MIN..=0_i32`, `2_i32..=3_i32` and `5_i32..=i32::MAX` not covered"), "{}", e[0]);
    let e = errors("pub fn f(x: u8) -> u8 { match x { 0..=127 => 0, 128..=255 => 1 } }", "f");
    assert!(e.is_empty(), "{:?}", e);
    let e = errors("pub fn f(x: usize) -> u8 { match x { 0..=18446744073709551615 => 0 } }", "f");
    assert!(e[0].contains("`usize::MAX..` not covered"), "{:?}", e);
}

#[test]
fn exh_bool_tuple_ref_slice() {
    let e = errors("pub fn f(x: (bool, bool)) -> u8 { match x { (true, _) => 0, (false, true) => 1 } }", "f");
    assert!(e[0].starts_with("E0004: non-exhaustive patterns: `(false, false)` not covered"), "{:?}", e);
    let e = errors("pub fn f(x: &[bool]) -> u8 { match x { [] => 0, [true, ..] => 1 } }", "f");
    assert!(e[0].starts_with("E0004: non-exhaustive patterns: `&[false, ..]` not covered"), "{:?}", e);
    let e = errors("pub fn f(x: [bool; 2]) -> u8 { match x { [true, _] => 0, [false, _] => 1 } }", "f");
    assert!(e.is_empty(), "{:?}", e);
    let e = errors("pub fn f(x: &bool) -> u8 { match x { &true => 0 } }", "f");
    assert!(e[0].starts_with("E0004: non-exhaustive patterns: `&false` not covered"), "{:?}", e);
}

#[test]
fn exh_guards_and_empty() {
    let e = errors("pub fn f(x: bool) -> u8 { match x { true => 0, false if x => 1 } }", "f");
    assert!(e[0].starts_with("E0004: non-exhaustive patterns: `false` not covered"), "{:?}", e);
    let e = errors("pub enum V {}\npub fn f(x: V) -> u8 { match x {} }", "f");
    assert!(e.is_empty(), "{:?}", e);
    let e = errors("pub fn f(x: u8) -> u8 { match x {} }", "f");
    assert!(e[0].starts_with("E0004: non-exhaustive patterns: type `u8` is non-empty"), "{:?}", e);
}

#[test]
fn exh_refutable_let_and_params() {
    let e = errors(&format!("{ENUM}pub fn f(e: E) {{ let E::A = e; }}"), "f");
    assert!(e[0].starts_with("E0005: refutable pattern in local binding -- patterns `E::B(_)` and `E::C { .. }` not covered"), "{:?}", e);
    let e = errors("pub fn f((a, 1): (u8, u8)) -> u8 { a }", "f");
    assert!(e[0].starts_with("E0005: refutable pattern in function argument"), "{:?}", e);
    let e = errors("pub struct S { a: u8, b: bool }\npub fn f(S { a, b: _ }: S, (x, y): (u8, u8)) -> u8 { let [p, q] = [a, x]; p + q + y }", "f");
    assert!(e.is_empty(), "{:?}", e);
    let e = errors("pub fn f(v: &[u8]) -> u8 { let [a, b] = v else { return 0 }; *a + *b }", "f");
    assert!(e.is_empty(), "{:?}", e);
}

#[test]
fn exh_or_patterns_and_bindings() {
    let e = errors(&format!("{ENUM}pub fn f(e: Option2) -> u8 {{ match e {{ Option2::S(E::A | E::B(true)) | Option2::N => 0, Option2::S(x @ (E::B(false) | E::C {{ .. }})) => {{ let _ = x; 1 }} }} }}\npub enum Option2 {{ S(E), N }}"), "f");
    assert!(e.is_empty(), "{:?}", e);
}

#[test]
fn exh_char_and_constants() {
    let e = errors("pub fn f(c: char) -> u8 { match c { '\\0'..='\\u{D7FF}' => 0, '\\u{E000}'..='\\u{10FFFF}' => 1 } }", "f");
    assert!(e.is_empty(), "{:?}", e);
    let e = errors("const T: bool = true;\npub fn f(b: bool) -> u8 { match b { T => 0, false => 1 } }", "f");
    assert!(e.is_empty(), "{:?}", e);
    let e = errors("const LO: u8 = 10;\npub fn f(b: u8) -> u8 { match b { 0..LO => 0, LO..=255 => 1 } }", "f");
    assert!(e.is_empty(), "{:?}", e);
}

#[test]
fn exh_const_pattern_aliased_declared_types() {
    let (prog, tcx) = program(br#"#![no_std]
        trait Tr { type A; }
        impl Tr for () { type A = bool; }
        const T: <() as Tr>::A = true;
        pub fn f_bool(b: bool) -> u8 { match b { T => 0, false => 1 } }
        pub fn f_alias(b: <() as Tr>::A) -> u8 { match b { T => 0, false => 1 } }
        pub fn f_missing(b: bool) -> u8 { match b { T => 0 } }
    "#);
    for name in ["f_bool", "f_alias"] {
        let errors = body_errors(&prog, &tcx, name);
        assert!(errors.is_empty(), "{name}: {errors:?}");
    }
    let errors = body_errors(&prog, &tcx, "f_missing");
    assert_eq!(errors.len(), 1, "f_missing: {errors:?}");
    assert!(errors[0].starts_with("E0004: non-exhaustive patterns: `false` not covered"), "{errors:?}");
    assert_eq!(tcx.dcx.err_count(), 0, "unexpected diagnostic evaluating the projection");

    let (prog, tcx) = program(br#"#![no_std]
        const N: usize = 1;
        const T: [bool; N] = [true];
        const F: [bool; N] = [false];
        pub fn f_arr(x: [bool; N]) -> u8 { match x { T => 0, F => 1 } }
        pub fn f_arr_missing(x: [bool; N]) -> u8 { match x { T => 0 } }
    "#);
    let entries = [("T", true), ("F", false)].iter().map(|&(name, value)| {
        let tys = &tcx.tys;
        let leaf = ValTree::from_scalar_int(tys, ScalarInt::try_from_uint(value as u128, 1).unwrap());
        let array = ValTree::from_branches(tys, &[Const::new_value(tys, leaf, tys.bool_)]);
        (named(&prog, name), Ok(array))
    }).collect();
    with_const_eval_map(tcx, entries, |tcx, hits| {
        let errors = body_errors(&prog, tcx, "f_arr");
        assert!(errors.is_empty(), "f_arr: {errors:?}");
        assert_eq!(hits.load(Ordering::Relaxed), 2, "only T and F need the provider");
        let errors = body_errors(&prog, tcx, "f_arr_missing");
        assert_eq!(errors.len(), 1, "f_arr_missing: {errors:?}");
        assert!(errors[0].starts_with("E0004: non-exhaustive patterns: `[false]` not covered"), "{errors:?}");
        assert_eq!(tcx.dcx.err_count(), 0, "unexpected diagnostic evaluating the arrays");
    });
}

#[test]
fn const_pattern_generic_and_unnormalized_aliases_stay_opaque() {
    let (prog, tcx) = program(br#"#![no_std]
        pub enum Opt<T> { None, Some(T) }
        pub trait Tr { type A; }
        pub struct S<T>(T);
        impl<T> Tr for S<T> { type A = Opt<T>; }
        impl<T> S<T> { pub const NONE: <S<T> as Tr>::A = Opt::None; }
        pub trait Tr2 { type A; const C: Self::A; }
        pub fn f_param<T>(x: <S<T> as Tr>::A) -> u8 { match x { S::<T>::NONE => 0, Opt::Some(_) => 1 } }
        pub fn f_rigid<T: Tr2>(x: T::A) -> u8 { match x { T::C => 0 } }
        pub fn f_bound<T: Tr2<A = bool>>(b: bool) -> u8 { match b { T::C => 0, false => 1 } }
    "#);
    with_const_eval_map(tcx, Vec::new(), |tcx, hits| {
        for (name, witness) in [("f_param", "Opt::None"), ("f_rigid", "_"), ("f_bound", "true")] {
            let errors = body_errors(&prog, tcx, name);
            assert_eq!(errors.len(), 1, "{name}: {errors:?}");
            let expected = format!("E0004: non-exhaustive patterns: `{witness}` not covered");
            assert!(errors[0].starts_with(&expected), "{name}: {errors:?}");
        }
        assert_eq!(hits.load(Ordering::Relaxed), 0, "opaque patterns reached the provider");
        assert_eq!(tcx.dcx.err_count(), 0, "unexpected constant evaluation diagnostic");
    });
}
