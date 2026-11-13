// An overloaded operator with a 128-bit operand is its trait call. Lowering sent every operator
// with a u128/i128 operand (or a reference to one) to the two-word built-in path before it looked
// for the operator's impl, so `w += 5u128` on a Wrapping<u128>, `x * k` with `impl Mul<i128> for X`
// and `n + x` with `impl Add<X> for u128` stopped with "internal: built-in `Add` on
// `Wrapping<u128>` and `u128` ... a type check gap".
use std::num::Wrapping;
use std::ops::{Add, AddAssign, Mul};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fixed(pub i128);

impl Mul<i128> for Fixed {
    type Output = Fixed;
    fn mul(self, k: i128) -> Fixed {
        Fixed(self.0.wrapping_mul(k))
    }
}
impl Add<Fixed> for u128 {
    type Output = u128;
    fn add(self, x: Fixed) -> u128 {
        self.wrapping_add(x.0 as u128)
    }
}
impl AddAssign<&u128> for Fixed {
    fn add_assign(&mut self, k: &u128) {
        self.0 = self.0.wrapping_add(*k as i128);
    }
}

#[test]
fn overloaded_operators_take_128_bit_operands() {
    let mut w = Wrapping(u128::MAX - 1);
    w += 5u128;
    assert_eq!(w, Wrapping(3), "Wrapping<u128> += u128");
    w -= &7u128;
    assert_eq!(w.0, u128::MAX - 3, "Wrapping<u128> -= &u128");
    let mut s = Wrapping(i128::MIN);
    s /= -1i128;
    assert_eq!(s, Wrapping(i128::MIN), "Wrapping<i128> /= i128 wraps");
    s %= -1i128;
    assert_eq!(s.0, 0, "Wrapping<i128> %= i128 wraps");
    assert_eq!(Fixed(-3) * (1i128 << 100), Fixed(-3 << 100), "Fixed * i128");
    assert_eq!((1u128 << 127) + Fixed(5), (1 << 127) + 5, "u128 + Fixed");
    let mut f = Fixed(1);
    f += &(u64::MAX as u128 + 1);
    assert_eq!(f, Fixed(1 << 64 | 1), "Fixed += &u128");
    // the built-in operators beside them keep both halves
    let (a, b) = (u128::MAX / 3, (1u128 << 100) + 9);
    assert_eq!(a + b, a.wrapping_add(b));
    assert_eq!(&a * &3, u128::MAX);
}
