//! Programs the check's type passes refuse, each with the error it gives: shapes that would otherwise let a
//! borrow outlive its value, read memory never written, or share a value across threads that is not `Sync`.
//! Every program is written here from scratch.
use crate::borrowck::mir_source_tests::load_source;
use crate::program::DefId;
use crate::tcx::{EvalCx, EvalState, TypingEnv};

/// The errors the check's type passes give for the crate `source`, as checked crates get them: the orphan
/// rules, well-formedness of every item, the type check of every body, and the borrow check of each body
/// whose type check passed. Each is "<item path>: <code> <message>".
fn errors(source: &str) -> Vec<String> {
    let (prog, tcx, user) = load(source);
    errors_of(&prog, &tcx, user)
}

/// The crate `source`, loaded and validated as a checked crate is; `user` is its crate.
fn load(source: &str) -> (crate::program::Program, crate::tcx::Tcx, u32) {
    let (mut prog, tcx, user) = load_source(source, 2021, "repro");
    prog.crates[user as usize].validate = true;
    (prog, tcx, user)
}

/// `errors` of a crate `load` gave.
fn errors_of(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, user: u32) -> Vec<String> {
    let mut out = Vec::new();
    for (d, _, e) in tcx.orphan_errors(&prog) {
        if prog.def(d).krate == user { out.push(format!("{}: {e}", prog.def_path(d))); }
    }
    for i in 0..prog.defs.len() {
        let d = DefId(i as u32);
        if prog.def(d).krate != user { continue; }
        out.extend(crate::wfcheck::obligations::check_def(&prog, &tcx, d).into_iter().map(|e| format!("{}: {e}", prog.def_path(d))));
    }
    for owner in crate::borrowck::body::owners(&prog) {
        if prog.def(owner.owner).krate != user { continue; }
        let path = prog.def_path(owner.owner);
        let body = tcx.typeck(&prog, owner.owner);
        if !body.errors.is_empty() {
            out.extend(body.errors.iter().map(|e| format!("{path}: {e}")));
            continue;
        }
        let state = EvalState::new();
        let cx = EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
        let v = crate::borrowck::driver::check_root(&cx, &prog, &tcx, &body, None);
        assert!(v.unchecked.is_empty(), "{path}: {:?}", v.unchecked);
        out.extend(v.errors.iter().map(|e| format!("{path}: {} {}", e.code, e.message)));
    }
    out
}

/// Whether `errors` has one with `code` for the item whose path ends in `item`.
fn has(errors: &[String], item: &str, code: &str) -> bool {
    errors.iter().any(|e| e.split_once(": ").is_some_and(|(path, rest)| (path == item || path.ends_with(&format!("::{item}"))) && rest.starts_with(code)))
}

