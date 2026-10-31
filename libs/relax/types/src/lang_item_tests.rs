//! Collection-level tests: rustc_middle/src/ty/adt.rs:340 and ty/util.rs:376.
use crate::lang_items::LangItem;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;

fn collect(source: &str) -> (Program, Tcx) {
    let sess = &std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("relax-adt-lang-{}-{n}.rs", std::process::id()));
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
fn lang_item_flags_and_destructors() {
    let (prog, tcx) = collect(r#"
        #[lang = "drop"] trait Cleanup { fn release(&mut self); }
        #[lang = "owned_box"] struct Heap<T>(*mut T);
        impl<T> Cleanup for Heap<T> { fn release(&mut self) {} }
        #[lang = "phantom_data"] struct Marker<T>;
        #[lang = "manually_drop"] struct Suppress<T> { value: T }
        #[lang = "unsafe_cell"] struct Cell<T> { value: T }
        #[lang = "unsafe_pinned"] struct Pinned<T> { value: T }
        struct Plain;
        struct User;
        impl Cleanup for User { fn release(&mut self) {} }
        trait Drop { fn drop(&mut self); }
        impl Drop for Plain { fn drop(&mut self) {} }
        #[non_exhaustive] enum Open { A, B }
        #[non_exhaustive] struct Record { value: u8 }
        union Storage { value: u8 }
    "#);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    for (lang, name) in [(LangItem::OwnedBox, "Heap"), (LangItem::PhantomData, "Marker"),
        (LangItem::ManuallyDrop, "Suppress"), (LangItem::UnsafeCell, "Cell"), (LangItem::UnsafePinned, "Pinned")] {
        assert_eq!(tcx.lang_items.require(lang).unwrap(), named(&prog, name));
    }
    assert!(tcx.adts[&named(&prog, "Heap")].is_box());
    assert!(tcx.adts[&named(&prog, "Marker")].is_phantom_data());
    assert!(tcx.adts[&named(&prog, "Suppress")].is_manually_drop());
    assert!(tcx.adts[&named(&prog, "Cell")].is_unsafe_cell());
    assert!(tcx.adts[&named(&prog, "Pinned")].is_unsafe_pinned());
    for name in ["Heap", "User"] {
        let adt = named(&prog, name);
        let dtor = tcx.adt_destructor(adt).unwrap();
        assert_eq!(prog.name(dtor.did), "release");
        assert_eq!(tcx.impl_trait[&prog.def(dtor.did).parent], tcx.lang_items.require(LangItem::Drop).unwrap());
    }
    assert!(tcx.adt_destructor(named(&prog, "Plain")).is_none());
    assert!(tcx.adts[&named(&prog, "Open")].is_variant_list_non_exhaustive());
    assert!(!tcx.adts[&named(&prog, "Record")].is_variant_list_non_exhaustive());
    assert!(tcx.adts[&named(&prog, "Heap")].has_ctor());
    assert!(tcx.adts[&named(&prog, "Plain")].has_ctor());
    assert!(!tcx.adts[&named(&prog, "Open")].has_ctor());
    assert!(!tcx.adts[&named(&prog, "Record")].has_ctor());
    assert!(tcx.adts[&named(&prog, "Storage")].is_union());
}

#[test]
fn lang_item_incremental_collection_and_rollback() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let (mut prog, mut tcx) = collect(r#"
        #[lang = "drop"] pub trait Cleanup { fn drop(&mut self); }
        pub struct User;
    "#);
    let before = prog.defs.len();
    let root = std::env::temp_dir().join(format!("relax-adt-added-{}.rs", std::process::id()));
    std::fs::write(&root, "#[lang = \"phantom_data\"] pub struct Marker<T>; struct Owned; impl custom::Cleanup for Owned { fn drop(&mut self) {} }").unwrap();
    let dep = prog.syms.intern("custom");
    prog.load_crate("extension", root.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![(dep, 0)]).unwrap();
    std::fs::remove_file(root).unwrap();
    prog.resolve_imports();
    let mut defs = Vec::new();
    for i in before..prog.defs.len() { defs.push(DefId(i as u32)); }
    let errors = tcx.collect_added(&prog, &defs, false);
    assert!(errors.is_empty(), "{errors:?}");
    let adt = named(&prog, "Owned");
    let dtor = tcx.adt_destructor(adt).unwrap();
    assert!(tcx.adts[&named(&prog, "Marker")].is_phantom_data());
    tcx.uncollect_impls(&[prog.def(dtor.did).parent]);
    assert!(tcx.adt_destructor(adt).is_none());
}

