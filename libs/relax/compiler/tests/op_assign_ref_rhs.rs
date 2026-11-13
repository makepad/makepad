// relax issue 65: a compound assignment to a primitive with a reference to a non-primitive on the
// right (`x %= &big` with `impl RemAssign<&Big> for u32`, num-bigint's scalar moduli) was checked
// as a primitive one: E0308, expected `u32`, found `&Big`. Every `<Op>Assign<&T>` and the plain
// operators with a `&T` right operand go through their impls.
use std::ops::*;

pub struct Big(pub u32);

macro_rules! by_ref {
    ($($tr:ident $m:ident $atr:ident $am:ident $op:tt;)*) => {$(
        impl $tr<&Big> for u32 {
            type Output = u32;
            fn $m(self, rhs: &Big) -> u32 { self $op rhs.0 }
        }
        impl $atr<&Big> for u32 {
            fn $am(&mut self, rhs: &Big) { *self = *self $op rhs.0; }
        }
    )*};
}

by_ref! {
    Add add AddAssign add_assign +;
    Sub sub SubAssign sub_assign -;
    Mul mul MulAssign mul_assign *;
    Div div DivAssign div_assign /;
    Rem rem RemAssign rem_assign %;
    BitAnd bitand BitAndAssign bitand_assign &;
    BitOr bitor BitOrAssign bitor_assign |;
    BitXor bitxor BitXorAssign bitxor_assign ^;
    Shl shl ShlAssign shl_assign <<;
    Shr shr ShrAssign shr_assign >>;
}

/// num-bigint's shape: the operator's body assigns through the reference it was given.
pub struct Modulus(pub u32);

impl RemAssign<&Modulus> for u32 {
    fn rem_assign(&mut self, m: &Modulus) { *self %= m.0; }
}

impl Rem<&Modulus> for u32 {
    type Output = u32;
    fn rem(mut self, other: &Modulus) -> u32 {
        self %= other;
        self
    }
}

fn scaled<'a, T>(mut x: u32, by: &'a T) -> u32
where
    u32: MulAssign<&'a T>,
{
    x *= by;
    x
}

#[test]
fn every_op_assign_takes_a_reference_on_the_right() {
    let b = Big(3);
    let mut x: u32 = 20;
    x += &b;
    assert_eq!(x, 23);
    x -= &b;
    assert_eq!(x, 20);
    x *= &b;
    assert_eq!(x, 60);
    x /= &b;
    assert_eq!(x, 20);
    x %= &b;
    assert_eq!(x, 2);
    x |= &b;
    assert_eq!(x, 3);
    x &= &Big(1);
    assert_eq!(x, 1);
    x ^= &b;
    assert_eq!(x, 2);
    x <<= &b;
    assert_eq!(x, 16);
    x >>= &Big(2);
    assert_eq!(x, 4);
    assert_eq!(17u32 % &Modulus(5), 2);
    assert_eq!(scaled(5, &Big(3)), 15);
    // core's own `AddAssign<&u32> for u32` is unchanged
    let mut y = 7u32;
    y += &3u32;
    assert_eq!(y, 10);
}

#[test]
fn operators_take_a_reference_on_the_right() {
    let b = Big(4);
    assert_eq!(10u32 + &b, 14);
    assert_eq!(10u32 % &b, 2);
    assert_eq!(1u32 << &b, 16);
    assert_eq!(256u32 >> &b, 16);
    // a shift by an odd amount: a shift built in by the reference's (aligned) address never gives it
    let odd = Big(9);
    assert_eq!(1u32 << &odd, 512);
    assert_eq!((1u32 << 20) >> &odd, 2048);
    assert_eq!(shifted(1 << 20, &odd), 2048);
}

/// The reference is a parameter, not a borrow at the operator.
fn shifted(x: u32, by: &Big) -> u32 {
    x >> by
}

