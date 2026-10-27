//! The whole MIR borrowck pipeline (build, signatures, type check, region inference, checks) on small
//! Rust sources over Relax's real core and alloc: the repros of what real crates showed.
use crate::program::{DefId, Program};
use crate::tcx::{EvalCx, EvalState, Tcx, TypingEnv};

/// Error codes (or messages) and unchecked reasons of every fn of `source`.
pub(super) fn borrowck_source(source: &str) -> (Vec<String>, Vec<String>) {
    borrowck_source_in(source, std::env::var("PROBE_EDITION").ok().and_then(|e| e.parse().ok()).unwrap_or(2021))
}

pub(super) fn borrowck_source_in(source: &str, edition: u16) -> (Vec<String>, Vec<String>) {
    borrowck_source_named(source, edition, "repro")
}

/// The crate `source` (named `crate_name`, as `#![crate_name]` does in rustc's tests) over Relax's core, alloc and std.
fn load_source(source: &str, edition: u16, crate_name: &str) -> (Program, Tcx, u32) {
    crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(Some(true)));
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    let std_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../compiler/std");
    prog.load_crate("core", &format!("{std_dir}/core/lib.rs"), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
    let core = prog.syms.intern("core");
    let alloc = prog.load_crate("alloc", &format!("{std_dir}/alloc/lib.rs"), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0)]).unwrap();
    let alloc_name = prog.syms.intern("alloc");
    let std = prog.load_crate("std", &format!("{std_dir}/std/lib.rs"), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0), (alloc_name, alloc)]).unwrap();
    let std_name = prog.syms.intern("std");
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("relax-mirsrc-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, source).unwrap();
    let user = prog.load_crate(crate_name, path.to_str().unwrap(), edition, crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0), (alloc_name, alloc), (std_name, std)]).unwrap();
    std::fs::remove_file(&path).unwrap(); std::fs::remove_dir(&dir).unwrap();
    prog.prelude_crate = Some(0);
    prog.resolve_imports();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    (prog, tcx, user)
}

/// The typeck errors of every fn of `source`, as the checker words them.
pub(super) fn typeck_errors_source(source: &str, edition: u16) -> Vec<String> {
    let (prog, tcx, user) = load_source(source, edition, "repro");
    let mut errors = vec![];
    for owner in crate::borrowck::body::owners(&prog) {
        if prog.def(owner.owner).krate != user { continue; }
        errors.extend(tcx.typeck(&prog, owner.owner).errors.iter().cloned());
    }
    errors
}

/// The well-formedness errors (wfcheck) of every definition of `source`.
pub(super) fn wfcheck_errors(source: &str) -> Vec<String> {
    let (prog, tcx, user) = load_source(source, 2021, "repro");
    let mut errors = vec![];
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        if prog.def(d).krate != user { continue; }
        errors.extend(crate::wfcheck::obligations::check_def(&prog, &tcx, d));
    }
    errors
}

/// `borrowck_source_in` for a crate of the given name.
pub(super) fn borrowck_source_named(source: &str, edition: u16, crate_name: &str) -> (Vec<String>, Vec<String>) {
    borrowck_source_checked_named(source, edition, crate_name, false)
}

/// `typeck_errors_source` for a crate that is being checked.
pub(super) fn typeck_errors_checked(source: &str, edition: u16) -> Vec<String> {
    let (mut prog, tcx, user) = load_source(source, edition, "repro");
    prog.crates[user as usize].validate = true;
    let mut errors = vec![];
    for owner in crate::borrowck::body::owners(&prog) {
        if prog.def(owner.owner).krate != user { continue; }
        errors.extend(tcx.typeck(&prog, owner.owner).errors.iter().cloned());
    }
    errors
}

/// `borrowck_source_in` for a crate that is being checked (`makepad check`: its diagnostics are reported).
pub(super) fn borrowck_source_checked(source: &str, edition: u16) -> (Vec<String>, Vec<String>) {
    borrowck_source_checked_named(source, edition, "repro", true)
}

fn borrowck_source_checked_named(source: &str, edition: u16, crate_name: &str, validate: bool) -> (Vec<String>, Vec<String>) {
    let (mut prog, tcx, user) = load_source(source, edition, crate_name);
    prog.crates[user as usize].validate = validate;
    let (mut errors, mut unchecked) = (vec![], vec![]);
    for owner in crate::borrowck::body::owners(&prog) {
        if prog.def(owner.owner).krate != user { continue; }
        let state = EvalState::new();
        let cx = EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
        let body = tcx.typeck(&prog, owner.owner);
        assert!(body.errors.is_empty(), "typeck: {:?}", body.errors);
        if std::env::var_os("PROBE").is_some() { for (ce, c) in body.min_captures.iter() { eprintln!("PROBE closure {ce:?} kind {:?} captures {:?}", c.kind, c.captures.iter().map(|c| (c.info.kind, c.place.projections.len())).collect::<Vec<_>>()); } }
        let v = crate::borrowck::driver::check_root(&cx, &prog, &tcx, &body, None);
        errors.extend(v.errors.iter().map(|e| format!("{} {} {}:{} {:?} {:?}", e.code, e.message, e.line, e.col, e.labels, e.notes)));
        unchecked.extend(v.unchecked);
    }
    let _ = DefId(0);
    (errors, unchecked)
}

#[test]
fn a_reference_to_self_implies_the_outlives_of_its_lifetime_arguments() {
    let (errors, unchecked) = borrowck_source("
        pub struct Line<'a> { data: &'a [u8] }
        pub struct Gradient<'a> { line: Line<'a> }
        pub struct Stops<'a, 'b> { line: &'b Line<'a>, index: u16 }
        impl<'a> Gradient<'a> {
            pub fn stops<'b>(&'b self, index: u16) -> Stops<'a, 'b> { Stops { line: &self.line, index } }
        }");
    assert!(errors.is_empty(), "{errors:?}");
    assert!(unchecked.is_empty(), "{unchecked:?}");
}