#[test]
fn lang_item_relax_std() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/core/lib.rs");
    prog.load_crate("core", root, 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    let core = prog.syms.intern("core");
    let alloc = prog.load_crate("alloc", concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/alloc/lib.rs"), 2021,
        crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0)]).unwrap();
    let alloc_name = prog.syms.intern("alloc");
    prog.load_crate("std", concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/std/lib.rs"), 2021,
        crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0), (alloc_name, alloc)]).unwrap();
    prog.prelude_crate = Some(0);
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    let lang_errors: Vec<_> = tcx.errors.iter().filter(|e| e.contains("E0152") || e.contains("E0718") || e.contains("E0522")).collect();
    assert!(lang_errors.is_empty(), "{lang_errors:?}");
    assert!(tcx.lang_items.get(LangItem::Termination).is_some());
    let boxed = tcx.lang_items.require(LangItem::OwnedBox).unwrap();
    let phantom = tcx.lang_items.require(LangItem::PhantomData).unwrap();
    let cell = tcx.lang_items.require(LangItem::UnsafeCell).unwrap();
    assert!(tcx.adts[&boxed].is_box());
    assert!(tcx.adts[&phantom].is_phantom_data());
    assert!(tcx.adts[&cell].is_unsafe_cell());
    assert!(tcx.adt_destructor(boxed).is_some());
    assert!(tcx.adt_destructor(named(&prog, "Vec")).is_some());
    assert!(tcx.adt_destructor(phantom).is_none());
    assert!(tcx.adt_destructor(cell).is_none());
    // ManuallyDrop is a struct carrying the lang item, as in rustc.
    let manually_drop = tcx.lang_items.require(LangItem::ManuallyDrop).unwrap();
    assert!(tcx.adts[&manually_drop].is_manually_drop());
    assert!(!tcx.adts[&manually_drop].is_union());
    assert!(tcx.lang_items.get(LangItem::UnsafePinned).is_none());
}

#[test]
fn relax_std_provides_coerce_unsized_for_pointers_and_smart_pointers() {
    use crate::next_solver::{Goal, collect};
    use crate::consts::GenericArg;
    use crate::types::TyKind;
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    prog.load_crate("core", concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/core/lib.rs"), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    let core = prog.syms.intern("core");
    prog.load_crate("alloc", concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/alloc/lib.rs"), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0)]).unwrap();
    prog.prelude_crate = Some(0);
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    let lang_errors: Vec<_> = tcx.errors.iter().filter(|e| e.contains("E0152") || e.contains("E0718") || e.contains("E0522")).collect();
    assert!(lang_errors.is_empty(), "{lang_errors:?}");
    let coerce = tcx.lang_items.require(LangItem::CoerceUnsized).unwrap();
    let tys = &tcx.tys;
    let array = tys.intern(TyKind::Array(tys.i32_, tys.const_usize(3)));
    let slice = tys.intern(TyKind::Slice(tys.i32_));
    let r = tys.intern_region(crate::types::RegionKind::ReStatic);
    let boxed = tcx.lang_items.require(LangItem::OwnedBox).unwrap();
    let adt = |d, t| tys.intern(TyKind::Adt(d, tys.mk_list(&[t]), crate::types::RegionArgsId::EMPTY));
    let infcx = crate::infer::InferCtxt::new(&tcx, crate::tcx::TypingMode::PostAnalysis, Default::default());
    let holds = |from, to| infcx.evaluate_obligation(&prog, Goal { param_env: crate::tcx::ParamEnvId::EMPTY,
        predicate: collect::trait_predicate(coerce, vec![GenericArg::Type(from), GenericArg::Type(to)]) }).must_apply_modulo_regions();
    let reference = |m, t| tys.intern(TyKind::Ref(m, t, r));
    assert!(holds(reference(false, array), reference(false, slice)));
    assert!(holds(reference(true, array), reference(false, slice)));
    assert!(holds(reference(true, array), reference(true, slice)));
    assert!(!holds(reference(false, array), reference(true, slice)));
    assert!(holds(reference(false, array), tys.intern(TyKind::Ptr(false, slice))));
    assert!(holds(adt(boxed, array), adt(boxed, slice)));
    assert!(!holds(adt(boxed, slice), adt(boxed, array)));
}

#[test]
fn relax_std_marks_container_drops_may_dangle() {
    use crate::next_solver::dropck_outlives::adt_dtorck_constraint;
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    prog.load_crate("core", concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std/core/lib.rs"), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    prog.prelude_crate = Some(0);
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    // `Vec<&T>` may be dropped while `T` is borrowed: the element type is not among what the destructor touches.
    let vec = named(&prog, "Vec");
    let constraint = adt_dtorck_constraint(&tcx, &prog, vec, 256);
    assert!(constraint.outlives.is_empty(), "{constraint:?}");
}
