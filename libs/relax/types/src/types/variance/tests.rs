use super::*;
use crate::program::Program;

fn collect(source: &str) -> (Program, Tcx) {
    let sess = &std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("relax-variance-{}-{n}.rs", std::process::id()));
    std::fs::write(&root, source).unwrap();
    let mut prog = Program::new(sess);
    prog.load_crate("custom", root.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    std::fs::remove_file(root).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (prog, tcx)
}

fn named(prog: &Program, name: &str) -> DefId {
    (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| prog.name(d) == name).unwrap()
}

fn check(prog: &Program, tcx: &Tcx, name: &str, expected: &[V]) {
    assert_eq!(tcx.variances_of(prog, named(prog, name)), expected, "{name}");
}

#[test]
fn lattice() {
    use V::*;
    let all = [Bi, Co, Contra, Inv];
    let composition = [[Bi, Bi, Bi, Bi], [Bi, Co, Contra, Inv],
        [Bi, Contra, Co, Inv], [Inv, Inv, Inv, Inv]];
    let meet = [[Bi, Co, Contra, Inv], [Co, Co, Inv, Inv],
        [Contra, Inv, Contra, Inv], [Inv, Inv, Inv, Inv]];
    for (i, &a) in all.iter().enumerate() {
        for (j, &b) in all.iter().enumerate() {
            assert_eq!(xform(a, b), composition[i][j]);
            assert_eq!(join(a, b), meet[i][j]);
        }
    }
}

#[test]
fn fields_pointers_and_binders() {
    let (p, t) = collect(r#"
        struct Co<T>(T);
        struct Contra<T>(fn(T));
        struct Inv<T>(fn(T) -> T);
        struct Bi<T>;
        struct Mut<'a, T>(&'a mut T);
        struct Raw<T, U>(*mut T, *const U);
        struct Higher<'a, T>(for<'b> fn(&'b T, &'a T));
        struct Double<T>(fn(fn(T)));
        struct Nested<'a, 'b, T>(&'a mut &'b T);
        struct Mixed<'a, T, const N: usize, U>(&'a T, [U; N]);
        struct Forward<'a, T, const N: usize, U>(Mixed<'a, T, N, U>);
        union Storage<T> { value: T }
        enum Choice<T, U> { A(T), B(fn(U)) }
    "#);
    for (name, vs) in [
        ("Co", vec![V::Co]), ("Contra", vec![V::Contra]), ("Inv", vec![V::Inv]),
        ("Bi", vec![V::Bi]), ("Mut", vec![V::Co, V::Inv]),
        ("Raw", vec![V::Inv, V::Co]), ("Higher", vec![V::Contra, V::Contra]),
        ("Double", vec![V::Co]), ("Nested", vec![V::Co, V::Inv, V::Inv]),
        ("Mixed", vec![V::Co, V::Co, V::Inv, V::Co]),
        ("Forward", vec![V::Co, V::Co, V::Inv, V::Co]),
        ("Storage", vec![V::Co]), ("Choice", vec![V::Co, V::Contra]),
    ] { check(&p, &t, name, &vs); }
    let indices = &t.adts[&named(&p, "Mixed")].generic_indices;
    assert_eq!(indices.lifetimes, [0]);
    assert_eq!(indices.types_consts, [1, 2, 3]);
    assert_eq!(indices.consts, [2]);
}

#[test]
fn lang_items_are_identities() {
    let (p, t) = collect(r#"
        #[lang = "phantom_data"] struct Marker<T>;
        #[lang = "unsafe_cell"] struct Interior<T>(T);
        struct PhantomData<T>;
        struct UnsafeCell<T>(T);
        struct Co<'a, T>(Marker<&'a T>);
        struct Inv<'a, T>(Interior<&'a T>);
        struct Contra<T>(Marker<fn(T)>);
    "#);
    check(&p, &t, "Marker", &[V::Co]);
    check(&p, &t, "Interior", &[V::Inv]);
    check(&p, &t, "PhantomData", &[V::Bi]);
    check(&p, &t, "UnsafeCell", &[V::Co]);
    check(&p, &t, "Co", &[V::Co, V::Co]);
    check(&p, &t, "Inv", &[V::Inv, V::Inv]);
    check(&p, &t, "Contra", &[V::Contra]);
}