#[test]
fn a_closure_returning_a_reborrow_of_its_capture_is_fn_once_when_the_bound_says_so() {
    let (errors, unchecked) = borrowck_source("
        pub fn f(buf: &mut [u8], n: Option<usize>) -> Option<&mut [u8]> { n.and_then(|k| buf.get_mut(..k)) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_vec_of_borrows_may_dangle_at_its_drop() {
    let (errors, unchecked) = borrowck_source("
        pub struct Heap { slots: Vec<u32>, other: u32 }
        impl Heap {
            fn set(&mut self, v: u32) { self.other = v; }
            pub fn f(&mut self) {
                let mut kvs = Vec::new();
                for kv in self.slots.iter() { kvs.push(kv); }
                let n = kvs.len() as u32;
                self.set(n);
            }
        }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_pointer_cast_may_change_the_lifetime_arguments_of_its_pointee() {
    let (errors, unchecked) = borrowck_source("
        pub struct Out<'a> { s: &'a str }
        pub struct Q { text: *mut Out<'static> }
        impl Q { pub fn f(&mut self, out: &mut Out) { self.text = out as *mut Out as *mut Out<'static>; } }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_struct_with_a_reference_field_implies_its_inferred_outlives() {
    let (errors, unchecked) = borrowck_source("
        pub struct Image<T> { pixels: Vec<T>, w: usize }
        impl<T> core::ops::Index<usize> for Image<T> { type Output = T; fn index(&self, i: usize) -> &T { &self.pixels[i] } }
        #[derive(Clone, Copy)]
        pub struct Subimage<'a, T> { image: &'a Image<T>, off: usize }
        impl<'a, T> Subimage<'a, T> { pub fn first(&self) -> &T { &self.image[self.off] } }
        impl<'a, T> core::ops::Index<usize> for Subimage<'a, T> { type Output = T; fn index(&self, i: usize) -> &T { &self.image[self.off + i] } }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn promoted_constants_are_checked_with_their_own_locations() {
    // `&[..]` and `&5` are promoted: their statements are numbered in the promoted body, not the parent.
    let (errors, unchecked) = borrowck_source("
        pub fn f(x: u32) -> (&'static [u32], &'static u32) { let a: &'static [u32] = &[1, 2, 3, 4, 5, 6]; if x > 1 { (a, &7) } else { (&[9], &8) } }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn closure_shapes_map_their_external_regions_to_the_creator() {
    for (name, src) in [
        ("late-bound parameter", "pub fn f(v: &[&str]) -> usize { v.iter().map(|s| s.len()).sum() }"),
        ("captured reference returned", "pub fn f<'a>(x: &'a str) -> impl Fn() -> &'a str { move || x }"),
        ("annotated parameter", "pub fn f() -> usize { let g = |s: &str| s.len(); g(\"a\") + g(\"bc\") }"),
        ("nested", "pub fn f(v: &[&str]) -> usize { v.iter().map(|s| { let h = |t: &str| t.len(); h(s) }).sum() }"),
    ] {
        let (errors, unchecked) = borrowck_source(src);
        assert!(unchecked.is_empty(), "{name}: {unchecked:?}");
        assert!(errors.is_empty(), "{name}: {errors:?}");
    }
}

#[test]
fn a_provided_trait_method_calls_another_through_self() {
    let (errors, unchecked) = borrowck_source("
        pub trait ByteOrder: Sized {
            fn read_u32_into(src: &[u8], dst: &mut [u32]);
            fn read_f32_into(src: &[u8], dst: &mut [f32]) {
                let dst = unsafe {
                    const _: () = assert!(core::mem::align_of::<u32>() <= core::mem::align_of::<f32>());
                    core::slice::from_raw_parts_mut(dst.as_mut_ptr() as *mut u32, dst.len())
                };
                Self::read_u32_into(src, dst);
            }
        }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_closure_returning_a_slice_of_a_captured_parameter() {
    let (errors, unchecked) = borrowck_source("
        pub struct R { rules: Vec<String> }
        impl R {
            pub fn f(&mut self, path: &str) -> usize {
                let mut bases = vec![\"\"];
                bases.extend(path.match_indices('/').map(|(i, _)| &path[..i]));
                bases.len() + self.rules.len()
            }
        }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn ignored_repo_from_makepad_git() {
    let src = r####"
        use std::collections::HashMap;
        use std::path::{Path, PathBuf};
        pub struct IgnoreError;
        pub struct Rule { pub negative: bool }
        impl Rule { pub fn matches(&self, _p: &str, _d: bool) -> bool { true } }
        pub fn read_rules(_p: &Path) -> Result<Vec<Rule>, IgnoreError> { Ok(Vec::new()) }
        pub struct S { directories: HashMap<String, bool>, excludes: Vec<Rule>, rules: HashMap<String, Vec<Rule>>, root: PathBuf }
        impl S {
    fn ignored_repo(&mut self, path: &str, directory: bool) -> Result<bool, IgnoreError> {
        if directory {
            if let Some(&ignored) = self.directories.get(path) {
                return Ok(ignored);
            }
        }
        let parent = path.rsplit_once('/').map_or("", |(d, _)| d);
        let mut ignored = !parent.is_empty() && self.ignored_repo(parent, true)?;
        if !ignored {
            for rule in &self.excludes {
                if rule.matches(path, directory) {
                    ignored = !rule.negative;
                }
            }
            let mut bases = vec![""];
            bases.extend(path.match_indices('/').map(|(i, _)| &path[..i]));
            for base in bases {
                if !self.rules.contains_key(base) {
                    self.rules.insert(
                        base.to_owned(),
                        read_rules(&self.root.join(base).join(".gitignore"))?,
                    );
                }
                let relative = if base.is_empty() {
                    path
                } else {
                    &path[base.len() + 1..]
                };
                for rule in &self.rules[base] {
                    if rule.matches(relative, directory) {
                        ignored = !rule.negative;
                    }
                }
            }
        }
        if directory {
            self.directories.insert(path.to_owned(), ignored);
        }
        Ok(ignored)
    }
        }
    "####;
    let (errors, unchecked) = borrowck_source(src);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_closure_that_returns_its_argument_reference() {
    let (errors, unchecked) = borrowck_source("
        pub fn f(path: &str) -> &str { path.rsplit_once('/').map_or(\"\", |(d, _)| d) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_method_closure_with_an_erased_signature_region_maps_it_to_the_creators_variable() {
    // The closure's signature mentions a region typeck erased: it is an external region of the closure,
    // found at the same position in the creator's instantiation of the closure type.
    let (errors, unchecked) = borrowck_source("
        pub struct S;
        impl S { pub fn f(&mut self, path: &str) -> usize { let parent = path.rsplit_once('/').map_or(\"\", |(d, _)| d); parent.len() } }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn bodies_naming_constants_are_checked_with_their_qualifs() {
    let (errors, unchecked) = borrowck_source("
        pub const N: usize = 3;
        pub const NAME: &str = \"x\";
        pub struct P { pub v: [u8; 2] }
        pub const EMPTY: P = P { v: [0; 2] };
        pub fn f() -> ([u8; N], &'static str, P) { ([0; N], NAME, EMPTY) }
        pub fn g() -> &'static P { &EMPTY }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_generic_with_a_lifetime_bounded_type_parameter_is_instantiated_in_declaration_order() {
    let (errors, unchecked) = borrowck_source("
        pub trait Item<'a>: Sized { fn parse(tag: u32, data: &'a [u8]) -> Option<Self>; }
        pub struct List<'a, T: Item<'a>> { data: &'a [u8], tags: &'a [u32], ty: core::marker::PhantomData<T> }
        impl<'a, T: Item<'a>> List<'a, T> {
            pub fn get(&self, index: usize) -> Option<T> {
                let tag = *self.tags.get(index)?;
                self.data.get(index..).and_then(|data| T::parse(tag, data))
            }
        }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_const_block_declaring_an_item_and_asserting_layout() {
    let (errors, unchecked) = borrowck_source("
        #[repr(C)] pub struct V { pub x: f32, pub y: f32 }
        pub trait Pod {} impl Pod for f32 {}
        const _: () = {
            let mut end = 0;
            assert!(core::mem::offset_of!(V, x) == end);
            end += core::mem::size_of::<f32>();
            assert!(core::mem::size_of::<V>() == 2 * end);
            fn fields_are_pod(value: &V) {
                fn pod<T: Pod>(_: &T) {}
                let field: &f32 = &value.x; pod(field);
            }
            let _ = fields_are_pod;
        };");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_promoted_closure_is_not_a_missing_nested_body() {
    let (errors, unchecked) = borrowck_source("
        pub fn run(cancel: &dyn Fn() -> bool) -> bool { cancel() }
        pub fn f() -> bool { run(&|| false) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_array_indexed_by_a_user_type_through_the_slice_impl() {
    let (errors, unchecked) = borrowck_source("
        #[derive(Clone, Copy)] pub enum T { A, B }
        impl<V> core::ops::Index<T> for [V] { type Output = V; fn index(&self, t: T) -> &V { &self[t as usize] } }
        pub struct M { found: [bool; 2] }
        impl M { pub fn found_script(&self, t: T) -> bool { self.found[t] } }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_static_array_of_inline_const_atomics() {
    let (errors, unchecked) = borrowck_source("
        use core::sync::atomic::{AtomicUsize, Ordering};
        static FRAMES: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
        pub fn f(i: usize) -> usize { FRAMES[i].load(Ordering::Relaxed) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_turbofish_on_a_tuple_struct_constructor_is_a_user_type_annotation() {
    let (errors, unchecked) = borrowck_source("
        pub struct SomeStruct<T>(pub T);
        pub fn bad() { let c = 66; SomeStruct::<&'static u32>(&c); }
        pub fn ok() { let c = 66; SomeStruct::<&u32>(&c); }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("E0597"), "{errors:?}");
}

/// Probe: `UI_FILES=a.rs:b.rs cargo test zz_probe_files -- --nocapture` prints what the pipeline says about each.
#[test]
fn zz_probe_files() {
    let Some(files) = std::env::var_os("UI_FILES") else { return };
    for f in files.to_string_lossy().split(':') {
        let src = std::fs::read_to_string(f).unwrap();
        let r = std::panic::catch_unwind(|| borrowck_source(&src));
        match r { Ok((e, u)) => eprintln!("PROBE {f}: errors {} {:?} unchecked {:?}", e.len(), e.iter().map(|x| x.chars().take(std::env::var("PROBE_W").ok().and_then(|w| w.parse().ok()).unwrap_or(60)).collect::<String>()).collect::<Vec<_>>(), u),
                  Err(_) => eprintln!("PROBE {f}: harness panic (typeck/front errors?)") }
    }
}

#[test]
fn a_higher_ranked_where_clause_on_an_associated_type_is_a_verify_bound() {
    let (errors, unchecked) = borrowck_source("
        pub fn csv<V>(value: &V) -> Result<String, &str>
        where V: 'static, for<'a> &'a V: IntoIterator, for<'a> <&'a V as IntoIterator>::Item: ToString + 'static,
        { let s: String = value.into_iter().map(|e| e.to_string()).collect::<String>(); Ok(s) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_move_out_of_a_promoted_array_element_is_e0508() {
    let (errors, unchecked) = borrowck_source("
        #[derive(Clone)] struct S1(S2);
        #[derive(Clone)] struct S2;
        pub fn f() { const C: [S2; 1] = [S2]; let _ = S1(C[0]).clone(); }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("E0508"), "{errors:?}");
}

#[test]
fn closures_in_a_promoted_constant_use_the_parents_user_type_annotations() {
    let (errors, unchecked) = borrowck_source("
        pub fn f<'a, T: 'a>(x: &'a T) -> usize { let g = |y: &'a T| -> usize { let _z: &'a T = y; 1 }; g(x) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn lifetimes_written_on_a_constructor_pattern_path_are_a_user_type_annotation() {
    for (decl, pat) in [
        ("struct Foo<'a>(&'a u32);", "Foo::<'static>(_z)"),
        ("struct Foo<'a> { field: &'a u32 }", "Foo::<'static> { field: _z }"),
        ("enum Foo<'a> { Bar(&'a u32) }", "Foo::Bar::<'static>(_z)"),
        ("enum Foo<'a> { Bar(&'a u32) }", "Foo::<'static>::Bar(_z)"),
        ("enum Foo<'a> { Bar { field: &'a u32 } }", "Foo::Bar::<'static> { field: _z }"),
    ] {
        let make = if decl.contains("field") { if decl.contains("enum") { "Foo::Bar { field: &y }" } else { "Foo { field: &y }" } }
            else if decl.contains("enum") { "Foo::Bar(&y)" } else { "Foo(&y)" };
        let (errors, unchecked) = borrowck_source(&format!("{decl} pub fn f() {{ let y = 22; let foo = {make}; let {pat} = foo; }}"));
        assert!(unchecked.is_empty(), "{pat}: {unchecked:?}");
        assert!(errors.len() == 1 && errors[0].starts_with("E0597"), "{pat}: {errors:?}");
    }
}

#[test]
fn lifetimes_written_on_the_self_type_of_an_inherent_path_are_kept() {
    let (errors, unchecked) = borrowck_source("
        pub struct A<'a> { x: &'a u32 }
        impl<'a> A<'a> { fn new<'b, T>(x: &'a u32, _y: T) -> Self { Self { x } } }
        pub fn foo<'a>() { let v = 22; let _x = A::<'a>::new(&v, 22); }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0597"), "{errors:?}");
}

#[test]
fn lifetimes_written_on_the_trait_of_a_qualified_path_are_kept() {
    let (errors, unchecked) = borrowck_source("
        pub trait Foo<'a> { const C: &'a u32; }
        impl<'a, T> Foo<'a> for T { const C: &'a u32 = &22; }
        pub fn foo<'a>(_: &'a u32) -> &'static u32 { <() as Foo<'a>>::C }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].contains("lifetime may not live long enough"), "{errors:?}");
}

#[test]
fn a_promoted_fn_item_keeps_its_turbofish_annotation() {
    let (errors, unchecked) = borrowck_source("
        pub fn foo<'a>() { let x = 0; let f = &drop::<&'a i32>; f(&x); }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0597"), "{errors:?}");
}

#[test]
fn a_tuple_constructor_item_is_invariant_in_its_arguments() {
    let (errors, unchecked) = borrowck_source("
        pub struct S<T>(T);
        pub fn bad() { let c = 66; let f = S::<&'static u32>; f(&c); }
        pub fn ok() { let c = 66; let f = S::<&u32>; f(&c); }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0597"), "{errors:?}");
}

#[test]
fn an_opaque_with_a_static_bound_keeps_the_regions_it_captures_dead() {
    let (errors, unchecked) = borrowck_source("
        pub trait Captures<'a> {}
        impl<T> Captures<'_> for T {}
        pub fn foo(x: &mut i32) -> impl Sized + Captures<'_> + 'static {}
        pub fn overlapping_mut() { let i = &mut 1; let _x = foo(i); let _y = foo(i); }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_opaque_outliving_every_lifetime_captures_no_live_region() {
    let (errors, unchecked) = borrowck_source("
        pub trait Captures<'a> {}
        impl<T> Captures<'_> for T {}
        pub trait Outlives<'a>: 'a {}
        impl<'a, T: 'a> Outlives<'a> for T {}
        pub fn test<'o>(v: &'o Vec<i32>) -> impl Captures<'o> + for<'a> Outlives<'a> {}
        pub fn opaque_doesnt_use_temporary() { let _a = test(&vec![]); }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_async_fn_body_is_ordered_among_its_closures_by_its_block() {
    // (the coroutine body itself is not yet qualified: the check stops, it does not panic)
    let (errors, _unchecked) = borrowck_source("
        pub async fn f(v: Vec<String>) -> usize { v.into_iter().map(|s| s.len()).sum() }");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_type_test_in_a_closure_is_promoted_to_the_lowest_universal_region_of_its_class() {
    let (errors, unchecked) = borrowck_source("
        use std::any::Any;
        type B = Box<dyn for<'a> Fn(&(dyn Any + 'a)) -> Box<dyn Any + 'a>>;
        pub fn foo<E>() -> B { Box::new(|e| Box::new(e.is::<E>())) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0310"), "{errors:?}");
}

#[test]
fn the_anonymous_lifetimes_of_a_bare_fn_trait_object_are_late_bound() {
    let (errors, unchecked) = borrowck_source_in("
        use std::fmt;
        pub fn new_format<F>(_f: F) where F: FnMut(i32, &mut FnMut(&fmt::Display) -> fmt::Result) -> fmt::Result {}
        pub fn main() { new_format(|i, f| f(&format_args!(\"0x{:x}\", i))); }", 2015);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_closure_returning_a_captured_reference_requires_the_capture_to_outlive_the_signature() {
    let (errors, unchecked) = borrowck_source("
        pub fn foo(_: impl FnOnce(&u32) -> &u32) {}
        pub fn bar() { let x = 22; foo(|_a| &x) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0597"), "{errors:?}");
}

#[test]
fn a_nested_closure_returning_a_captured_reference_keeps_the_borrow_alive() {
    let (errors, unchecked) = borrowck_source("
        pub fn main() {
            let mut greeting = String::new();
            let res = (|| (|| &greeting)())();
            greeting = String::new();
            drop(greeting);
            drop(res);
        }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0506"), "{errors:?}");
}

#[test]
fn a_higher_ranked_mismatch_is_e0308_one_type_is_more_general() {
    // (typeck's: a function-pointer coercion forces the leak check, coercion.rs coerce_from_fn_pointer;
    // tests/ui/regions/higher-ranked-implied.rs)
    let (prog, tcx, user) = load_source("
        pub struct Inv<T>(*mut T);
        pub type A = for<'a, 'b> fn(Inv<&'a &'b ()>, Inv<&'b &'a ()>, Inv<&'a ()>);
        pub type B = for<'a, 'b> fn(Inv<&'a &'b ()>, Inv<&'b &'a ()>, Inv<&'b ()>);
        pub fn f(x: A) { let _y: B = x; }", 2021, "repro");
    let errors: Vec<String> = crate::borrowck::body::owners(&prog).filter(|o| prog.def(o.owner).krate == user).flat_map(|o| tcx.typeck(&prog, o.owner).errors.to_vec()).collect();
    assert!(errors.len() == 1 && errors[0].contains("E0308: mismatched types"), "{errors:?}");
}

#[test]
fn an_async_block_is_borrow_checked_as_a_coroutine() {
    let (errors, unchecked) = borrowck_source_in("
        pub fn g() { let _a = async { let temp = 1; let _p: &'static u32 = &temp; }; }
        pub async fn h(x: &mut Vec<u8>) { let r = &*x; yield_now().await; x.push(r.len() as u8); }
        async fn yield_now() {}", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0597"), "{errors:?}");
}

#[test]
fn a_coroutine_without_suspension_points_needs_only_its_upvars_drop_live() {
    let (errors, unchecked) = borrowck_source_in("
        pub async fn foo() { let mut f = None; let value = 0; f = Some(async { value }); core::mem::drop(f); }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_async_fn_matches_its_parameter_patterns_inside_the_coroutine() {
    let (errors, unchecked) = borrowck_source_in("
        pub async fn c(ref vec: Vec<u32>) { vec.contains(&0); }
        pub async fn c2(n: u32, ref mut vec: Vec<u32>) { vec.push(n); }
        pub async fn c3(((ref a, ref mut b), (ref mut c, ref d)): ((Vec<u32>, Vec<u32>), (Vec<u32>, Vec<u32>))) { let _ = (a, d); b.push(1); c.push(2); }
        pub async fn d((a, b): (String, String), _: Vec<u8>, mut m: i32, &n: &i32) { let _ = (a, b, m, n); m += 1; }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
    let (errors, unchecked) = borrowck_source_in("pub async fn e((a, _b): (String, String)) -> &'static str { &a }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    // (rustc says E0515 for a coroutine's borrow of its own binding that is returned; the line is the same)
    assert!(errors.len() == 1, "{errors:?}");
}

#[test]
fn an_async_closure_with_a_macro_body_orders_its_coroutine_before_it() {
    let (errors, unchecked) = borrowck_source_in("
        pub fn main() { let _ = async || println!(); }", 2018);
    assert!(unchecked.iter().all(|u| u.contains("async closures")), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_nested_opaque_captures_the_items_lifetimes_through_the_enclosing_opaque() {
    // Equal `'a`/`'b` are the same canonical argument of the nested opaque: a defining use, not a duplicate parameter.
    let (errors, unchecked) = borrowck_source("
        pub fn test<'a: 'b, 'b: 'a>() -> impl IntoIterator<Item = (&'a u8, impl Into<(&'b u8, &'a u8)>)> {
            None::<(_, (_, _))>
        }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_closure_cast_to_a_higher_ranked_fn_pointer_takes_the_signature_from_the_target() {
    let (errors, unchecked) = borrowck_source("
        pub fn foo() -> impl Into<for<'a> fn(&'a ())> { (|_| {}) as for<'a> fn(&'a ()) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn the_region_bound_pairs_of_a_closure_name_the_universal_regions_of_its_type() {
    // The upvar type carries typeck's own region variables; the pairs `Opaque<..>: 'env` must name
    // the closure's external regions, not whatever NLL variable has the same number.
    let (errors, unchecked) = borrowck_source("
        pub struct It;
        pub trait Resolver {}
        impl<R: Resolver> Resolver for &R {}
        impl Resolver for It {}
        pub fn get2<'a, 'b: 'b>(mut items: impl Iterator<Item = &'a It>) -> impl Resolver + 'a { items.next().unwrap() }
        pub struct Container<'a> { name: String, resolver: Box<dyn Resolver + 'a> }
        impl<'a> Container<'a> { fn new<R: Resolver + 'a>(name: &str, resolver: R) -> Self { Self { name: name.to_owned(), resolver: Box::new(resolver) } } }
        pub fn main() {
            let it = It;
            let resolver = get2(Some(&it).into_iter());
            let f = |n: &str| Container::new(n, &resolver);
            f(\"a\");
        }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_opaque_types_implied_bounds_follow_its_lifetime_parameters_by_index() {
    // The late-bound `'c` is in the opaque's contract but not among its lifetime arguments: the where
    // clauses of `'d`/`'e` must not swap, or `'b: 'd` is assumed and the member constraint picks neither.
    let (errors, unchecked) = borrowck_source("
        trait Trait<'a, 'b> {}
        impl<T> Trait<'_, '_> for T {}
        struct Ordinary<'a>(&'a u8);
        fn upper_bounds<'a, 'b, 'c, 'd, 'e>(a: Ordinary<'a>, b: Ordinary<'b>) -> impl Trait<'d, 'e>
        where 'a: 'e, 'b: 'e, 'a: 'd { let p = if true { a } else { b }; p }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_coroutine_whose_witness_needs_no_drop_needs_only_its_upvars_drop_live() {
    // rust-lang/rust#116242: the locals saved across the await (`&String`, `Ready<()>`) have no drop glue,
    // so the future's drop needs the captured borrow of `s` only if the borrowed type itself has drop glue.
    let (errors, unchecked) = borrowck_source_in("
        pub fn f() {
            let s = String::new();
            let fut = async { let r = &s; core::future::ready(()).await; let _x = r.len(); };
            drop(s);
        }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_coroutine_whose_witness_needs_drop_keeps_all_its_upvars_live_at_its_drop() {
    // The `String` is saved across the await and has drop glue: the captured borrow of `s` is live at the drop.
    let (errors, unchecked) = borrowck_source_in("
        pub fn f() {
            let s = String::new();
            let fut = async { let r = &s; let t = String::new(); core::future::ready(()).await; let _x = (r.len(), t.len()); };
            drop(s);
        }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0505"), "{errors:?}");
}

#[test]
fn nested_or_patterns_bind_one_variable_across_all_alternatives() {
    let (errors, unchecked) = borrowck_source_in("
        pub fn f() { let (Ok((Ok(a) | Err(a), b)) | Err(Ok((a, b)) | Err((a, b)))): Result<(Result<i32, i32>, i32), Result<(i32, i32), (i32, i32)>> = Ok((Ok(1), 1)); let _ = (a, b); }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_async_fn_in_a_generic_impl_maps_the_signatures_erased_regions_after_its_parents() {
    // The coroutine's hidden type has the parent's `'a` slot erased too: the signature's first erased region
    // is not the first erased slot of the closure type.
    let (errors, unchecked) = borrowck_source_in("
        pub struct S<'a> { x: &'a u32 }
        impl<'a> S<'a> { pub async fn t(f: &u32) -> &u32 { f } pub async fn u(self, f: &u32) -> &u32 { f } }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_late_bound_lifetime_equal_to_static_by_implied_bounds_is_a_defining_use_argument() {
    // `&'static &'_ ()` implies `'_: 'static`: the opaque's captured `'_` is 'static in the canonical arguments.
    let (errors, unchecked) = borrowck_source_in("pub async fn x(x: &'static &'_ ()) {} pub fn y(x: &'static &()) -> impl Sized { }", 2024);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_trait_objects_supertrait_projection_implies_the_outlives_of_its_term() {
    let (errors, unchecked) = borrowck_source("
        trait Trait { type Assoc; } trait Bar { type Assoc; }
        trait Foo<'a, T: 'a>: Bar<Assoc = &'a T> {}
        fn outlives<'a, T: 'a>() {}
        pub fn implied_outlives<'a, T: Trait>(x: &dyn Foo<'a, T::Assoc>) { outlives::<'a, T::Assoc>(); }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_type_alias_prefix_of_a_path_instantiates_the_aliass_lifetimes() {
    // `S2::shl` for `type S2<'a> = S<'a, 'a>`: the user self type has inference regions, not the alias's own parameter.
    let (errors, unchecked) = borrowck_source_in("
        use std::ops::Shl;
        pub struct S<'a, 'b: 'a>(&'a &'b u32);
        type S2<'a> = S<'a, 'a>;
        impl<'a, 'b> Shl<&'b u32> for S<'a, 'b> { type Output = &'a u32; fn shl(self, t: &'b u32) -> &'a u32 { t } }
        pub fn f(s: S2) -> &u32 { S2::shl(s, &mut 3) }", 2015);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0515"), "{errors:?}");
}

#[test]
fn owning_pointers_own_their_pointee_for_dropck() {
    // `Box`/`Rc`/`Arc`/`Vec` drop their `T`: a `Drop` type behind them keeps what it borrows live at their drop.
    for owner in ["Box::new(Foo(&x))", "std::rc::Rc::new(Foo(&x))", "std::sync::Arc::new(Foo(&x))", "vec![Foo(&x)]"] {
        let (errors, unchecked) = borrowck_source_in(&format!("
            struct Foo<'a>(&'a String);
            impl<'a> Drop for Foo<'a> {{ fn drop(&mut self) {{}} }}
            pub fn main() {{ let y; let x = String::new(); y = {owner}; }}"), 2018);
        assert!(unchecked.is_empty(), "{owner}: {unchecked:?}");
        assert!(errors.len() == 1 && errors[0].starts_with("E0597"), "{owner}: {errors:?}");
    }
}

#[test]
fn an_async_closure_returns_a_coroutine_that_borrows_from_its_environment() {
    // The `FnMut` async closure's coroutine reborrows `i` from the closure's environment: its upvar has
    // the environment's region (rustc `tupled_upvars_by_closure_kind`), not an unrelated external region.
    let (errors, unchecked) = borrowck_source_in("
        fn call_once<F>(_: impl FnOnce() -> F) {}
        pub fn f() { let mut i = 0; let c = async || { i += 1; }; call_once(c); }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_async_closures_arguments_are_its_coroutines_leading_upvars() {
    // The coroutine's argument upvar has the closure signature's own (higher-ranked) region.
    let (errors, unchecked) = borrowck_source_in("
        use std::future::Future;
        trait Foo<'a> { type Future: Future<Output = u8> + 'a; fn start(self, f: &'a u8) -> Self::Future; }
        impl<'a, Fn, Fut> Foo<'a> for Fn where Fn: FnOnce(&'a u8) -> Fut, Fut: Future<Output = u8> + 'a {
            type Future = Fut;
            fn start(self, f: &'a u8) -> Self::Future { (self)(f) }
        }
        fn foo<F>(f: F) where F: for<'a> Foo<'a> { let bar = 5; f.start(&bar); }
        pub fn g() { foo(async move |f: &u8| { *f }); }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_async_closure_cannot_move_out_of_a_shared_reference() {
    let (errors, unchecked) = borrowck_source_in("
        pub struct Ty;
        pub fn hello(x: &Ty) { let _c = async || { *x; }; }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.iter().any(|e| e.starts_with("E0507")), "{errors:?}");
}

#[test]
fn bevys_param_set_takes_the_outlives_its_normalization_registers_as_implied_bounds() {
    // `ParamSet<Query<&'a u8>>` is well-formed only where `'a: 'static` (the impl's `Q: 'static`); rustc keeps
    // accepting bevy_ecs's use of it by assuming the region obligations normalizing its where clauses registers.
    let source = "
        pub trait WorldQuery {}
        impl WorldQuery for &u8 {}
        pub struct Query<Q: WorldQuery>(Q);
        pub trait SystemParam { type State; }
        impl<Q: WorldQuery + 'static> SystemParam for Query<Q> { type State = (); }
        pub struct ParamSet<T: SystemParam>(T) where T::State: Sized;
        fn handler<'a>(x: ParamSet<Query<&'a u8>>) { let _: ParamSet<_> = x; }
        fn ref_handler<'a>(_: &ParamSet<Query<&'a u8>>) {}";
    let (errors, unchecked) = borrowck_source_named(source, 2021, "bevy_ecs");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
    // another crate's ParamSet gets no such allowance
    let (errors, _) = borrowck_source_named(source, 2021, "other_crate");
    assert!(!errors.is_empty(), "{errors:?}");
}

#[test]
fn a_local_without_an_annotation_is_instantiated_by_the_coercion_of_its_initializer() {
    // type-inference/generalize-subtyped-variables.rs: `let y = x` relates two type variables later, so the
    // first use of `z` generalizes it to the non-higher-ranked `fn(&'0 ())`.
    let (errors, unchecked) = borrowck_source_in("
        pub fn main() {
            let mut x = None;
            let y = x;
            let z = Default::default();
            let mut w = (&mut x, z, z);
            let a = (&mut None::<fn(&())>, y, None::<fn(&'static ())>);
            w = a;
        }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_reference_to_a_local_returned_as_an_opaque_type_names_the_universal_region() {
    // The argument's region is named (`'1`), so the borrow is explained by the opaque type's constraint:
    // E0597 "opaque type requires that `y` is borrowed for `'1`" (regions/do-not-suggest-adding-bound-to-opaque-type).
    let (errors, unchecked) = borrowck_source_in("
        pub fn b(x: &u32) -> impl Sized + '_ { let y = 5u32; &y }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0597") && errors[0].contains("opaque type requires that `y` is borrowed for `'1`"), "{errors:?}");
}

#[test]
fn an_async_closure_returned_as_a_trait_object_references_a_local() {
    // `(async || x)()` borrows `x` through a coroutine-closure, which is neither a closure nor a coroutine
    // for the E0373 explanation: the error is E0515.
    let (errors, unchecked) = borrowck_source_in("
        pub fn foo() -> Box<dyn std::future::Future<Output = u32>> { let x = 0u32; Box::new((async || x)()) }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0515"), "{errors:?}");
}

#[test]
fn a_local_borrowed_by_a_returned_closure_or_async_block_is_e0373() {
    // The returned value's region is boring (it outlives the `impl Trait` argument's universal region), so
    // the borrow is explained by the constraint that returns it: a closure that escapes (E0373), not the
    // generic "borrowed value does not live long enough".
    let (errors, unchecked) = borrowck_source_in("
        pub fn boxed() -> Box<impl Fn() -> u32> { let x = 0u32; Box::new(|| x) }
        pub fn by_ref(x: &u32) -> impl Fn() -> u32 + '_ { || *x }
        pub async fn late() {}
        pub fn block(x: &u32) -> impl std::future::Future<Output = u32> + '_ { async { *x } }", 2018);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert_eq!(errors.iter().filter(|e| e.starts_with("E0373")).count(), 3, "{errors:?}");
    assert_eq!(errors.len(), 3, "{errors:?}");
}

#[test]
fn calling_a_function_that_returns_an_opaque_type_is_no_defining_use() {
    // rustc's default solver does not define an opaque type while normalizing a callee's signature
    // (impl-trait/member-constraints/incomplete-constraint.rs).
    let (errors, unchecked) = borrowck_source_in("
        fn new_defining_use<F: FnOnce(T) -> R, T, R>(_: F) {}
        pub fn rpit1<'a, 'b: 'b>(x: &'b ()) -> impl Sized + use<'a, 'b> { new_defining_use(rpit1::<'a, 'b>); x }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_closure_stored_in_a_local_then_returned_as_an_opaque_type_is_e0597() {
    // The returned value reaches the opaque type through the assignment to the local (category `Assignment`,
    // "assignment requires that `p` is borrowed for `'static`"): no E0373 (closures/2229_closure_analysis/
    // diagnostics/borrowck/borrowck-4.rs). Normalizing a place type does not define the opaque type.
    let (errors, unchecked) = borrowck_source_in("
        #[derive(Debug)] struct Point { x: i32, y: i32 }
        pub fn foo() -> impl FnMut() {
            let mut p = Point { x: 1, y: 2 };
            let mut c = || { p.x += 5; println!(\"{:?}\", p); };
            c
        }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.len() == 1 && errors[0].starts_with("E0597"), "{errors:?}");
}

#[test]
fn an_ambiguous_subtype_predicate_is_a_type_annotation_error() {
    // type/type-check/cannot_infer_local_or_array.rs: the local's variable is related to the array type's.
    let errors = typeck_errors_source("pub fn main() { let x = []; }", 2021);
    assert!(errors.iter().any(|e| e.contains("E0282")) && !errors.iter().any(|e| e.contains("E0283")), "{errors:?}");
}

#[test]
fn a_use_of_an_opaque_type_with_non_parameter_arguments_is_e0792() {
    // On stable a non-defining use of an opaque type in its defining scope is a hard error
    // (impl-trait/rpit/non-defining-use.rs, non-defining-use-lifetimes.rs, recursive-ice-101862.rs).
    let (errors, unchecked) = borrowck_source_in("
        #![allow(unconditional_recursion)]
        pub fn foo<T>() -> impl Sized { let _: () = foo::<u8>(); }
        pub fn bar<T>(val: T) -> impl Sized { let _: u8 = bar(0u8); val }
        pub fn ice(x: impl AsRef<str>) -> impl IntoIterator<Item = ()> { vec![].append(&mut ice(x.as_ref())); Vec::new() }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert_eq!(errors.iter().filter(|e| e.starts_with("E0792") && e.contains("expected generic type parameter, found")).count(), 3, "{errors:?}");
    let (errors, _) = borrowck_source_in("
        type Lt<'lt> = Option<*mut &'lt ()>;
        pub fn foo<'a>(_: Lt<'a>) -> impl Sized + 'a { let _: () = foo(Lt::<'static>::None); }
        pub fn equal<'a, 'b>(_: Lt<'a>, _: Lt<'b>) -> impl Sized + 'a + 'b { let _: () = equal(Lt::<'a>::None, Lt::<'a>::None); }", 2021);
    assert!(errors.iter().any(|e| e.starts_with("E0792") && e.contains("expected generic lifetime parameter, found `'static`")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("non-defining opaque type use in defining scope")), "{errors:?}");
}

#[test]
fn a_region_inferred_in_a_closure_signature_is_an_external_region_of_the_closure() {
    // type-alias-impl-trait/implied_bounds_closure.rs: `T`'s region comes from the closure body, so it is erased by writeback
    // and is a universal region of the closure that the creator maps to its own variable (here `'static`, from `T: 'static`).
    let (errors, unchecked) = borrowck_source("
        trait StaticDefaultRef: 'static { fn default_ref() -> &'static Self; }
        impl StaticDefaultRef for str { fn default_ref() -> &'static str { \"\" } }
        fn into_impl(x: &str) -> &(impl ?Sized + AsRef<str> + StaticDefaultRef + '_) { x }
        fn extend_lifetime<'a>(x: &'a str) -> &'static str { let t = into_impl(x); helper(|_| t) }
        fn helper<T: ?Sized + AsRef<str> + StaticDefaultRef>(f: impl FnOnce(&T) -> &T) -> &'static str { f(T::default_ref()).as_ref() }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("lifetime may not live long enough"), "{errors:?}");
}

#[test]
fn a_type_that_must_outlive_a_region_in_a_well_formedness_check_reports_the_bound_failure() {
    // wf/wf-in-fn-type-static.rs, lifetimes/lifetime-doesnt-live-long-enough.rs: wfcheck resolves the regions its
    // obligations registered (E0310 for `'static`, E0309 for a named region).
    let errors = wfcheck_errors("
        struct Foo<T> { x: fn() -> &'static T }
        struct Ok<'a, T> { x: &'a T }
        fn f<'a, T>(_: &'a T) {}
        fn main() {}");
    assert_eq!(errors.iter().filter(|e| e.contains("E0310") && e.contains("may not live long enough")).count(), 1, "{errors:?}");
}

#[test]
fn a_local_live_across_an_await_is_part_of_the_future_for_auto_traits() {
    // rustc mir_coroutine_witnesses: `Send` of an async block waits for the body's MIR, then asks every type
    // of the locals live across a suspension point.
    let (errors, unchecked) = borrowck_source_checked("
        use std::rc::Rc;
        fn needs_send<T: Send>(_: T) {}
        async fn nothing() {}
        pub fn held() { needs_send(async { let rc = Rc::new(1u8); nothing().await; drop(rc); }); }
        pub fn dropped_first() { needs_send(async { { let rc = Rc::new(1u8); drop(rc); } nothing().await; }); }", 2021);
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("future cannot be sent between threads safely"), "{errors:?}");
}

#[test]
fn the_witness_of_another_fns_future_is_made_on_demand() {
    let errors = typeck_errors_checked("
        use std::rc::Rc;
        fn needs_send<T: Send>(_: T) {}
        async fn nothing() {}
        async fn holds() { let rc = Rc::new(1u8); nothing().await; drop(rc); }
        async fn fine() { let n = 1u8; nothing().await; drop(n); }
        pub fn user() { needs_send(holds()); needs_send(fine()); }", 2021);
    assert_eq!(errors.len(), 1, "{errors:?}");
}

#[test]
fn a_nested_closures_erased_signature_regions_are_not_its_creators() {
    // the inner closure's erased output region is its own external region; it must not be the outer closure's erased input region
    let (errors, unchecked) = borrowck_source("
        fn connected(c: &mut u32) -> Option<&mut u32> { Some(c) }
        fn g(c: &mut u32) -> Option<u32> {
            connected(c).and_then(|client| {
                let revisions = vec![*client];
                let previous = Some(1usize).and_then(|index| revisions.get(index))?;
                Some(*previous)
            })
        }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_closures_higher_ranked_signature_regions_survive_normalization_under_the_binder() {
    // higher-ranked/trait-bounds/normalize-under-binder/issue-89436.rs
    let (errors, unchecked) = borrowck_source("
        trait MiniYokeable<'a> { type Output; }
        struct Bar<'a> { string_1: &'a str, string_2: &'a str }
        impl<'a> MiniYokeable<'a> for Bar<'static> { type Output = Bar<'a>; }
        impl<'a> MiniYokeable<'a> for &'static str { type Output = &'a str; }
        fn map<Y: for<'a> MiniYokeable<'a>, P: for<'a> MiniYokeable<'a>>(y: Y, f: impl for<'a> FnOnce(<Y as MiniYokeable<'a>>::Output, core::marker::PhantomData<&'a ()>) -> <P as MiniYokeable<'a>>::Output) {}
        fn demo(bar: Bar<'static>) { map::<_, &'static str>(bar, |bar, _| bar.string_1) }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}
