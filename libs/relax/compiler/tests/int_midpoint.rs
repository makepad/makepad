// `midpoint` on every integer width (library/core/src/num/mod.rs:129-186): the sum is taken
// without overflow, signed results round toward zero. egui emath smart_aim.rs uses u8::midpoint.
#[test]
fn midpoints() {
    assert_eq!(0u8.midpoint(4), 2);
    assert_eq!(1u8.midpoint(4), 2);
    assert_eq!(255u8.midpoint(255), 255);
    assert_eq!(u8::MAX.midpoint(1), 128);
    assert_eq!(u64::MAX.midpoint(u64::MAX - 2), u64::MAX - 1);
    assert_eq!(u128::MAX.midpoint(u128::MAX - 2), u128::MAX - 1);
    assert_eq!(usize::MAX.midpoint(1), usize::MAX / 2 + 1);
    assert_eq!((-1i8).midpoint(2), 0);
    assert_eq!((-7i8).midpoint(0), -3);
    assert_eq!(0i16.midpoint(-7), -3);
    assert_eq!(i32::MAX.midpoint(i32::MAX), i32::MAX);
    assert_eq!(i64::MIN.midpoint(i64::MAX), 0);
    assert_eq!(i128::MIN.midpoint(i128::MAX), 0);
    assert_eq!((-7i128).midpoint(0), -3);
    assert_eq!(i128::MIN.midpoint(i128::MIN + 1), i128::MIN + 1);
    assert_eq!(isize::MIN.midpoint(isize::MAX - 1), -1 + 0);
}