/// rustc 1.99's core forward-ref operator impls: a primitive with a reference to a primitive on
/// either side, every arithmetic and bit operator, and through a closure parameter (`|t| x - t`,
/// t: &f64, from `Option<&f64>::is_some_and`).
#[test]
fn primitives_take_a_reference_to_a_primitive() {
    let (a, b) = (7.5f64, 2.0f64);
    assert_eq!([a + &b, a - &b, a * &b, a / &b, a % &b], [9.5, 5.5, 15.0, 3.75, 1.5]);
    assert_eq!([&a + b, &a - b, &a * &b, &a / &b, &a % b], [9.5, 5.5, 15.0, 3.75, 1.5]);
    let (c, d) = (7.5f32, 2.0f32);
    assert_eq!([c - &d, &c * d, &c / &d], [5.5, 15.0, 3.75]);
    let (i, j) = (-17i32, 5i32);
    assert_eq!([i + &j, i - &j, &i * j, &i / &j, i % &j], [-12, -22, -85, -3, -2]);
    let (u, v) = (200u8, 7u8);
    assert_eq!([u + &v, u - &v, &u / v, &u % &v], [207, 193, 28, 4]);
    // 128-bit: both words of the result, with both operands references too
    let (w, x) = (-(1i128 << 100), 7i128);
    assert_eq!([w + &x, &w - x, &w + &x, &w - &x], [-(1i128 << 100) + 7, -(1i128 << 100) - 7, -(1i128 << 100) + 7, -(1i128 << 100) - 7]);
    assert_eq!([&w * &x, &w / &x, &w % &x], [-(1i128 << 100) * 7, -(1i128 << 100) / 7, -(1i128 << 100) % 7]);
    let (p, q) = (1u128 << 70, 3u128);
    assert_eq!([&p - &q, &p >> &q, &p & &q, p << &q], [(1u128 << 70) - 3, 1u128 << 67, 0, 1u128 << 73]);
    assert!(&w < &x && &x > &w && &w <= &w && &p > &q && &w != &x);
    let (m, n) = (0b1100u32, 0b1010u32);
    assert_eq!([m & &n, &m | n, &m ^ &n, m << &2u32, &m >> &2u8], [8, 14, 6, 48, 3]);
    assert_eq!([true & &false, &true | false, &true ^ &true], [false, true, false]);
    let mut f = 10.0f64;
    f -= &2.5;
    f *= &3.0;
    let mut k = 10u16;
    k -= &3;
    k <<= &2u32;
    assert_eq!((f, k), (22.5, 28));
    let last: std::collections::HashMap<u32, f64> = [(1, 1.0)].into_iter().collect();
    let t0 = 1.0f64;
    assert!(last.get(&1).is_some_and(|t| t0 - t <= 1e-7));
    let scaled: Vec<f64> = [1.0, 2.0].iter().map(|t| 0.5 + t).collect();
    assert_eq!(scaled, [1.5, 2.5]);
}

/// The map's value type comes from the `insert` after the closure, so at `hit - t` the closure's
/// `t` is a reference to a type not known yet: rustc takes `f64: Sub<&?V>`, which only core's
/// `Sub<&f64> for f64` matches (a check once rejected it with E0308).
fn deduped(hits: &[f64]) -> usize {
    let mut last = std::collections::BTreeMap::new();
    let mut kept = 0;
    for &hit in hits {
        if last.get(&0u8).is_some_and(|t| hit - t <= 1e-7) {
            continue;
        }
        last.insert(0u8, hit);
        kept += 1;
    }
    kept
}

/// The same with integers and a compound assignment through the reference.
fn spread(xs: &[i64]) -> i64 {
    let mut first = std::collections::HashMap::new();
    let mut total = 0;
    for &x in xs {
        if let Some(f) = first.get(&1u8) {
            let mut d = x;
            d -= f;
            total += d;
        }
        first.entry(1u8).or_insert(x);
    }
    total
}

#[test]
fn a_reference_to_a_value_inferred_later() {
    assert_eq!(deduped(&[1.0, 1.0, 2.0]), 2);
    assert_eq!(spread(&[5, 7, 10]), 7);
}
