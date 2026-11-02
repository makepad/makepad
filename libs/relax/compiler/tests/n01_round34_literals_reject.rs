// overflowing_literals (deny by default): consts, a negated literal, a const argument and a
// `#[deny]` statement inside an `#[allow]` fn. rustc rejects all six literals.
pub const A: u8 = 256;
pub const B: i8 = -129;
pub const C: i8 = 0xff;
pub const D: f32 = 1e100;
struct U<const N: u8>;
pub fn const_arg() { let _ = U::<300>; }
#[allow(overflowing_literals)]
pub fn denied_again() { #[deny(overflowing_literals)] let _: u8 = 256; }

#[test]
fn overflowing_literals_regression() {
    let _ = (A, B, C, D);
    const_arg();
    denied_again();
}
