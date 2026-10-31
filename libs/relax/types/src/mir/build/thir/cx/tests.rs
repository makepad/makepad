//! THIR construction tests: real sources through typeck, then `thir_body`; the shape is checked by
//! the kinds of expressions the arenas hold.
use super::Cx;
use crate::borrowck::body::{BodyKind, CheckedBody};
use crate::mir::build::pattern::tests::{named, program};
use crate::mir::build::thir::{ExprKind, ThirBody};
use crate::mir::build::{BuildCx, BuildResult, BuildRoot};
use crate::tcx::{EvalCx, EvalState, TypingEnv};

fn build<R>(source: &str, name: &str, f: impl FnOnce(BuildResult<ThirBody>) -> R) -> R {
    let (prog, tcx) = program(source);
    let def = named(&prog, name);
    let body = tcx.typeck(&prog, def);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let env = TypingEnv::fully_monomorphized(&prog);
    let state = EvalState::new();
    let eval = EvalCx::new(&prog, &tcx, env, &state);
    let root = BuildRoot::new(BuildCx { prog: &prog, tcx: &tcx, env, eval: &eval }, &body).unwrap();
    let cx = Cx::new(&root, CheckedBody { owner: def, kind: if name.chars().next().unwrap().is_uppercase() { BodyKind::Const } else { BodyKind::Function } });
    f(cx.thir_body())
}

fn kinds(source: &str, name: &str) -> Vec<String> {
    build(source, name, |r| {
        let t = r.unwrap_or_else(|e| panic!("{e:?}"));
        let mut out: Vec<String> = t
            .thir
            .exprs
            .iter()
            .map(|e| {
                let s = format!("{:?}", e.kind);
                s.split(|c: char| !c.is_alphanumeric()).next().unwrap().to_string()
            })
            .collect();
        out.sort();
        out.dedup();
        out
    })
}

fn has(k: &[String], names: &[&str]) {
    for n in names {
        assert!(k.iter().any(|x| x == n), "missing {n} in {k:?}");
    }
}

#[test]
fn arithmetic_and_calls() {
    let k = kinds("pub fn g(a: u8) -> u8 { a } pub fn f(x: u8, y: u8) -> u8 { let z = x + y; g(z) }", "f");
    has(&k, &["Binary", "Call", "VarRef", "Scope", "ZstLiteral"]);
}

#[test]
fn loops_branches_and_returns() {
    let k = kinds("pub fn f(mut n: u32) -> u32 { while n > 3 { n -= 1; if n == 5 { break; } } loop { return n; } }", "f");
    has(&k, &["Loop", "If", "Break", "Return", "AssignOp", "Binary"]);
}

#[test]
fn match_and_references() {
    let k = kinds("pub enum E { A(u8), B } pub fn f(e: &E) -> u8 { match e { E::A(n) => *n, E::B => 0 } }", "f");
    has(&k, &["Match", "Deref"]);
}

#[test]
fn struct_tuple_array_field_index() {
    let k = kinds("pub struct S { a: u8, b: [u8; 2] } pub fn f(s: S) -> u8 { let t = (s.a, 1u8); let S { a, b } = S { a: t.0, b: [t.1, 2] }; b[1] + a }", "f");
    has(&k, &["Adt", "Tuple", "Array", "Field", "Index"]);
}

#[test]
fn closures_capture() {
    let k = kinds("pub fn f(x: u8) -> u8 { let _c = |y: u8| x + y; x }", "f");
    has(&k, &["Closure"]);
}

#[test]
fn borrows_and_casts() {
    let k = kinds("pub fn f(x: u8) -> u64 { let r = &x; let m = &mut 3u8; *m += *r; (x as u64) + 1 }", "f");
    has(&k, &["Borrow", "Cast", "Deref"]);
}

#[test]
fn fn_body_fields_are_filled() {
    build("pub fn f(x: u8, y: u8) -> u8 { x }", "f", |r| {
        let t = r.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(t.params.len(), 2);
        assert!(t.fn_scopes.is_some());
        assert!(t.coroutine.is_none());
    });
}

#[test]
fn const_body_has_no_fn_scopes() {
    build("pub const K: u8 = 1 + 2; pub fn f() {}", "K", |r| {
        let t = r.unwrap_or_else(|e| panic!("{e:?}"));
        assert!(t.params.is_empty());
        assert!(t.fn_scopes.is_none());
    });
}

