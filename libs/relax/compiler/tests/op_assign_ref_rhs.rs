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
}