#[test]
fn recursion_converges_beyond_four_rounds() {
    let mut src = String::new();
    for i in 0..12 {
        src.push_str(&format!("struct Link{i}<T>(*const Link{}<T>{});\n", (i + 1) % 12,
            if i == 11 { ", T" } else { "" }));
    }
    src.push_str(r#"
        struct Unused<T>(*const Unused<T>);
        struct Contra<T>(fn(T), *const Contra<T>);
        struct Flip<T>(fn(Flip<T>), T);
        struct Left<T>(*const Right<T>, T);
        struct Right<T>(fn(Left<T>));
    "#);
    let (p, t) = collect(&src);
    for i in 0..12 { check(&p, &t, &format!("Link{i}"), &[V::Co]); }
    check(&p, &t, "Unused", &[V::Bi]);
    check(&p, &t, "Contra", &[V::Contra]);
    for name in ["Flip", "Left", "Right"] { check(&p, &t, name, &[V::Inv]); }
}

#[test]
fn signatures_and_parent_indices() {
    let (p, t) = collect(r#"
        fn input<T>(x: T) {}
        fn output<T>() -> T { loop {} }
        fn both<T>(x: T) -> T { x }
        fn unused<T, const N: usize>() {}
        fn bound<'a, T>(x: &'a T) {}
        fn early<'a: 'a, T, const N: usize, U>(x: &'a T) -> U { loop {} }
        struct Owner<'a, T>(&'a T);
        impl<'a, T> Owner<'a, T> {
            fn method<'b: 'b, U>(&'b self, x: U) -> &'a T { self.0 }
        }
        impl<T> Owner<'_, T> {
            fn anonymous<'b: 'b, U>(&'b self, x: U) {}
        }
    "#);
    check(&p, &t, "input", &[V::Contra]);
    check(&p, &t, "output", &[V::Co]);
    check(&p, &t, "both", &[V::Inv]);
    check(&p, &t, "unused", &[V::Inv, V::Inv]);
    check(&p, &t, "bound", &[V::Contra]);
    check(&p, &t, "early", &[V::Contra, V::Contra, V::Inv, V::Co]);
    check(&p, &t, "method", &[V::Inv, V::Inv, V::Contra, V::Contra]);
    let indices = &t.fn_sig(&p, named(&p, "method")).unwrap().generic_indices;
    assert_eq!(indices.lifetimes, [0, 2]);
    assert_eq!(indices.types_consts, [1, 3]);
    check(&p, &t, "anonymous", &[V::Contra, V::Contra, V::Contra, V::Contra]);
    let indices = &t.fn_sig(&p, named(&p, "anonymous")).unwrap().generic_indices;
    assert_eq!(indices.lifetimes, [1, 2]);
    assert_eq!(indices.types_consts, [0, 3]);
}

#[test]
fn relax_core_declarations() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut p = Program::new(sess);
    p.load_crate("core", concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/core/lib.rs"),
        2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    p.prelude_crate = Some(0);
    p.resolve_imports();
    let mut t = Tcx::new();
    t.collect(&p);
    for (&d, adt) in &t.adts {
        assert_eq!(t.variances_of(&p, d).len(), adt.generic_indices.len());
    }
    for d in (0..p.defs.len() as u32).map(DefId) {
        let Some(sig) = t.fn_sig(&p, d) else { continue; };
        assert_eq!(t.variances_of(&p, d).len(), sig.generic_indices.len());
    }
    assert_eq!(t.variances_of(&p, t.lang_items.get(LangItem::PhantomData).unwrap()), &[V::Co]);
    assert_eq!(t.variances_of(&p, t.lang_items.get(LangItem::UnsafeCell).unwrap()), &[V::Inv]);
}

#[test]
fn publication_invalidates_dependents() {
    let (p, mut t) = collect("struct A<T>(T); struct B<T>(A<T>);");
    check(&p, &t, "B", &[V::Co]);
    let param = t.tys.intern(TyKind::Param(0));
    let mutable = t.tys.intern(TyKind::Ptr(true, param));
    t.adts.get_mut(&named(&p, "A")).unwrap().variants[0].fields[0].ty = mutable;
    t.publish_collected();
    check(&p, &t, "B", &[V::Inv]);
}

#[test]
fn lazy_signatures_publish_complete_bounds_to_parallel_readers() {
    let (p, t) = collect(r#"
        trait Bound<A> { type Item; }
        struct Holder<T>(T);
        fn used<T: Bound<u8, Item = u16>>(x: T) -> T { x }
        fn untouched<T>(x: T) -> T { x }
    "#);
    let used = named(&p, "used");
    assert_eq!(t.signature_count(), 0);
    // An ADT variance query must not pull in any function declarations.
    assert_eq!(t.variances_of(&p, named(&p, "Holder")), &[V::Co]);
    assert_eq!(t.signature_count(), 0);
    let gate = std::sync::Barrier::new(8);
    let addresses = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8).map(|_| scope.spawn(|| {
            gate.wait();
            let sig = t.fn_sig(&p, used).unwrap();
            assert_eq!(sig.n_generics, 1);
            assert_eq!(t.fn_trait_bounds(&p, used).unwrap().len(), 1);
            assert_eq!(t.clauses_of(&p, used).iter().filter(|p| matches!(p.kind, crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::Projection(..)))).count(), 1);
            assert_eq!(t.variances_of(&p, used), &[V::Inv]);
            sig as *const crate::tcx::FnSigT as usize
        })).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>()
    });
    assert!(addresses.iter().all(|a| *a == addresses[0]));
    assert_eq!(t.signature_count(), 1);
}

