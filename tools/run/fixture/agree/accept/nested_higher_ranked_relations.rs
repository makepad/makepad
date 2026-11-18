// Higher-ranked relations nested in a higher-ranked closure or fn pointer signature (dyn principals, a user
// trait, next to an opaque, a reify, a dyn binder that opens only variables): all accepted.
pub type Bounds = ([f64; 3], [f64; 3]);

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

#[test]
fn runs() {
    let b: Bounds = ([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]);

    let mut calls = 0;
    let mut mutable = |b: &Bounds| { calls += 1; b.1[0] };
    assert_eq!(mut_pred(Some(&mut mutable), &b, 1.0), Some(4.0));
    assert_eq!(mut_pred(Some(&mut mutable), &b, 0.0), None);
    assert_eq!(mut_pred(None, &b, 1.0), None);
    assert_eq!(calls, 1);

    let width = |b: &Bounds| b.1[0] - b.0[0];
    assert!(chained(Some(&width), &b, 1.0));
    assert!(!chained(Some(&width), &b, 0.0));
    assert!(!chained(None, &b, 1.0));
    assert!(!chained(Some(&|_: &Bounds| -1.0), &b, 1.0));

    let bias = 2.0;
    let biased = |b: &Bounds, v: &f64| b.0[0] + *v;
    assert_eq!(outer_lifetime(Some(&biased), &b, &bias), Some(3.0));
    assert_eq!(outer_lifetime(Some(&biased), &b, &-1.0), None);
    assert_eq!(outer_lifetime(None, &b, &bias), None);

    fn first(b: &Bounds) -> &f64 { &b.0[0] }
    assert!(std::ptr::eq(borrowed_output(Some(&first), &b).unwrap(), &b.0[0]));
    assert_eq!(borrowed_output(None, &b), None);

    assert!(two_dyns(Some((&width, &width)), &b));
    assert!(!two_dyns(Some((&width, &|b: &Bounds| b.0[0])), &b));
    assert!(!two_dyns(None, &b));

    struct Lower;
    impl<'s> Tr<'s> for Lower {
        fn value(&self, b: &'s Bounds) -> f64 { b.0[0] }
    }
    assert_eq!(user_trait(Some(&Lower), &b), Some(1.0));
    assert_eq!(user_trait(None, &b), None);

    let m = M { a: 2.0, b: 1.0 };
    let offset = 3u8;
    let cb = |x: &u8| *x + offset;
    assert!(opaque_dyn(&m, &cb));

    reify_dyn()(&|_: &u8| {});

    fn specific<'r>(x: &'r u8, cb: &'r (dyn Fn(&'r u8) + 'r)) { cb(x) }
    let total = std::cell::Cell::new(0u8);
    let record = |x: &u8| total.set(total.get() + *x);
    existential_only_fn(specific)(&3, &record);
    assert_eq!(total.get(), 3);
    existential_only_closure(specific, &4, &record);
    assert_eq!(total.get(), 7);
    existential_only_output(&5, &record);
    assert_eq!(total.get(), 12);
}