#[test]
fn an_inherited_check_failure_names_its_first_cause() {
    // The missing field type is already an error at collection; check the body that reads it too.
    let source = "#![no_std]
        fn d() -> Missing { loop {} }
        struct S { f: Missing }
        fn c(s: S) -> u32 { let _x = s.f; 0 }";
    let path = std::env::temp_dir().join(format!("relax-taint-{}.rs", crate::unique_test_suffix()));
    std::fs::write(&path, source).unwrap();
    let session = std::sync::Arc::new(crate::Session::default());
    let mut prog = crate::program::Program::new(&session);
    let user = prog.load_crate("repro", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    std::fs::remove_file(path).unwrap();
    prog.crates[user as usize].validate = true;
    prog.resolve_imports();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let check = |name: &str| {
        let owner = crate::borrowck::body::owners(&prog)
            .find(|o| prog.def(o.owner).krate == user && prog.name(o.owner) == name)
            .unwrap_or_else(|| panic!("missing body for {name}"));
        tcx.typeck(&prog, owner.owner)
    };
    let d = check("d");
    let c = check("c");
    assert_eq!(c.errors.len(), 1, "caller's inherited error: c {:?}, d {:?}", c.errors, d.errors);
    let (_, summary) = c.errors[0].split_once(": ").expect("caller error has a source position");
    let site = summary.strip_prefix("type checking failed due to previous errors (tainted by an error type at relax-types src/")
        .and_then(|site| site.strip_suffix(')')).and_then(|site| site.rsplit_once(':'));
    assert!(site.is_some_and(|(file, line)| file.ends_with(".rs") && line.parse::<u32>().is_ok_and(|n| n > 0)),
        "caller's first taint site: {summary}");
    assert_eq!(d.errors.len(), 2, "owner's errors: {:?}", d.errors);
    assert!(d.errors[0].ends_with(":2:19: E0425: cannot find type `Missing` in this scope"), "owner's missing type: {:?}", d.errors);
    assert!(d.errors[1].ends_with(":2:9: signature has unresolved types -- fix the signature errors first"),
        "owner's signature error: {:?}", d.errors);
}

#[test]
fn bare_struct_variant_ident_is_binding_not_path_pattern() {
    let e = errors("
        pub enum Role { Host, Viewer { sharer: u32 }, EmptyStruct {} }
        pub fn single(r: Role) -> Role {
            use Role::Viewer;
            let Viewer = Role::Host;
            let _: Role = Viewer;
            match r { Viewer => Viewer }
        }
        pub fn glob(r: &Role) {
            use Role::*;
            match r { Viewer => { let _: &Role = Viewer; } }
            match r { EmptyStruct => { let _: &Role = EmptyStruct; } }
        }
        pub fn modes(r: Role) -> Role {
            use Role::Viewer;
            match r { ref Viewer => { let _: &Role = Viewer; } }
            match r { mut Viewer => { Viewer = Role::Host; match Viewer { Viewer @ _ => Viewer } } }
        }
        mod a { pub enum A { Same { x: u32 }, Other } }
        mod b { pub enum B { Same { y: u32 } } }
        pub fn ambiguous() {
            use a::A::*;
            use b::B::*;
            match a::A::Other { Same => { let _: a::A = Same; } }
        }");
    assert!(e.is_empty(), "legal struct variant bindings: {e:?}");

    let e = errors("
        pub enum Role { Host, Viewer { sharer: u32 } }
        use Role::{Host, Viewer};
        pub fn dup(a: Role, b: Role) { match (a, b) { (Viewer, Viewer) => {} } }
        pub fn or_miss(r: Role) { match r { Viewer | _ => {} } }
        pub fn or_mode(r: Role) { match r { mut Viewer | Viewer => {} } }
        pub fn dup_param(Viewer: Role, Viewer: Role) {}
        pub fn dup_mut_host_param(mut Host: Role, mut Host: Role) {}
        pub fn unit_ok(a: Role, b: Role) {
            match a { Host | _ => {} }
            match (a, b) { (Host, Host) => {}, _ => {} }
        }");
    for (f, code) in [("dup", "E0416"), ("or_miss", "E0408"), ("or_mode", "E0409"),
                      ("dup_param", "E0415"), ("dup_mut_host_param", "E0415")] {
        assert!(e.iter().any(|x| x.starts_with(&format!("repro::{f}: ")) && x.contains(&format!(": {code}: "))), "{f}: expected {code}, got {e:?}");
    }
    assert!(!e.iter().any(|x| x.starts_with("repro::unit_ok: ")), "unit variant paths: {e:?}");
}

#[test]
fn glob_value_beside_braced_variant_positions() {
    let (prog, tcx, user) = load("
        mod a { pub enum A { Same { x: u32 } } }
        mod b { pub const Same: u32 = 1; }
        use a::A::*;
        use b::*;
        pub fn local() { let Same = 0u32; }
        pub fn param(Same: u32) {}
        pub async fn async_param(Same: u32) {}
        pub fn closure() { let c = |Same: u32| {}; }
        pub fn for_loop() { for Same in [0u32] {} }
        mod ambiguous {
            enum A { Same }
            use self::A::*;
            use super::b::*;
            pub fn pick(v: u32) -> u32 { match v { Same => 1, _ => 2 } }
        }
        enum PrivateEnum { BracedVariant { x: u32 } }
        pub use PrivateEnum::BracedVariant;");
    for (name, origin) in [
        ("local", "local binding"), ("param", "function argument"), ("async_param", "function argument"),
        ("closure", "closure argument"), ("for_loop", "`for` loop binding"),
    ] {
        let owner = crate::borrowck::body::owners(&prog).find(|o| prog.def(o.owner).krate == user && prog.name(o.owner) == name).unwrap();
        let body = tcx.typeck(&prog, owner.owner);
        assert!(body.errors.is_empty(), "{name}: {:?}", body.errors);
        let errors = crate::pattern_analysis::check_body(&prog, &tcx, &body).errors;
        assert_eq!(errors.len(), 1, "{name}: {errors:?}");
        assert!(errors[0].contains(&format!("E0005: refutable pattern in {origin}")), "{name}: {errors:?}");
    }
    let owner = crate::borrowck::body::owners(&prog).find(|o| prog.def(o.owner).krate == user && prog.name(o.owner) == "pick").unwrap();
    let body = tcx.typeck(&prog, owner.owner);
    assert_eq!(body.errors.len(), 1, "ambiguous pattern: {:?}", body.errors);
    assert!(body.errors[0].contains("E0659: `Same` is ambiguous"), "{:?}", body.errors);

    let mut own = vec![false; prog.crates.len()];
    own[user as usize] = true;
    let privacy = prog.import_privacy_errors(crate::program::Judged::crates(&own));
    assert_eq!(privacy.len(), 1, "private enum re-export: {privacy:?}");
    assert!(privacy[0].2.contains("E0365") && privacy[0].2.contains("BracedVariant"), "{privacy:?}");
}

#[test]
fn non_unit_path_patterns_are_refused() {
    let (prog, tcx, user) = load("
        pub enum Role { Host, Member(u32), Viewer { sharer: u32 }, Empty {}, EmptyTup() }
        pub type Alias = Role;
        pub struct Pair(pub u32);
        pub struct Unit;
        pub struct Braced { pub x: u32 }
        pub type BracedAlias = Braced;
        pub union Union { pub x: u32 }
        pub trait Tr { type Out; fn method(); }
        pub struct Holder;
        impl Tr for Holder { type Out = Role; fn method() {} }
        pub trait NonAdt { fn non_adt(self); }
        pub fn fun(_: u32) -> Role { Role::Host }
        pub static ST: bool = true;
        use Role::Viewer;

        impl Role {
            pub fn make(_: u32) -> Self { Self::Host }
            pub fn self_paths(self) {
                match self { Self::Viewer => {}, Self::Member => {}, Self::make => {}, _ => {} }
            }
            pub fn self_calls(self) {
                match self { Self::Host() => {}, Self::Viewer(x) | Self::make(x) => { let _ = x; }, _ => {} }
            }
            pub fn enum_self(self) { match self { Self => {} } }
        }
        impl Pair { pub fn pair_self(self) { match self { Self => {} } } }
        impl Union { pub fn union_self(self) { match self { Self => {} } } }
        impl BracedAlias { pub fn alias_self(self) { match self { Self => {} } } }
        impl NonAdt for u32 { fn non_adt(self) { match self { Self => {} } } }
        impl Braced {
            pub fn braced_self(self) { match self { Self => {} } }
            pub fn braced_call(self) { match self { Self(x) => { let _ = x; } } }
        }
        impl Unit { pub fn unit_call(self) { match self { Self() => {} } } }
        pub fn relative(r: Role) {
            match r { Alias::Viewer => {}, <Role>::Viewer => {}, <Holder as Tr>::Out::Viewer => {},
                      Alias::Member => {}, <Role>::Member => {}, _ => {} }
        }
        pub fn rejected_path(r: &Role) { match r { Role::Viewer => {}, _ => {} } }
        pub fn rejected_tuple(r: &Role) { match r { Role::Viewer(x) => { let _ = x; }, _ => {} } }
        pub fn direct(r: Role) {
            match r { Role::Member => {}, Role::EmptyTup => {}, Role::Empty => {}, Role::make => {}, _ => {} }
        }
        pub fn names(r: Role) {
            match r { self::Viewer => {}, self::Braced => {}, self::Pair => {}, self::fun => {},
                      Tr::method => {}, <Holder as Tr>::method => {}, self::ST => {}, _ => {} }
        }
        pub fn assoc_type(v: u32) { match v { Tr::Out => {}, _ => {} } }
        pub fn qualified_type(v: u32) { match v { <Holder as Tr>::Out => {}, _ => {} } }
        macro_rules! pass { ($p:path) => { $p }; }
        pub fn macro_path_viewer(r: Role) { match r { pass!(Role::Viewer) => {}, _ => {} } }
        pub fn tuple_names<const N: u32>(v: u32) {
            match v { pass!(N) => {}, N(_) => {}, Role::Host() => {}, self::fun(_) => {}, _ => {} }
        }
        pub fn generic_fn(v: u32) { match v { Vec::new => {}, _ => {} } }
        pub fn infer() { let Role::Viewer = Default::default(); }
        pub fn assign_viewer(r: Role, mut v: u32) { Role::Viewer(v) = r; }
        pub fn assign_method(r: Role, mut v: u32) { Role::make(v) = r; }
        pub fn assign_fn(r: Role, v: u32) { self::fun(v) = r; }
        pub fn assign_unit(r: Role) { Role::Host() = r; }
        pub fn assign_braced(r: Braced, v: u32) { self::Braced(v) = r; }
        pub fn nested_viewer(r: Role, mut v: u32, mut x: u32) { (Role::Viewer(v), x) = (r, 1); }
        pub fn nested_fn(r: Role, v: u32, mut x: u32) { (self::fun(v), x) = (r, 1); }
        pub enum E { V { x: u32 } }
        pub fn const_enum_prefix<const E: usize>(x: self::E) {
            match x { E::V => {} _ => {} }
        }
        mod projection {
            pub trait HasRole { type Out; }
            pub struct Holder;
            pub enum Role { Viewer { sharer: u32 } }
            impl HasRole for Holder { type Out = Role; }
            type A = <Holder as HasRole>::Out;
            pub fn const_alias_prefix<const A: usize>() -> u32 {
                let r = A::Viewer { sharer: 1 };
                match r { A::Viewer { sharer } => sharer }
            }
        }
    ");

    let bare_self = "E0533: expected unit struct, found self constructor `Self`";
    let viewer = "E0533: expected unit struct, unit variant or constant, found struct variant `Role::Viewer`";
    let viewer_call = "E0164: expected tuple struct or tuple variant, found struct variant `Role::Viewer`";
    let invalid_lhs = "E0070: invalid left-hand side of assignment";
    let cases: &[(&str, &[&str])] = &[
        ("self_paths", &[
            "E0533: expected unit struct, unit variant or constant, found struct variant `Self::Viewer`",
            "E0533: expected unit struct, unit variant or constant, found tuple variant `Self::Member`",
            "E0533: expected unit struct, unit variant or constant, found associated function `Self::make`",
        ]),
        ("self_calls", &[
            "E0164: expected tuple struct or tuple variant, found unit variant `Self::Host`",
            "E0164: expected tuple struct or tuple variant, found struct variant `Self::Viewer`",
            "E0164: expected tuple struct or tuple variant, found associated function `Self::make`",
        ]),
        ("pair_self", &[bare_self]), ("braced_self", &[bare_self]), ("alias_self", &[bare_self]),
        ("enum_self", &[bare_self]), ("union_self", &[bare_self]), ("non_adt", &[bare_self]),
        ("unit_call", &["E0164: expected tuple struct or tuple variant, found unit struct `Self`"]),
        ("braced_call", &[
            "error: the `Self` constructor can only be used with tuple or unit structs",
            "E0164: expected tuple struct or tuple variant, found self constructor `Self`",
        ]),
        ("relative", &[
            "E0533: expected unit struct, unit variant or constant, found struct variant `Alias::Viewer`",
            viewer,
            "E0533: expected unit struct, unit variant or constant, found struct variant `<<Holder as Tr>::Out>::Viewer`",
            "E0533: expected unit struct, unit variant or constant, found tuple variant `Alias::Member`",
            "E0533: expected unit struct, unit variant or constant, found tuple variant `Role::Member`",
        ]),
        ("rejected_path", &[viewer]), ("rejected_tuple", &[viewer_call]),
        ("macro_path_viewer", &[viewer]),
        ("direct", &[
            "E0532: expected unit struct, unit variant or constant, found tuple variant `Role::Member`",
            "E0532: expected unit struct, unit variant or constant, found tuple variant `Role::EmptyTup`",
            "E0533: expected unit struct, unit variant or constant, found struct variant `Role::Empty`",
            "E0533: expected unit struct, unit variant or constant, found associated function `Role::make`",
        ]),
        ("names", &[
            "E0532: cannot find unit struct, unit variant or constant `Viewer` in module `self`",
            "E0532: cannot find unit struct, unit variant or constant `Braced` in module `self`",
            "E0532: expected unit struct, unit variant or constant, found tuple struct `self::Pair`",
            "E0532: expected unit struct, unit variant or constant, found function `self::fun`",
            "E0532: expected unit struct, unit variant or constant, found associated function `Tr::method`",
            "E0532: expected unit struct, unit variant or constant, found associated function `Tr::method`",
            "E0532: expected unit struct, unit variant or constant, found static `self::ST`",
        ]),
        ("assoc_type", &["E0782: expected a type, found a trait"]),
        ("qualified_type", &["E0575: cannot find method or associated constant `Out` in trait `Tr`"]),
        ("tuple_names", &[
            "E0532: expected unit struct, unit variant or constant, found const parameter `N`",
            "E0532: expected tuple struct or tuple variant, found const parameter `N`",
            "E0532: expected tuple struct or tuple variant, found unit variant `Role::Host`",
            "E0532: expected a pattern, found a function call",
        ]),
        ("generic_fn", &["E0533: expected unit struct, unit variant or constant, found associated function `Vec::new`"]),
        ("infer", &[viewer]),
        ("assign_viewer", &[viewer_call]), ("nested_viewer", &[viewer_call]),
        ("assign_method", &["E0164: expected tuple struct or tuple variant, found associated function `Role::make`"]),
        ("assign_fn", &[invalid_lhs]), ("nested_fn", &[invalid_lhs]),
        ("assign_unit", &["E0618: expected function", invalid_lhs]),
        ("assign_braced", &["E0425: cannot find function, tuple struct or tuple variant", invalid_lhs]),
        ("const_enum_prefix", &["E0533: expected unit struct, unit variant or constant, found struct variant `E::V`"]),
        ("const_alias_prefix", &[]),
    ];
    let owners: Vec<_> = crate::borrowck::body::owners(&prog).filter(|o| prog.def(o.owner).krate == user).collect();
    for &(name, expected) in cases {
        let owner = owners.iter().find(|o| prog.name(o.owner) == name).unwrap_or_else(|| panic!("missing body: {name}"));
        let body = tcx.typeck(&prog, owner.owner);
        let mut errors: Vec<_> = body.errors.iter().collect();
        for message in expected {
            let index = errors.iter().position(|e| e.contains(*message))
                .unwrap_or_else(|| panic!("{name}: missing {message}, got {:?}", body.errors));
            errors.remove(index);
        }
        assert!(errors.is_empty(), "{name}: unexpected diagnostics: {errors:?}");

        if matches!(name, "rejected_path" | "rejected_tuple") {
            assert!(body.pat_res.iter().any(|(_, r)| *r == crate::typeck::Res::Err), "{name}: missing error resolution");
            assert!(body.pat_derefs.is_empty(), "{name}: invalid pattern peeled the reference");
            assert!(body.pat_discriminant_reads.is_empty(), "{name}: invalid pattern read a tag");
            let analysis = crate::pattern_analysis::check_body(&prog, &tcx, &body);
            assert!(analysis.errors.is_empty(), "{name}: {:?}", analysis.errors);
        }
    }

    // In edition 2015 the leading :: selects the crate root, even beside a type parameter.
    let (mut prog, tcx, user) = load_source("
        pub enum E { V { x: u32 } }
        pub fn global_braced<E>(_e: E, v: ::E) -> u32 {
            let w = ::E::V { x: 1 };
            match w { ::E::V { x } => x + match v { ::E::V { x } => x } }
        }
        pub fn global_bare<E>(_e: E, v: ::E) { match v { ::E::V => {} } }
    ", 2015, "repro");
    prog.crates[user as usize].validate = true;
    let errors = errors_of(&prog, &tcx, user);
    assert!(!errors.iter().any(|e| e.starts_with("repro::global_braced: ")), "global_braced: {errors:?}");
    assert_eq!(errors.len(), 1, "global_bare: {errors:?}");
    assert!(errors[0].starts_with("repro::global_bare: ") && errors[0].contains("E0533: expected unit struct, unit variant or constant, found struct variant"), "global_bare: {errors:?}");
}

/// `let _ = *r;` mentions the place behind `r` without using its value. Going through the `Deref` reads the
/// pointer `r`, so with `r` never initialized it is E0381, as an assignment through it would give. A mention
/// without a `Deref` reads nothing and stays accepted.
#[test]
fn a_place_mention_reads_the_pointer_it_goes_through() {
    let e = errors("
        pub fn deref() { let r: &u8; let _ = *r; }
        pub fn field() { let r: &(u8, u8); let _ = (*r).1; }
        pub fn twice() { let r: &&u8; let _ = **r; }
        pub fn matched() { let r: &u8; match *r { _ => {} } }
        pub fn tuple() { let r: &(u8, u8); let (_, _) = *r; }
        pub fn if_let() { let r: &u8; if let _ = *r {} }
        pub fn boxed() { let b: Box<u8>; let _ = *b; }
        pub fn fine(s: String) { let x: u8; let _ = x; drop(s); let _ = s; let r = &1u8; let _ = *r; }");
    for f in ["deref", "field", "twice", "matched", "tuple", "if_let", "boxed"] { assert!(has(&e, f, "E0381"), "{f}: {e:?}"); }
    assert!(!e.iter().any(|x| x.contains("fine")), "{e:?}");
}

/// `dyn TwoSuper<X, X>: Super<X>` holds through either supertrait in the type check, but the borrow check
/// (fresh regions in each position) finds the two candidates ambiguous. It must not take the ambiguity as
/// success and drop its region constraints, which would let `extend` return the short `&'a u8` it stored as
/// `&'static u8`: the goal is reported (E0283).
#[test]
fn an_ambiguous_goal_in_the_borrow_check_is_not_a_success() {
    let e = errors("
        use std::cell::Cell;
        pub trait Super<T> { fn put(&self, t: T); fn take(&self) -> T; }
        pub trait TwoSuper<A, B>: Super<A> + Super<B> {}
        pub struct Slot<X>(Cell<Option<X>>);
        impl<X> Super<X> for Slot<X> {
            fn put(&self, t: X) { self.0.set(Some(t)) }
            fn take(&self) -> X { self.0.take().unwrap() }
        }
        impl<X> TwoSuper<X, X> for Slot<X> {}
        pub fn mk<'s, X: 's>() -> Box<dyn TwoSuper<X, X> + 's> { Box::new(Slot(Cell::new(None))) }
        pub fn extend<'a>(z: &'a u8) -> &'static u8 { let cell = mk(); cell.put(z); cell.take() }");
    assert!(has(&e, "extend", ""), "{e:?}");
    assert!(!has(&e, "mk", ""), "{e:?}");
}

/// A value holding `PhantomData<*const ()>` has no interior mutability but is not `Sync`; promoting its shared
/// borrow in a fn body would make one `&'static` allocation every thread can reach, which a `static` of the type
/// is refused for. Not promoted in fn and const fn bodies (E0515); values that hold nothing that is not `Sync`
/// (`&[]`, `&None` for any `T`) stay promoted, and a const item's own lifetime-extended value stays.
#[test]
fn a_promoted_reference_to_a_value_that_is_not_sync_is_refused() {
    let e = errors(r#"
        use std::marker::PhantomData;
        pub struct NotSync(pub u8, PhantomData<*const ()>);
        pub const N: NotSync = NotSync(1, PhantomData);
        pub const CI: &NotSync = &NotSync(4, PhantomData);
        pub fn g() -> &'static NotSync { &NotSync(2, PhantomData) }
        pub fn h() -> &'static NotSync { &N }
        pub const fn k() -> &'static NotSync { &NotSync(3, PhantomData) }
        pub fn ok() -> &'static (u8, &'static str) { &(1, "x") }
        pub fn empty<T>() -> &'static [T] { &[] }
        pub fn none<T>() -> &'static Option<T> { &None }"#);
    for f in ["g", "h", "k"] { assert!(has(&e, f, "E0515"), "{f}: {e:?}"); }
    for f in ["CI", "N", "ok", "empty", "none"] { assert!(!has(&e, f, ""), "{f}: {e:?}"); }
}

/// `S<T: 'static>` built from a `&'static u8` is a covariant `S<&'s u8>`; assigning a short borrow to its field
/// shrinks `'s`, and the value would then reach `impl<T: Clone> Tr for S<T>`, whose body assumes `T: 'static`,
/// through `dyn Tr`, and a short borrow come back as `&'static u8`. A local's type must be well-formed.
#[test]
fn a_local_of_a_type_whose_bounds_fail_is_refused() {
    let e = errors("
        use std::any::Any;
        pub struct S<T: 'static>(pub T);
        pub trait Tr { fn to_any(&self) -> Box<dyn Any>; }
        impl<T: Clone> Tr for S<T> { fn to_any(&self) -> Box<dyn Any> { Box::new(self.0.clone()) } }
        pub fn extend<'x>(x: &'x u8) -> &'static u8 {
            static Z: u8 = 0;
            let mut s = S(&Z);
            s.0 = x;
            let r: &(dyn Tr + 'x) = &s;
            *r.to_any().downcast::<&'static u8>().unwrap()
        }
        pub fn fine() -> u8 { static Z: u8 = 1; let s = S(&Z); let r: &dyn Tr = &s; let _ = r.to_any(); *s.0 }");
    assert!(has(&e, "extend", ""), "{e:?}");
    assert!(!has(&e, "fine", ""), "{e:?}");
}

/// A bound that only an alias's arguments carry, `'b: 'a` from `<&'a &'b () as Trait>::Type`, is gone once the
/// alias normalizes to `()`, and a fn item used through the `Fn` traits is called without its unnormalized
/// signature being proven. The body may not assume it: implied bounds come from the normalized type, so an
/// alias that normalizes away contributes nothing from its arguments (lifetime may not live long enough). An
/// alias that stays rigid keeps what its arguments and its item's where-clauses imply.
#[test]
fn a_bound_only_an_alias_argument_carries_is_not_implied() {
    let e = errors("
        pub trait Trait { type Type; }
        impl<T> Trait for T { type Type = (); }
        pub fn extend<'a, 'b, T>(_: <&'a &'b () as Trait>::Type, v: &'b T) -> &'a T { v }
        pub fn fine<'a, 'b, T>(_: &'a &'b (), v: &'b T) -> &'a T { v }
        pub fn rigid<'a, X: Trait>(_: &'a X::Type, v: &'a X) -> &'a X { v }
        pub trait Lock { type Guard<'g, T> where T: 'g; fn read<T>(_: &Self) -> Self::Guard<'_, T>; }
        pub fn guarded<'a, L: Lock, T>(_: L::Guard<'a, T>) -> Option<&'a T> { None }");
    assert!(has(&e, "extend", ""), "{e:?}");
    for f in ["fine", "rigid", "read", "guarded"] { assert!(!has(&e, f, ""), "{f}: {e:?}"); }
}

/// An auto trait's goals are coinductive, but the where-clause of a written impl is no productive step:
/// `unsafe impl<T> Sync for S<T> where S<T>: Sync` does not prove itself, nor do two impls that each ask for
/// the other (E0275). A cycle through the fields of a type still holds.
#[test]
fn an_impl_whose_where_clause_needs_itself_proves_nothing() {
    let e = errors(r#"
        use std::marker::PhantomData;
        pub struct S<T>(PhantomData<*const T>);
        unsafe impl<T> Sync for S<T> where S<T>: Sync {}
        pub struct A<T>(PhantomData<*const T>);
        pub struct B<T>(PhantomData<*const T>);
        unsafe impl<T> Send for A<T> where B<T>: Send {}
        unsafe impl<T> Send for B<T> where A<T>: Send {}
        pub struct Tree(pub Option<Box<Tree>>, pub u8);
        fn sync<T: Sync>() {}
        fn send<T: Send>() {}
        pub fn itself() { sync::<S<u8>>(); }
        pub fn pair() { send::<A<u8>>(); }
        pub fn tree() { send::<Tree>(); sync::<Tree>(); }"#);
    // (a type check error carries its location before the code)
    for f in ["itself", "pair"] { assert!(e.iter().any(|x| x.starts_with(&format!("repro::{f}: ")) && x.contains(": E0275: ")), "{f}: {e:?}"); }
    assert!(!has(&e, "tree", ""), "{e:?}");
}

/// `for<'a, 'b> Sub<'a, 'b, &'b &'a ()>` carries `'a: 'b` in its arguments. Elaborated to
/// `for<'a, 'b> Super<'a, 'b>` it would lose that bound and let a generic call or a dyn method extend a
/// borrow, so it is not elaborated (E0277 for the call). Retained arguments and Fn to FnMut stay valid.
#[test]
fn a_higher_ranked_bound_keeps_the_bounds_its_arguments_imply() {
    let e = errors("
        pub trait Sub<'a, 'b, R>: Super<'a, 'b> {}
        pub trait Super<'a, 'b> { fn conv<T: ?Sized>(x: &'a T) -> &'b T; }
        pub struct S;
        impl<'a: 'b, 'b> Super<'a, 'b> for S {
            fn conv<T: ?Sized>(x: &'a T) -> &'b T { x }
        }
        impl<'a, 'b> Sub<'a, 'b, &'b &'a ()> for S {}
        fn via_super<'x, 'y, X: for<'a, 'b> Super<'a, 'b>, T: ?Sized>(v: &'x T) -> &'y T { X::conv(v) }
        pub fn via_sub<'x, 'y, X: for<'a, 'b> Sub<'a, 'b, &'b &'a ()>, T: ?Sized>(v: &'x T) -> &'y T {
            via_super::<X, T>(v)
        }

        pub trait Sub2<'a, 'b, R>: Super2<'a, 'b> {}
        pub trait Super2<'a, 'b> { fn conv2(&self, x: &'a str) -> &'b str; }
        impl<'a: 'b, 'b> Super2<'a, 'b> for S {
            fn conv2(&self, x: &'a str) -> &'b str { x }
        }
        impl<'a, 'b> Sub2<'a, 'b, &'b &'a ()> for S {}
        pub fn dyn_extend<'x>(v: &'x str) -> &'static str {
            let d: &dyn for<'a, 'b> Sub2<'a, 'b, &'b &'a ()> = &S;
            d.conv2(v)
        }

        pub trait Keep<'a, 'b, R>: KeepSuper<'a, 'b, R> {}
        pub trait KeepSuper<'a, 'b, R> {}
        fn via_keep_super<X: for<'a, 'b> KeepSuper<'a, 'b, &'b &'a ()>>() {}
        pub fn via_keep<X: for<'a, 'b> Keep<'a, 'b, &'b &'a ()>>() { via_keep_super::<X>() }
        fn via_fn_mut<G: for<'a> FnMut(&'a u8) -> &'a u8>(mut g: G, v: &u8) -> &u8 { g(v) }
        pub fn via_fn<F: for<'a> Fn(&'a u8) -> &'a u8>(f: F, v: &u8) -> &u8 { via_fn_mut(f, v) }");
    assert!(e.iter().any(|x| x.starts_with("repro::via_sub: ") && x.contains(": E0277: ")), "{e:?}");
    assert!(has(&e, "dyn_extend", ""), "{e:?}");
    for f in ["via_super", "via_keep_super", "via_keep", "via_fn_mut", "via_fn"] { assert!(!has(&e, f, ""), "{f}: {e:?}"); }
    assert!(e.iter().all(|x| x.starts_with("repro::via_sub: ") || x.starts_with("repro::dyn_extend: ")), "{e:?}");
}

/// A callee's clauses are proved together: `ndu(a2, &local)` needs `<a2 as FnOnce(u8)>::Output == R`, where the
/// output is the caller's own opaque type and `R` the type the call gave it, and `T: 'static`. The first holds, so
/// the second still requires `local` to be borrowed for `'static` (E0597); with a `'static` argument it passes.
#[test]
fn a_callee_bound_on_the_callers_own_opaque_output_keeps_its_other_bounds() {
    let e = errors("
        fn ndu<F: FnOnce(u8) -> R, R, T: 'static>(_: F, _: T) {}
        pub fn a2(x: u8) -> impl Sized { let local = 1u8; ndu(a2, &local); x }
        pub fn ok(x: u8) -> impl Sized { ndu(ok, 5u8); x }");
    assert!(has(&e, "a2", "E0597") && e.len() == 1, "{e:?}");
}

/// A higher-ranked closure over a reference to a value holding an alias: the closure's binder assumes its
/// signature well formed, the alias outliving the binder's region among it, so that obligation of its other side
/// is decided by the assumption rather than by the alias's declared bound (an opaque capturing a borrow, a plain
/// or a region-carrying projection that stays rigid, two opaques, a fn pointer next to one). Reifying `foo` at a
/// type holding an opaque stays refused.
#[test]
fn a_higher_ranked_closure_assumes_what_an_alias_in_its_argument_outlives() {
    let e = errors("
        pub struct M { pub a: f32, pub b: f32 }
        fn chords(m: &M) -> (usize, impl Fn(f32) -> f32 + '_) { (1, move |f| m.a * f + m.b) }
        pub fn exact(ms: &[M]) -> f32 {
            let mut t = 0.0;
            for m in ms {
                let arc = (m.a > 0.0).then(|| chords(m));
                let one = arc.as_ref().is_none_or(|(n, _)| *n == 1);
                let mid = arc.as_ref().filter(|_| one).map(|(_, at)| at(0.5));
                t += mid.unwrap_or(0.0);
            }
            t
        }
        pub fn only_then(m: &M) -> usize { let arc = (m.a > 0.0).then(|| chords(m)); arc.map_or(0, |(n, _)| n) }
        pub fn none_or(m: &M) -> bool { let arc = Some(chords(m)); arc.as_ref().is_none_or(|(n, _)| *n == 1) }
        pub fn map_ref(m: &M) -> Option<f32> { let arc = Some(chords(m)); arc.as_ref().map(|(_, at)| at(0.5)) }
        pub fn filter_ref(m: &M) -> bool { let arc = Some(chords(m)); arc.as_ref().filter(|_| true).is_some() }
        pub fn filter_own(m: &M) -> bool { let arc = Some(chords(m)); arc.filter(|_| true).is_some() }
        fn hr<T>(x: &T, f: impl for<'r> Fn(&'r T) -> bool) -> bool { f(x) }
        pub fn higher(m: &M) -> bool { let arc = chords(m); hr(&arc, |x| x.0 == 1) }
        pub fn higher_ref(m: &M) -> bool { let arc = chords(m); hr(&&arc, |x| x.0 == 1) }
        pub struct Hold<'h>(pub &'h M);
        pub fn higher_struct(m: &M) -> bool { let h = Hold(m); hr(&h, |x| x.0.a > 0.0) }
        fn mk<T: Fn() -> u8>(t: T) -> impl Fn() -> u8 { t }
        pub fn captured<T: Fn() -> u8>(t: T) -> bool { let o = mk(t); hr(&o, |x| x() == 1) }
        pub fn item<I: Iterator>(v: I::Item) -> bool { hr(&v, |_| true) }
        pub fn rigid_ref<'a, T>(v: <&'a T as IntoIterator>::IntoIter) -> bool where &'a T: IntoIterator { hr(&v, |_| true) }
        pub fn two(m1: &M, m2: &M) -> bool { hr(&(chords(m1), chords(m2)), |x| (x.0).0 == (x.1).0) }
        fn twice(x: &u8) -> u8 { *x * 2 }
        pub fn with_ptr(m: &M) -> bool { hr(&(chords(m), twice as fn(&u8) -> u8), |x| (x.1)(&1) == 2) }
        static UNIT: &'static &'static () = &&();
        fn foo<'a, 'b, T>(_: &'a &'b (), v: &'b T) -> &'a T { v }
        pub fn reify_opaque(m: &M) -> usize {
            let arc = chords(m);
            let f: for<'s> fn(&'static &'static (), &'s (usize, _)) -> &'static (usize, _) = foo;
            f(UNIT, &arc).0
        }");
    for f in ["chords", "exact", "only_then", "none_or", "map_ref", "filter_ref", "filter_own", "hr", "higher", "higher_ref", "higher_struct",
        "mk", "captured", "item", "rigid_ref", "two", "twice", "with_ptr", "foo"] { assert!(!has(&e, f, ""), "{f}: {e:?}"); }
    assert!(has(&e, "reify_opaque", ""), "{e:?}");
}

/// `foo` relies on `'b: 'a`, which its first argument's type implies. Reified to
/// `for<'s> fn(&'static &'static (), &'s T) -> &'static T`, or first to its own signature and then by subtyping
/// to that one, it would turn any borrow into a `'static` one. A binder opened with fresh variables proves its
/// instantiated signature well formed; one opened with placeholders assumes its own signature well formed, so
/// the pointers whose signature gives the bounds stay accepted, also where a fn pointer sits inside another
/// binder's signature (a closure over items holding fn pointers, a closure taking one).
#[test]
fn a_fn_pointer_keeps_the_bounds_its_signature_implies() {
    let e = errors("
        static UNIT: &'static &'static () = &&();
        fn foo<'a, 'b, T>(_: &'a &'b (), v: &'b T) -> &'a T { v }
        pub fn reify<'x, T>(x: &'x T) -> &'static T {
            let f: for<'s> fn(&'static &'static (), &'s T) -> &'static T = foo;
            f(UNIT, x)
        }
        pub fn subtype<'x, T>(x: &'x T) -> &'static T {
            let f: for<'a, 'b> fn(&'a &'b (), &'b T) -> &'a T = foo;
            let g: for<'s> fn(&'static &'static (), &'s T) -> &'static T = f;
            g(UNIT, x)
        }
        pub fn get<S, T>(_: &S) -> &T { loop {} }
        pub fn output<S, T>() -> for<'a> fn(&'a S) -> &'a T { get::<S, T> }
        pub fn nested<'a, 'b>(x: &'a &'b u8) -> &'a u8 { *x }
        pub fn placeholders() -> for<'a, 'b> fn(&'a &'b u8) -> &'a u8 { nested }
        pub struct Foo<'c>(pub &'c u8);
        impl<'c> Foo<'c> { pub fn me(&self) -> &Foo<'c> { self } }
        pub fn free<'c>() -> for<'a> fn(&'a Foo<'c>) -> &'a Foo<'c> { Foo::me }
        pub type Params = fn(&[String]) -> &'static str;
        static QUERIES: std::sync::Mutex<Vec<(&'static str, Params)>> = std::sync::Mutex::new(Vec::new());
        pub fn found(name: &str) -> Option<Params> { QUERIES.lock().unwrap().iter().find(|x| x.0 == name).map(|x| x.1) }
        pub struct Build<'b>(pub &'b mut Vec<u8>);
        impl Build<'_> {
            fn put(&mut self, x: u8) -> u8 { self.0.push(x); x }
            pub fn through(&mut self) -> u8 { let call = |b: &mut Self, f: fn(&mut Self, u8) -> u8| f(b, 1); call(self, Self::put) }
        }");
    for f in ["reify", "subtype"] { assert!(has(&e, f, ""), "{f}: {e:?}"); }
    for f in ["foo", "get", "output", "nested", "placeholders", "me", "free", "found", "put", "through"] { assert!(!has(&e, f, ""), "{f}: {e:?}"); }
}

/// A higher-ranked relation nested in a binder's signature that opens no scope of its own (a `dyn`'s
/// higher-ranked principal) is set aside by the binder when nothing ties it to the binder's other regions: the
/// shapes below stay accepted (`filter` on an `Option<&dyn Fn(&B)>`, through `&mut dyn FnMut`, a chain, an outer
/// lifetime inside the dyn's binder, a borrowed output, two dyns, a user trait, next to an opaque, a reify, and
/// a dyn binder that opens only variables). A nested placeholder tied to the binder's own stays refused, and so
/// does `foo`'s lifetime extension with a higher-ranked dyn added to both signatures.
#[test]
fn a_nested_higher_ranked_relation_is_set_aside_by_its_binder() {
    let e = errors(r#"
        // `pred` and the csg-sdf expression it was reduced from
        pub type Bounds = ([f64; 3], [f64; 3]);
        
        pub fn pred<'a>(slope: Option<&'a dyn Fn(&Bounds, f64) -> f64>, v: f64) -> bool {
            slope.filter(|_| v > 0.0).is_some()
        }
        
        pub struct Prover<'a> {
            pub slope: Option<&'a dyn Fn(&Bounds, f64) -> f64>,
            pub lip: &'a dyn Fn() -> f64,
        }
        
        impl Prover<'_> {
            pub fn radius(&self, c: [f64; 3], v: f64) -> f64 {
                let at = |c: [f64; 3], v: f64| {
                    self.slope.filter(|_| v > 0.0).map_or(f64::INFINITY, |slope| {
                        slope(&(c.map(|x| x - v), c.map(|x| x + v)), v)
                    })
                };
                let local = at(c, v);
                v / if local.is_finite() { local.max(1.0) } else { (self.lip)().max(1.0) }
            }
        }
        
        // Each closure signature contains the nested dyn relation being checked.
        pub fn mut_pred(
            slope: Option<&mut dyn FnMut(&Bounds) -> f64>,
            b: &Bounds,
            v: f64,
        ) -> Option<f64> {
            slope.filter(|_| v > 0.0).map(|slope| slope(b))
        }
        
        pub fn chained(slope: Option<&dyn Fn(&Bounds) -> f64>, b: &Bounds, v: f64) -> bool {
            // Retain the dyn through all three closure signatures.
            slope.map(|cb| cb).filter(|_| v > 0.0).is_some_and(|cb| cb(b) > 0.0)
        }
        
        pub fn outer_lifetime<'a>(
            slope: Option<&'a dyn Fn(&Bounds, &'a f64) -> f64>,
            b: &Bounds,
            v: &'a f64,
        ) -> Option<f64> {
            slope.filter(|_| *v > 0.0).map(|cb| cb(b, v))
        }
        
        pub fn borrowed_output<'b>(
            slope: Option<&dyn Fn(&Bounds) -> &f64>,
            b: &'b Bounds,
        ) -> Option<&'b f64> {
            slope.filter(|_| true).map(|cb| cb(b))
        }
        
        pub fn two_dyns(
            pair: Option<(&dyn Fn(&Bounds) -> f64, &dyn Fn(&Bounds) -> f64)>,
            b: &Bounds,
        ) -> bool {
            pair.filter(|_| true).is_some_and(|(left, right)| left(b) == right(b))
        }
        
        pub trait Tr<'s> {
            fn value(&self, b: &'s Bounds) -> f64;
        }
        
        pub fn user_trait(slope: Option<&dyn for<'s> Tr<'s>>, b: &Bounds) -> Option<f64> {
            slope.filter(|_| true).map(|cb| cb.value(b))
        }
        
        // These definitions are exactly the binder3 helpers from rejected_shape_tests.rs.
        pub struct M { pub a: f32, pub b: f32 }
        fn chords(m: &M) -> (usize, impl Fn(f32) -> f32 + '_) { (1, move |f| m.a * f + m.b) }
        fn hr<T>(x: &T, f: impl for<'r> Fn(&'r T) -> bool) -> bool { f(x) }
        
        pub fn opaque_dyn(m: &M, cb: &dyn Fn(&u8) -> u8) -> bool {
            hr(&(chords(m), cb), |_| true)
        }
        
        fn g(_: &dyn Fn(&u8)) {}
        
        pub fn reify_dyn() -> for<'a> fn(&'a (dyn for<'s> Fn(&'s u8) + 'a)) {
            g
        }
        
        // The source callback accepts one outer-bound lifetime; contravariance permits
        // its caller to provide a dyn that accepts every input lifetime.
        pub fn existential_only_fn(
            f: for<'r> fn(&'r u8, &'r (dyn Fn(&'r u8) + 'r)),
        ) -> for<'r> fn(&'r u8, &'r (dyn for<'s> Fn(&'s u8) + 'r)) {
            f
        }
        
        fn call_higher_ranked(
            f: impl for<'r> Fn(&'r u8, &'r (dyn for<'s> Fn(&'s u8) + 'r)),
            x: &u8,
            cb: &dyn for<'s> Fn(&'s u8),
        ) {
            f(x, cb)
        }
        
        pub fn existential_only_closure(
            f: for<'r> fn(&'r u8, &'r (dyn Fn(&'r u8) + 'r)),
            x: &u8,
            cb: &dyn for<'s> Fn(&'s u8),
        ) {
            call_higher_ranked(|x, cb| f(x, cb), x, cb)
        }
        
        // Also put the narrowed dyn in the higher-ranked closure's output signature.
        fn call_adapter(
            f: impl for<'r> Fn(&'r (dyn for<'s> Fn(&'s u8) + 'r)) -> &'r (dyn Fn(&'r u8) + 'r),
            x: &u8,
            cb: &dyn for<'s> Fn(&'s u8),
        ) {
            f(cb)(x)
        }
        
        pub fn existential_only_output(x: &u8, cb: &dyn for<'s> Fn(&'s u8)) {
            call_adapter(|cb| cb, x, cb)
        }
        
        fn extend<'a, 'b, T>(_: &'a &'b (), x: &'b T, _: &dyn for<'s> Fn(&'s u8)) -> &'a T { x }
        pub fn extend_dyn<'x, T>(x: &'x T, cb: &dyn for<'s> Fn(&'s u8)) -> &'static T {
            let f: for<'a, 'b, 'c> fn(&'a &'b (), &'b T, &'c dyn for<'s> Fn(&'s u8)) -> &'a T = extend;
            let g: for<'r, 'c> fn(&'static &'static (), &'r T, &'c dyn for<'s> Fn(&'s u8)) -> &'static T = f;
            g(&&(), x, cb)
        }

        pub fn tied_placeholder(
            source: for<'e> fn(&'e u8, Box<dyn for<'s> Fn(&'s u8)>),
        ) -> for<'r> fn(&'r u8, Box<dyn Fn(&'r u8)>) {
            source
        }
    "#);
    assert!(!has(&e, "pred", ""), "pred: {e:?}");
    assert!(!has(&e, "radius", ""), "radius: {e:?}");
    assert!(!has(&e, "mut_pred", ""), "mut_pred: {e:?}");
    assert!(!has(&e, "chained", ""), "chained: {e:?}");
    assert!(!has(&e, "outer_lifetime", ""), "outer_lifetime: {e:?}");
    assert!(!has(&e, "borrowed_output", ""), "borrowed_output: {e:?}");
    assert!(!has(&e, "two_dyns", ""), "two_dyns: {e:?}");
    assert!(!has(&e, "user_trait", ""), "user_trait: {e:?}");
    assert!(!has(&e, "opaque_dyn", ""), "opaque_dyn: {e:?}");
    assert!(!has(&e, "reify_dyn", ""), "reify_dyn: {e:?}");
    assert!(!has(&e, "existential_only_fn", ""), "existential_only_fn: {e:?}");
    assert!(!has(&e, "existential_only_closure", ""), "existential_only_closure: {e:?}");
    assert!(!has(&e, "existential_only_output", ""), "existential_only_output: {e:?}");
    assert!(has(&e, "tied_placeholder", ""), "tied_placeholder: {e:?}");
    assert!(has(&e, "extend_dyn", ""), "extend_dyn: {e:?}");
    // Check the helpers too, so errors there cannot make the shape checks vacuous.
    assert!(!has(&e, "chords", ""), "chords: {e:?}");
    assert!(!has(&e, "hr", ""), "hr: {e:?}");
    assert!(!has(&e, "g", ""), "g: {e:?}");
    assert!(!has(&e, "call_higher_ranked", ""), "call_higher_ranked: {e:?}");
    assert!(!has(&e, "call_adapter", ""), "call_adapter: {e:?}");
    assert!(!has(&e, "extend", ""), "extend: {e:?}");
}

