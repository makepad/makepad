use std::ops::{Neg, Not};
struct S(i32);
impl Neg for S { type Output = i32; fn neg(self) -> i32 { -self.0 } }
impl Not for S { type Output = bool; fn not(self) -> bool { self.0 == 0 } }
fn generic<T: Neg<Output = i32>>(x: T) -> i32 { -x }
#[test] fn unary_types() {
    assert_eq!(-3i32, -3);
    assert_eq!(-&3i32, -3);
    assert_eq!(!&false, true);
    assert_eq!(!0u8, 255);
    assert_eq!(generic(S(3)), -3);
    assert!(!S(0));
}
