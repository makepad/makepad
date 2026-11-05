// Type checking and lowering cases larger crates rely on, each as rustc does it: `?` passes the
// expected type back to its operand (`let n: i64 = -s.parse()?`), a scalar left operand compares
// with a user type through `PartialEq<User> for u16`, `&str` comparisons through their traits, a
// struct literal through a projection (`Self::Out { .. }`, `T::Out { .. }`, an alias to one), a
// precise closure capture of a nested field (the uncaptured siblings drop with their owner),
// temporaries extended through `if` and `match` arms, constant promotion (`&(1 + 2)`,
// `&CONST.field`), a constant pattern of several scalars, a single-variant enum's discriminant, and
// a where-clause on a rigid projection that is no inherent candidate (rustc probe.rs: only a type
// parameter's bounds are; the projection's method comes from the traits in scope).
// Run with `makepad test` and rustc --test; lines "| " are compared.
use std::cell::RefCell;

thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
fn log(s: &str) {
    LOG.with(|l| l.borrow_mut().push(s.to_string()));
}
fn take() -> String {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut())).join(",")
}
struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) {
        log(self.0);
    }
}

fn negated(s: &str) -> Result<i64, std::num::ParseIntError> {
    let n: i64 = -s.parse()?;
    Ok(n)
}

#[test]
fn try_passes_the_expected_type_back() {
    println!("| {:?} {:?}", negated("5"), negated("x").is_err());
}

// A binary operator's left operand has no expected type (rustc op.rs check_overloaded_binop): a
// reference out of `?` stays one and `&T: Op<T>` applies, in an operator, a compound assignment and
// a shift alike. A coercion the expected type asks for still happens at the `?`.
fn flipped(v: &[u8]) -> Option<Vec<u8>> {
    Some(vec![(v.first()? ^ 1) & 1])
}

fn weighted(input: &[f32], taps: &[f32]) -> Option<f32> {
    let mut sum = 0.0f32;
    for i in 0..taps.len() {
        sum += input.get(i)? * taps[i];
    }
    Some(sum)
}

fn solved(row: &[f64], x: &[f64]) -> Option<f64> {
    let mut s = 10.0;
    for j in 0..row.len() {
        s -= row.get(j)? * x.get(j)?;
    }
    Some(s)
}

fn shifted(v: &[u32]) -> Option<u32> {
    let k: u32 = v.first()? << 2;
    Some(k)
}

fn sliced(v: &[[u8; 2]]) -> Option<usize> {
    let r: &[u8] = v.first()?;
    Some(r.len())
}

// A `match`, `if` or block left operand gets no expected type either: its value stays a reference.
fn matched(v: &[u8]) -> u8 {
    (match v.first() { Some(x) => x, None => &0 } ^ 1) & 1
}

fn chosen(v: &[u16], c: bool) -> u16 {
    (if c { &v[0] } else { &v[1] } * 2) + 1
}

fn blocked(v: &[u16]) -> u16 {
    ({ &v[0] } * 2) + 1
}

fn shifted_arm(v: &[u32], c: bool) -> u32 {
    let k: u32 = (if c { &v[0] } else { &v[1] }) << 1;
    k
}

#[test]
fn try_operands_of_operators_keep_their_reference() {
    println!("| {:?} {:?} {:?} {:?} {:?}", flipped(&[4]), flipped(&[]), weighted(&[1.0, 2.0], &[0.5, 0.25]), weighted(&[1.0], &[0.5, 0.25]), solved(&[1.0, 2.0], &[3.0, 4.0]));
    println!("| {:?} {:?} {} {} {} {}", shifted(&[3]), sliced(&[[1, 2]]), matched(&[4]), matched(&[]), chosen(&[3, 5], true), chosen(&[3, 5], false));
    println!("| {} {}", shifted_arm(&[3, 5], false), blocked(&[3]));
}

#[derive(Debug, Clone, Copy)]
struct Code(u16);
impl PartialEq<Code> for u16 {
    fn eq(&self, other: &Code) -> bool {
        *self == other.0
    }
}
impl PartialOrd<Code> for u16 {
    fn partial_cmp(&self, other: &Code) -> Option<std::cmp::Ordering> {
        self.partial_cmp(&other.0)
    }
}

