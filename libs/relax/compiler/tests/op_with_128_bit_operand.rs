// An operator with a u128 or i128 operand is the built-in two-word operation only between
// integers, which may be behind references. Two faults:
// - with a struct on either side, or a unary operator whose result is 128 bits wide, it is a call
//   of the operator trait's method, but was lowered as two-word arithmetic on the struct's words
//   (`W(a) + b` gave `(b.high, a.low + b.low)`; `Saturating<u128> += u128` lost the high half);
// - with a reference for the left operand (`&a + &b`, `&a >> 3`, `&a != &b`) it is the built-in
//   operation, but was lowered as a one-word one.
use std::num::{Saturating, Wrapping};
use std::ops::{Add, AddAssign, Mul, Neg, Not, Shl, Sub};

#[derive(Clone, Copy, PartialEq, Debug)]
struct W(u128);

impl Add<u128> for W {
    type Output = W;
    fn add(self, rhs: u128) -> W {
        W(self.0.wrapping_add(rhs))
    }
}
impl AddAssign<u128> for W {
    fn add_assign(&mut self, rhs: u128) {
        self.0 = self.0.wrapping_add(rhs);
    }
}
impl Add<W> for u128 {
    type Output = W;
    fn add(self, rhs: W) -> W {
        W(self.wrapping_add(rhs.0))
    }
}
impl Sub<i128> for W {
    type Output = i128;
    fn sub(self, rhs: i128) -> i128 {
        (self.0 as i128).wrapping_sub(rhs)
    }
}
impl Shl<u128> for W {
    type Output = W;
    fn shl(self, rhs: u128) -> W {
        W(self.0 << (rhs as u32 % 128))
    }
}
// a small struct whose operators give 128-bit results
#[derive(Clone, Copy)]
struct Small(u8);
impl Neg for Small {
    type Output = i128;
    fn neg(self) -> i128 {
        -(self.0 as i128) << 100
    }
}
impl Not for Small {
    type Output = u128;
    fn not(self) -> u128 {
        !(self.0 as u128)
    }
}
impl Mul<Small> for Small {
    type Output = u128;
    fn mul(self, rhs: Small) -> u128 {
        (self.0 as u128 * rhs.0 as u128) << 90
    }
}
impl PartialEq<u128> for W {
    fn eq(&self, rhs: &u128) -> bool {
        self.0 == *rhs
    }
}
impl PartialOrd<u128> for W {
    fn partial_cmp(&self, rhs: &u128) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(rhs)
    }
}
impl PartialEq<W> for u128 {
    fn eq(&self, rhs: &W) -> bool {
        *self == rhs.0
    }
}

const A: u128 = u128::MAX / 3; // 0x5555...5555
const B: u128 = (1 << 100) + 9;

fn opaque<T>(x: T) -> T {
    std::hint::black_box(x)
}

fn generic_add<T: Add<Output = T>>(a: T, b: T) -> T {
    a + b
}
fn generic_neg<T: Neg<Output = T>>(a: T) -> T {
    -a
}
fn generic_lt<T: PartialOrd>(a: T, b: T) -> bool {
    a < b
}

#[test]
fn struct_with_a_128_bit_right_operand() {
    let (a, b) = (opaque(A), opaque(B));
    let want = W(a.wrapping_add(b));
    assert_eq!(W(a) + b, want);
    let mut x = W(a);
    x += b;
    assert_eq!(x, want);
    let mut x = W(a);
    x.add_assign(b);
    assert_eq!(x, want);
    assert_eq!(W(a) - opaque(-7i128 << 70), (a as i128).wrapping_sub(-7i128 << 70));
    assert_eq!(W(a) << opaque(68u128), W(a << 68));
}

#[test]
fn struct_as_the_right_operand_of_a_128_bit_value() {
    let (a, b) = (opaque(A), opaque(B));
    assert_eq!(a + W(b), W(a.wrapping_add(b)));
    assert!(a == W(a));
    assert!(!(a == W(b)));
    assert!(a != W(b));
}

