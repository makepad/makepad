//! Function header identity and coercion regressions.
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, EvalCx, EvalState, TypingEnv};
use crate::types::{Abi, FnHeader, Safety, TyKind};

fn collect(source: &str) -> (Program, Tcx) {
    let sess = &std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("relax-fnhdr-{}-{n}.rs", std::process::id()));
    std::fs::write(&root, source).unwrap();
    let mut prog = Program::new(sess);
    prog.load_crate("custom", root.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    std::fs::remove_file(root).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    (prog, tcx)
}

fn named(prog: &Program, name: &str) -> DefId {
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        if prog.name(d) == name { return d; }
    }
    panic!("missing {name}")
}

#[test]
fn fnhdr_identity_relations_and_folding() {
    let (prog, tcx) = collect(r#"
        fn probe<T>(a: fn(T), b: unsafe fn(T), c: extern "C" fn(T),
            d: extern "C-unwind" fn(T), e: extern "system" fn(T),
            f: extern "rust-call" fn(T), g: unsafe extern "C" fn(T, ...),
            h: unsafe extern "C" fn(T)) {}
        fn binder(f: for<'a> unsafe extern "C" fn(&'a u32)) {}
    "#);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let ps = &tcx.fn_sig(&prog, named(&prog, "probe")).unwrap().params;
    let state = EvalState::new();
    let cx = EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
    for i in 0..ps.len() {
        for j in 0..ps.len() {
            assert_eq!(ps[i] == ps[j], i == j);
            assert_eq!(crate::stable::type_hash(&prog, &tcx, ps[i]) == crate::stable::type_hash(&prog, &tcx, ps[j]), i == j);
            let infcx = crate::infer::InferCtxt::new(&tcx, cx.typing_env.mode, cx.typing_env.limit);
            assert_eq!(infcx.at(&prog, Default::default()).eq(ps[i], ps[j]).is_ok(), i == j);
        }
        let header = match tcx.tys.kind(ps[i]) { TyKind::FnPtr(_, _, h, _) => *h, _ => panic!() };
        let folded = tcx.tys.subst(ps[i], &[tcx.tys.unit]);
        assert!(matches!(tcx.tys.kind(folded), TyKind::FnPtr(_, _, h, _) if *h == header));
        assert_eq!(header.is_fn_trait_compatible(), i == 0);
    }
    let t = tcx.fn_sig(&prog, named(&prog, "binder")).unwrap().params[0];
    let erased = tcx.tys.erase_regions(t);
    assert!(matches!(tcx.tys.kind(erased), TyKind::FnPtr(_, _, FnHeader { safety: Safety::Unsafe, abi: Abi::C, c_variadic: false }, 1)));
}

#[test]
fn fnhdr_foreign_and_definition_headers() {
    let (prog, tcx) = collect(r#"
        extern "C-unwind" { fn foreign(x: u32, ...); safe fn declared_safe(); }
        extern "Rust" { fn rust_foreign(); }
        extern "system" fn system() {}
        unsafe extern "rust-call" fn rust_call(args: (u32,)) {}
    "#);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    assert_eq!(tcx.fn_sig(&prog, named(&prog, "foreign")).unwrap().header, FnHeader { safety: Safety::Unsafe, abi: Abi::CUnwind, c_variadic: true });
    assert_eq!(tcx.fn_sig(&prog, named(&prog, "declared_safe")).unwrap().header.safety, Safety::Safe);
    assert_eq!(tcx.fn_sig(&prog, named(&prog, "rust_foreign")).unwrap().header.abi, Abi::Rust);
    assert_eq!(tcx.fn_sig(&prog, named(&prog, "system")).unwrap().header.abi, Abi::System);
    assert_eq!(tcx.fn_sig(&prog, named(&prog, "rust_call")).unwrap().header, FnHeader { safety: Safety::Unsafe, abi: Abi::RustCall, c_variadic: false });
}

fn check(source: &str, reject: bool) {
    let (prog, tcx) = collect(source);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let state = EvalState::new();
    let cx = EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
    let body = crate::typeck::instantiate_body(&cx, &prog, &tcx, named(&prog, "probe"), &[]);
    assert_eq!(!body.errors.is_empty(), reject, "{:?}", body.errors);
    if reject {
        assert!(body.errors.iter().any(|e| e.contains("mismatched types") || e.contains("E0061")), "{:?}", body.errors);
    }
}

#[test]
fn fnhdr_coercions() {
    check("fn safe() {} fn probe() { let f: unsafe fn() = safe; let s: fn() = safe; let u: unsafe fn() = s; let c: unsafe fn() = || {}; }", false);
    check("unsafe fn danger() {} fn probe() { let f: fn() = danger; }", true);
    check("fn probe(f: unsafe fn()) { let s: fn() = f; }", true);
    check("unsafe fn danger() {} fn probe() { let f = danger as fn(); }", true);
    check("fn probe(f: unsafe fn()) { let s = f as fn(); }", true);
    check("fn safe() {} unsafe fn danger() {} fn probe(b: bool) { let f = if b { safe } else { danger }; let u: unsafe fn() = f; }", false);
    check("fn probe(f: unsafe extern \"C\" fn(u32, ...)) { unsafe { f(); } }", true);
    check("fn probe() { let f: extern \"C\" fn() = || {}; }", true);
    check("extern \"C\" fn c() {} fn probe() { let f: fn() = c; }", true);
    check("fn probe(f: extern \"C-unwind\" fn()) { let s: extern \"C\" fn() = f; }", true);
    check("extern \"C\" { fn var(x: u32, ...); } fn probe() { let f: unsafe extern \"C\" fn(u32, ...) = var; unsafe { f(1, 2u32); } }", false);
    check("extern \"C\" { fn var(x: u32, ...); } fn probe() { let f: unsafe extern \"C\" fn(u32) = var; }", true);
}

#[test]
fn fnhdr_unsupported_abi() {
    for source in ["fn probe(f: extern \"not-an-abi\" fn()) {}", "extern \"not-an-abi\" fn probe() {}", "extern \"not-an-abi\" { fn probe(); }"] {
        let (prog, tcx) = collect(source);
        let errors = tcx.signature_errors(&prog, named(&prog, "probe"));
        assert!(errors.iter().any(|e| e.contains("unsupported function ABI")), "{errors:?}");
    }
}