/// Signature lifetimes produce actionable warnings without changing acceptance; explicit bounds, bound
/// signature regions, static outputs and inference regions that grow to meet it satisfy the rule.
#[test]
fn a_callable_whose_signature_may_not_outlive_a_region_warns() {
    const CHILD: &str = "RELAX_CALLABLE_OUTLIVES_TEST";
    if std::env::var(CHILD).is_err() {
        let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "rejected_shape_tests::a_callable_whose_signature_may_not_outlive_a_region_warns", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("RELAX_LINT_FLAGS", ""));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && stdout.contains("1 passed"), "callable warning test did not pass: {stdout}\n{}", String::from_utf8_lossy(&out.stderr));
        return;
    }
    let source = r#"
        fn need<F: 'static>(_: F) {}
        fn early<'a: 'a>(x: &'a u8) -> &'a u8 { x }
        pub fn early_fn<'x>() { need(early::<'x>); }
        fn plain() {}
        fn id<'a>(x: &'a u8) -> &'a u8 { x }
        pub fn plain_fn() { need(plain); }
        pub fn bound_fn() { need(id); }
        pub fn bound_closure() { need(|x: &u8| *x); }
        pub fn static_closure() { need(|| -> &'static str { "" }); }
        pub struct Link<'s>(pub &'s str);
        pub struct Doc(u8);
        impl Doc {
            pub fn resolve<'s>(&self) -> impl FnMut(Link<'s>) -> (&'s str, u8) + '_ { let value = &self.0; move |link: Link<'s>| (link.0, *value) }
            pub fn resolve_bound<'a, 's: 'a>(&'a self) -> impl FnMut(Link<'s>) -> (&'s str, u8) + 'a { let value = &self.0; move |link: Link<'s>| (link.0, *value) }
        }
        pub struct Trees(pub [u8; 2], pub [u8; 2]);
        impl Trees {
            pub fn all(&self) -> impl Iterator<Item = u8> + '_ { [&self.0, &self.1].into_iter().flat_map(|t| (0..2usize).map(move |i| t[i])) }
        }
    "#;
    crate::lint::enable();
    let (e, reports) = crate::lint::capture(|| errors(source));
    assert!(e.is_empty(), "callable advisories changed acceptance, including a helper: {e:?}");
    assert!(reports.iter().all(|r| r.level < crate::lint::Level::Deny), "callable cases emitted a lint error: {:?}", reports.iter().map(|r| &r.line).collect::<Vec<_>>());
    let warnings: Vec<_> = reports.iter().filter(|r| r.lint == crate::lint::CALLABLE_OUTLIVES.name).map(|r| &r.line).collect();
    let line = |item: &str| source.lines().position(|l| l.contains(&format!("fn {item}"))).expect("test item") + 1;
    let warning_line = |w: &String| w.split(": warning: ").next().unwrap().rsplit(':').nth(1).and_then(|n| n.parse::<usize>().ok()).expect("warning location");
    for (item, signature) in [("early_fn", "the signature of `"), ("resolve<", "this closure's signature")] {
        let found: Vec<_> = warnings.iter().filter(|w| warning_line(w) == line(item)).collect();
        assert!(!found.is_empty(), "{item}: missing signature lifetime warning: {warnings:?}");
        assert!(found.iter().all(|w| w.contains(signature) && w.contains("note: a callable outlives a lifetime only if every lifetime its signature names does")
            && w.contains("help: state the bound where both lifetimes are declared")), "{item}: warning lost the callable, rule or fix: {found:?}");
    }
    assert!(warnings.iter().any(|w| warning_line(w) == line("early_fn") && w.contains("early`") && w.contains("`'x: 'static`")), "early_fn: warning lost the function path or named lifetime bound: {warnings:?}");
    assert!(warnings.iter().any(|w| warning_line(w) == line("resolve<") && w.contains("names `'s`") && w.contains("fn f<'a, 's: 'a>")), "resolve: warning lost the named signature lifetime or anonymous receiver fix: {warnings:?}");
    for item in ["need<", "early<", "plain()", "id<", "plain_fn", "bound_fn", "bound_closure", "static_closure", "resolve_bound", "all("] {
        assert!(!warnings.iter().any(|w| warning_line(w) == line(item)), "{item}: a satisfied signature lifetime requirement warned: {warnings:?}");
    }
    assert!(warnings.iter().all(|w| [line("early_fn"), line("resolve<")].contains(&warning_line(w))), "unexpected callable warning outside the two unproven requirements: {warnings:?}");
}