#[test]
fn lazy_signature_lengths_use_native_const_queries() {
    let (p, t) = collect("const LEN: usize = 4; fn uses(x: [u8; LEN]) -> [u8; LEN] { x } fn untouched() {}");
    assert_eq!(t.signature_count(), 0);
    let sig = t.fn_sig(&p, named(&p, "uses")).unwrap();
    for ty in [sig.params[0], sig.ret] {
        let TyKind::Array(_, n) = t.tys.kind(ty) else { panic!("array") };
        let crate::consts::ConstKind::Alias(_, alias) = t.tys.const_kind(*n) else { panic!("unevaluated length") };
        let value = t.const_eval_resolve(&p, crate::tcx::TypingEnv::fully_monomorphized(&p), alias.kind.def_id(), &alias.args).unwrap();
        assert_eq!(value.try_to_leaf().unwrap().to_bits(8), 4);
    }
    assert_eq!(t.signature_count(), 1);
}

#[test]
fn lazy_signature_diagnostics_survive_first_reader_and_reload() {
    let (p, mut t) = collect("extern \"not-an-abi\" fn bad() {} fn untouched() {}");
    let bad = named(&p, "bad");
    assert_eq!(t.signature_count(), 0);
    t.fn_sig(&p, bad).unwrap();
    assert!(t.signature_errors(&p, bad).iter().any(|e| e.contains("E0703: invalid ABI")));
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&p, &t, crate::tcx::TypingEnv::fully_monomorphized(&p), &state);
    let body = crate::typeck::instantiate_body(&cx, &p, &t, bad, &[]);
    assert!(body.errors.iter().any(|e| e.contains("E0703: invalid ABI")));
    assert_eq!(t.signature_count(), 1);
    t.collect_added(&p, &[bad], false);
    assert_eq!(t.signature_count(), 0);
    assert!(t.signature_errors(&p, bad).iter().any(|e| e.contains("E0703: invalid ABI")));
}

#[test]
fn lazy_foreign_signature_errors_reach_every_caller() {
    let (p, t) = collect(r#"
        extern "not-an-abi" { fn foreign(); }
        fn first() { unsafe { foreign(); } }
        fn second() { unsafe { foreign(); } }
        fn untouched() {}
    "#);
    let state = crate::tcx::EvalState::new();
    let cx = crate::tcx::EvalCx::new(&p, &t, crate::tcx::TypingEnv::fully_monomorphized(&p), &state);
    for name in ["first", "second"] {
        let body = crate::typeck::instantiate_body(&cx, &p, &t, named(&p, name), &[]);
        assert!(body.errors.iter().any(|e| e.contains("E0703: invalid ABI")), "{:?}", body.errors);
    }
    assert_eq!(t.signature_count(), 3);
}