#[test]
fn scalar_left_operand_with_a_user_type() {
    let c = Code(7);
    let s: &str = "abc";
    let owned = String::from("abc");
    println!("| {} {} {} {} {} {}", 7u16 == c, 8u16 != c, 6u16 < c, s == owned, owned == s, s < "abd");
}

trait Make {
    type Out;
    fn make() -> Self::Out;
}
struct Point {
    x: i32,
    y: i32,
}
impl Make for () {
    type Out = Point;
    fn make() -> Self::Out {
        Self::Out { x: 1, y: 2 }
    }
}
fn build<T: Make<Out = Point>>() -> Point {
    T::Out { x: 3, y: 4 }
}
type Made = <() as Make>::Out;

#[test]
fn struct_literal_through_a_projection() {
    let (p, q, r) = (<() as Make>::make(), build::<()>(), Made { x: 5, y: 6 });
    println!("| {} {} {} {} {} {}", p.x, p.y, q.x, q.y, r.x, r.y);
}

struct Inner {
    name: D,
    other: D,
}
struct Outer {
    inner: Inner,
    tail: D,
}

#[test]
fn closure_owns_only_the_nested_field_it_captures() {
    let o = Outer { inner: Inner { name: D("name"), other: D("other") }, tail: D("tail") };
    let c = move || {
        let n = o.inner.name;
        log("in closure");
        drop(n);
    };
    log("before call");
    c();
    log("after call");
    drop(o.inner.other);
    log("end");
    drop(o.tail);
    println!("| {}", take());
}

#[test]
fn unused_nested_capture_drops_with_the_closure() {
    let o = Outer { inner: Inner { name: D("name2"), other: D("other2") }, tail: D("tail2") };
    let c = move || {
        let _r = &o.inner.name;
        log("seen");
    };
    c();
    drop(c);
    log("closure gone");
    drop(o.inner.other);
    drop(o.tail);
    println!("| {}", take());
}

fn make(s: &'static str) -> D {
    D(s)
}

#[test]
fn temporaries_extended_through_if_and_match() {
    for flag in [true, false] {
        let r = if flag { &make("then") } else { &make("else") };
        log(r.0);
        let m = match flag {
            true => &make("arm1"),
            false => &make("arm2"),
        };
        log(m.0);
    }
    println!("| {}", take());
}

struct Pair {
    a: [i32; 3],
    b: i32,
}
const PAIR: Pair = Pair { a: [4, 5, 6], b: 9 };
const T: (u8, u16) = (1, 2);

fn promoted() -> &'static i32 {
    &(1 + 2)
}

fn field_of_const() -> &'static [i32; 3] {
    &PAIR.a
}

#[test]
fn constants_promoted_to_static_memory() {
    let t: &'static u16 = &T.1;
    let b: &'static i32 = &PAIR.b;
    println!("| {} {:?} {} {}", promoted(), field_of_const(), t, b);
}

mod hidden {
    pub trait Trait1 {
        fn method(&self) -> u8 {
            1
        }
    }
}
trait Trait2 {
    fn method(&self) -> u8 {
        2
    }
}
trait Other {}
impl<T: Other + ?Sized> Trait2 for T {}
struct S;
impl hidden::Trait1 for S {}
impl Other for S {}

fn through_target<T>(x: T) -> u8
where
    T: std::ops::Deref,
    <T as std::ops::Deref>::Target: hidden::Trait1 + Other,
{
    x.method()
}

#[test]
fn projection_bounds_are_not_inherent() {
    println!("| {} {}", through_target(Box::new(S)), through_target(&S));
}

#[derive(PartialEq, Eq)]
struct Key {
    a: u8,
    b: u32,
}
const K: Key = Key { a: 1, b: 300 };

#[derive(Clone, Copy)]
enum One {
    Only = 5,
}

#[test]
fn constant_patterns_and_single_variant_tags() {
    let probe = |k: Key| match k {
        K => "k",
        _ => "other",
    };
    let e = One::Only;
    println!("| {} {} {} {}", probe(Key { a: 1, b: 300 }), probe(Key { a: 1, b: 301 }), e as i32, std::mem::size_of::<One>());
}