#[test]
fn destructuring_assignment() {
    let k = kinds(
        "pub struct S { a: u8, b: u8 } pub struct T(u8, u8) ; pub enum E { V(u8, u8) } pub fn f(s: S, t: T, mut a: u8, mut b: u8, mut arr: [u8; 3]) { (a, b) = (b, a); S { a, b } = s; T(a, ..) = t; (a, _) = (1, 2); [a, .., b] = arr; (a, (b, _)) = (1, (2, 3)); let _ = E::V(1, 2); }",
        "f",
    );
    has(&k, &["Block", "Assign", "VarRef", "Tuple"]);
}

#[test]
fn destructuring_assignment_binds_in_a_remainder_scope() {
    build("pub fn f(mut a: u8, mut b: u8) { (a, b) = (b, a); }", "f", |r| {
        let t = r.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(t.thir.var_scopes.len(), 2);
    });
}

#[test]
fn default_field_values() {
    let k = kinds("pub struct S { a: u8 = 7, c: u8 } pub fn f() -> S { S { c: 1, .. } }", "f");
    has(&k, &["Adt", "NamedConst"]);
}

/// A program of `source` linked against the real `core`, for the desugarings that need lang items.
fn core_program(source: &str) -> (crate::program::Program, crate::tcx::Tcx) {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = crate::program::Program::new(sess);
    let core_root = concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/core/lib.rs");
    prog.load_crate("core", core_root, 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    let core = prog.syms.intern("core");
    let dir = std::env::temp_dir().join(format!("relax-thir-cx-{}-{}", std::process::id(), crate::unique_test_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, source).unwrap();
    prog.load_crate("thir_cx", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0)]).unwrap();
    prog.prelude_crate = Some(0);
    prog.resolve_imports();
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    (prog, tcx)
}

/// `thir_body` of `owner` (a function, or the closure/async block `kind`) in a program with core.
fn core_thir<R>(source: &str, name: &str, kind: impl FnOnce(&crate::typeck::Body, &crate::program::Program) -> BodyKind, f: impl FnOnce(BuildResult<ThirBody>) -> R) -> R {
    let (prog, tcx) = core_program(source);
    let def = named(&prog, name);
    let body = tcx.typeck(&prog, def);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let env = TypingEnv::fully_monomorphized(&prog);
    let state = EvalState::new();
    let eval = EvalCx::new(&prog, &tcx, env, &state);
    let root = BuildRoot::new(BuildCx { prog: &prog, tcx: &tcx, env, eval: &eval }, &body).unwrap();
    let kind = kind(&body, &prog);
    let cx = Cx::new(&root, CheckedBody { owner: def, kind });
    f(cx.thir_body())
}

fn core_kinds(source: &str, name: &str) -> Vec<String> {
    core_thir(source, name, |_, _| BodyKind::Function, |r| {
        let t = r.unwrap_or_else(|e| panic!("{e:?}"));
        let mut out: Vec<String> = t.thir.exprs.iter().map(|e| format!("{:?}", e.kind).split(|c: char| !c.is_alphanumeric()).next().unwrap().to_string()).collect();
        out.sort();
        out.dedup();
        out
    })
}

#[test]
fn for_loop_desugars_to_into_iter_and_next() {
    let k = core_kinds("pub fn thir_cx_probe(v: [u8; 3]) -> u8 { let mut s = 0; for x in v { s += x; } s }", "thir_cx_probe");
    has(&k, &["Match", "Loop", "Call", "Borrow", "Break"]);
}

#[test]
fn question_mark_desugars_to_branch_and_from_residual() {
    let k = core_kinds("pub fn thir_cx_probe(x: Option<u8>) -> Option<u8> { let y = x?; Some(y) }", "thir_cx_probe");
    has(&k, &["Match", "Return", "Call"]);
}

#[test]
fn matches_and_asserts() {
    let k = core_kinds("pub fn thir_cx_probe(x: Option<u8>) -> bool { assert!(true); matches!(x, Some(n) if n > 1) }", "thir_cx_probe");
    has(&k, &["Match", "If"]);
}

#[test]
fn async_block_aggregate() {
    let k = core_kinds("pub fn thir_cx_probe(x: u8) { let _g = async move { x }; }", "thir_cx_probe");
    has(&k, &["Closure"]);
}

