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
    let (mut prog, tcx, user) = load_source(source, 2021, "repro");
    prog.crates[user as usize].validate = true;
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
