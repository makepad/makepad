//! The whole MIR borrowck pipeline (build, signatures, type check, region inference, checks) on small
//! Rust sources over Relax's real core and alloc: the repros of what real crates showed.
use crate::program::{DefId, Program};
use crate::tcx::{EvalCx, EvalState, Tcx, TypingEnv};

/// Error codes (or messages) and unchecked reasons of every fn of `source`.
pub(super) fn borrowck_source(source: &str) -> (Vec<String>, Vec<String>) {
    borrowck_source_in(source, std::env::var("PROBE_EDITION").ok().and_then(|e| e.parse().ok()).unwrap_or(2021))
}

pub(super) fn borrowck_source_in(source: &str, edition: u16) -> (Vec<String>, Vec<String>) {
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
    let user = prog.load_crate("repro", path.to_str().unwrap(), edition, crate::cfg::CfgSet::host(sess, &[]), vec![(core, 0), (alloc_name, alloc), (std_name, std)]).unwrap();
    std::fs::remove_file(&path).unwrap(); std::fs::remove_dir(&dir).unwrap();
    prog.prelude_crate = Some(0);
    prog.resolve_imports();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
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
    let (errors, unchecked) = borrowck_source("
        pub struct Inv<T>(*mut T);
        pub type A = for<'a, 'b> fn(Inv<&'a &'b ()>, Inv<&'b &'a ()>, Inv<&'a ()>);
        pub type B = for<'a, 'b> fn(Inv<&'a &'b ()>, Inv<&'b &'a ()>, Inv<&'b ()>);
        pub fn f(x: A) { let _y: B = x; }");
    assert!(unchecked.is_empty(), "{unchecked:?}");
    assert!(!errors.is_empty() && errors.iter().all(|e| e.starts_with("E0308 mismatched types")), "{errors:?}");
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
fn an_async_fn_with_a_pattern_parameter_stops_the_check_instead_of_misreporting() {
    let (errors, unchecked) = borrowck_source_in("
        pub async fn c(ref vec: Vec<u32>) { vec.contains(&0); }", 2018);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(unchecked.iter().any(|u| u.contains("async fn parameter pattern")), "{unchecked:?}");
}

#[test]
fn an_async_closure_with_a_macro_body_orders_its_coroutine_before_it() {
    let (errors, unchecked) = borrowck_source_in("
        pub fn main() { let _ = async || println!(); }", 2018);
    assert!(unchecked.iter().all(|u| u.contains("async closures")), "{unchecked:?}");
    assert!(errors.is_empty(), "{errors:?}");
}