fn async_block(source: &str) -> BuildResult<ThirBody> {
    core_thir(
        source,
        "thir_cx_probe",
        |body, prog| {
            let ast = &prog.files[body.file as usize].ast;
            let e = (0..body.expr_ty.len()).map(|i| body.expr_at(i)).find(|&e| matches!(ast.expr(e).kind, crate::ast::ExprKind::Async(..))).expect("an async block");
            BodyKind::Closure(e)
        },
        |r| r,
    )
}

#[test]
fn async_block_body_is_a_coroutine_with_a_resume_argument() {
    let t = async_block("pub fn thir_cx_probe(x: u8) { let _g = async move { x }; }").unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(t.params.len(), 2);
    let c = t.coroutine.expect("coroutine info");
    assert!(matches!(c.kind, crate::mir::CoroutineKind::Desugared(crate::mir::CoroutineDesugaring::Async, crate::mir::CoroutineSource::Block)));
    assert!(t.fn_scopes.is_some());
}

#[test]
fn await_polls_in_a_loop_and_yields() {
    let t = async_block("pub async fn g() -> u8 { 1 } pub fn thir_cx_probe() { let _g = async move { g().await }; }").unwrap_or_else(|e| panic!("{e:?}"));
    let k: Vec<String> = t.thir.exprs.iter().map(|e| format!("{:?}", e.kind).split(|c: char| !c.is_alphanumeric()).next().unwrap().to_string()).collect();
    for n in ["Yield", "Loop", "Match", "Break", "Call"] {
        assert!(k.iter().any(|x| x == n), "missing {n}");
    }
}

#[test]
fn async_fn_is_a_wrapper_building_its_body_coroutine() {
    core_thir("pub async fn thir_cx_probe(x: u8, y: u8) -> u8 { let z = x + y; z }", "thir_cx_probe", |_, _| BodyKind::Function, |r| {
        let t = r.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(t.params.len(), 2);
        let ExprKind::Closure(c) = &t.thir.exprs[t.expr.0 as usize].kind else { panic!("{:?}", t.thir.exprs[t.expr.0 as usize].kind) };
        assert_eq!(c.upvars.len(), 2);
        assert!(t.coroutine.is_none());
    });
}

#[test]
fn async_fn_body_is_the_coroutine() {
    core_thir(
        "pub async fn thir_cx_probe(x: u8, y: u8) -> u8 { let z = x + y; z }",
        "thir_cx_probe",
        |body, _| BodyKind::Closure(crate::ast::ExprId(crate::tcx::ASYNC_FN_BODY | body.body.unwrap().0)),
        |r| {
            let t = r.unwrap_or_else(|e| panic!("{e:?}"));
            assert_eq!(t.params.len(), 2);
            let c = t.coroutine.expect("coroutine");
            assert!(matches!(c.kind, crate::mir::CoroutineKind::Desugared(crate::mir::CoroutineDesugaring::Async, crate::mir::CoroutineSource::Fn)));
            // the parameters are upvars, `z` is a local
            assert!(t.thir.exprs.iter().any(|e| matches!(e.kind, ExprKind::UpvarRef { .. })));
            assert!(t.thir.exprs.iter().any(|e| matches!(e.kind, ExprKind::VarRef { .. })));
        },
    );
}

#[test]
fn async_closure_is_a_coroutine_closure_creating_its_coroutine() {
    let k = core_kinds("pub fn thir_cx_probe(a: u8) { let _c = async |x: u8| { x + a }; }", "thir_cx_probe");
    has(&k, &["Closure"]);
}

/// `buf[..2].fill(0)` on `&mut [u8]`: the range index is a mutable place, so typeck records
/// `IndexMut::index_mut` for it (and the std `Index` impl in `std_index`).
#[test]
fn range_index_in_a_mutable_place_is_index_mut() {
    crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(Some(true)));
    let (prog, tcx) = core_program("pub fn thir_cx_probe(buf: &mut [u8]) { buf[..2].fill(0); }");
    let body = tcx.typeck(&prog, named(&prog, "thir_cx_probe"));
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    assert_eq!(body.std_index.len(), 1);
    assert_eq!(body.mutable_index.len(), 1, "std_index: {:?}", body.std_index);
}
