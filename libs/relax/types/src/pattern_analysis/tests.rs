use crate::program::{DefId, Program};
use crate::tcx::Tcx;

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
    let body = tcx.typeck(&prog, named(&prog, f));
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let r = super::check_body(&prog, &tcx, &body);
    r.errors.into_iter().chain(r.at_patterns).map(|e| e.splitn(4, ':').nth(3).unwrap_or("").trim().to_string()).collect()
}

const ENUM: &str = "pub enum E { A, B(bool), C { x: u8 } }\n";

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