/// Comparing an impl item's signature with its trait's assumes the impl header types the trait signature names
/// (every item of the impl assumes its header), and no more. A where-clause or an impl signature that needs a
/// header bound the trait signature does not name is refused, as rustc refuses them (E0276, E0310). Two shapes
/// rustc accepts stay refused: a bound only a non-header alias argument carries (a generic associated type's
/// where-clause, the alias normalizing away in the impl) proves no where-clause of the impl method, and a header
/// that is itself an alias normalizing away (`<&'b &'a () as Trait>::Type`) implies nothing. The declarations
/// and a generic caller are fine.
#[test]
fn an_impl_item_assumes_the_header_types_its_trait_signature_names_and_no_more() {
    let (prog, tcx, user) = load("
        use std::marker::PhantomData;
        pub mod decl {
            pub trait Plain { fn f(x: &u32) -> u32; }
            pub trait Named { type A; fn g(x: &u32) -> u32; }
            pub trait Gat { type W<'a, 'b> where 'b: 'a; fn w<'a, 'b, T>(_: Self::W<'a, 'b>, v: &'b T) -> &'a T where 'a: 'a, 'b: 'b; }
            pub trait Trait { type Type; }
            impl<T> Trait for T { type Type = (); }
            pub trait Ext<'a, 'b> { fn extend(self, s: &'a str) -> &'b str; }
        }
        use decl::*;
        pub struct G<T: 'static>(PhantomData<T>);
        impl<T> Plain for G<T> { fn f(x: &u32) -> u32 where T: 'static { *x } }
        impl<T> Named for G<T> { type A = u32; fn g(x: &<G<T> as Named>::A) -> u32 { *x } }
        pub struct S;
        impl Gat for S { type W<'a, 'b> = () where 'b: 'a; fn w<'a, 'b, T>(_: Self::W<'a, 'b>, v: &'b T) -> &'a T where 'a: 'a, 'b: 'a { v } }
        pub fn call<'a, 'b, X: Gat, T>(w: X::W<'a, 'b>, v: &'b T) -> &'a T { X::w(w, v) }
        impl<'a, 'b> Ext<'a, 'b> for <&'b &'a () as Trait>::Type { fn extend(self, s: &'a str) -> &'b str { s } }");
    // the impl items whose signature comparison with their trait's fails (E0053 in a checked crate)
    let mismatched: Vec<String> = (0..prog.defs.len() as u32).map(DefId)
        .filter(|&d| prog.def(d).krate == user && prog.trait_item_def_id.contains_key(&d) && tcx.compare_impl_item(&prog, d).is_err())
        .map(|d| prog.name(d).to_string()).collect();
    assert!(mismatched.iter().any(|n| n == "f"), "a where-clause only the header implies, the trait signature not naming it: {mismatched:?}");
    assert!(mismatched.iter().any(|n| n == "g"), "an impl signature naming the header, the trait signature not: {mismatched:?}");
    assert!(mismatched.iter().any(|n| n == "w"), "a bound only a non-header alias argument carries: {mismatched:?}");
    assert!(mismatched.iter().any(|n| n == "extend"), "a header that normalizes away implies nothing: {mismatched:?}");
    assert_eq!(mismatched.len(), 4, "only those four: {mismatched:?}");
    let e = errors_of(&prog, &tcx, user);
    for item in ["decl::Plain::f", "decl::Named::g", "decl::Gat::w", "decl::Gat::W", "decl::Ext::extend", "call"] {
        assert!(!has(&e, item, ""), "{item} is fine: {e:?}");
    }
}