#[test]
fn comparisons_of_a_struct_with_a_128_bit_value() {
    let (a, b) = (opaque(A), opaque(B));
    assert!(W(a) == a);
    assert!(W(a) != b);
    // the two differ only above bit 64
    assert!(W(1 << 64) != opaque(2u128 << 64));
    assert!(W(1 << 64) < opaque(2u128 << 64));
    assert!(W(a) > b);
    assert!(W(b) <= b);
}

#[test]
fn operators_of_a_small_type_with_128_bit_results() {
    let s = opaque(Small(3));
    assert_eq!(-s, -3i128 << 100);
    assert_eq!(!s, u128::MAX - 3);
    assert_eq!(s * Small(5), 15u128 << 90);
}

fn ab() -> (u128, u128, u128) {
    let (a, b) = (opaque(A), opaque(B));
    (a, b, a.wrapping_add(b))
}

#[test]
fn reference_as_the_right_operand() {
    let (a, b, sum) = ab();
    assert_eq!(a + &b, sum);
    let r: &u128 = &b;
    assert_eq!(a + r, sum);
    assert_eq!(a + **&&b, sum);
    assert_eq!(Add::add(a, &b), sum);
    assert_eq!(a << &opaque(70u32), a << 70);
    assert_eq!(a & &b, a & b);
}

#[test]
fn reference_as_the_left_operand() {
    let (a, b, sum) = ab();
    assert_eq!(&a + b, sum);
    assert_eq!(&a >> opaque(70u32), a >> 70);
    assert_eq!(&a | b, a | b);
}

#[test]
fn references_on_both_sides() {
    let (a, b, sum) = ab();
    assert_eq!(&a + &b, sum);
    assert_eq!(&opaque(1u128 << 70) * &opaque(3u128), 3u128 << 70);
    assert_eq!(&a - &a, 0);
    assert_eq!(&a ^ &b, a ^ b);
}

#[test]
fn compound_assignment_from_a_reference() {
    let (a, b, sum) = ab();
    let mut x = a;
    x += &b;
    assert_eq!(x, sum);
    let mut x = a;
    x -= &b;
    assert_eq!(x, a.wrapping_sub(b));
    let mut x = a;
    x <<= &opaque(65u32);
    assert_eq!(x, a << 65);
    let mut x = a;
    x ^= &b;
    assert_eq!(x, a ^ b);
}

#[test]
fn unary_operators_on_a_reference() {
    let (a, _, _) = ab();
    let i = opaque(-5i128 << 80);
    assert_eq!(-&i, 5i128 << 80);
    assert_eq!(!&a, !a);
}

#[test]
fn comparisons_of_references() {
    let (a, b, _) = ab();
    assert!(&a != &b);
    assert!(&b < &a);
    // the two differ only above bit 64
    assert!(&opaque(1u128 << 64) != &opaque(2u128 << 64));
    assert!(&opaque(1u128 << 64) < &opaque(2u128 << 64));
    let (x, y) = (opaque(-1i128), opaque(i128::MIN));
    assert!(&y < &x);
    assert!(&x >= &y);
    assert!(&x == &opaque(-1i128));
}

#[test]
fn references_to_one_word_integers_still_work() {
    let (c, d) = (opaque(7u64), opaque(5u64));
    assert_eq!(c + &d, 12);
    assert_eq!(&c + &d, 12);
    assert_eq!(&c + d, 12);
    assert!(&c > &d);
}

#[test]
fn generic_operators_at_128_bits() {
    let (a, b) = (opaque(A), opaque(B));
    assert_eq!(generic_add(a, b), a + b);
    assert_eq!(generic_neg(opaque(3i128 << 90)), -3i128 << 90);
    assert!(generic_lt(opaque(1u128 << 64), opaque(2u128 << 64)));
    assert!(!generic_lt(opaque(-1i128), opaque(i128::MIN)));
}

#[test]
fn saturating_and_wrapping_128_bit_values() {
    let (a, b) = (opaque(A), opaque(B));
    let mut s = Saturating(a);
    s += b;
    assert_eq!(s.0, a + b);
    let mut s = Saturating(u128::MAX - 1);
    s += b;
    assert_eq!(s.0, u128::MAX);
    assert_eq!((Wrapping(u128::MAX) + Wrapping(b)).0, b - 1);
    let mut w = Wrapping(a);
    w += Wrapping(b);
    assert_eq!(w.0, a + b);
}
