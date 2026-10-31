// Mixed-sign integer ops (core/src/num/int_macros.rs:821 checked_sub_unsigned, :2019
// saturating_sub_unsigned, :2219 wrapping_sub_unsigned, :2753 overflowing_sub_unsigned;
// uint_macros.rs:2490 saturating_sub_signed, :2639 wrapping_sub_signed, :3032
// overflowing_add_signed, :3122 overflowing_sub_signed). glam's i16/i32 vectors use
// saturating_sub_unsigned; libm's hex_float.rs:320 does too.
#[test]
fn signed_with_unsigned() {
    assert_eq!(5i32.saturating_sub_unsigned(7), -2);
    assert_eq!(i32::MIN.saturating_sub_unsigned(1), i32::MIN);
    assert_eq!((-100i8).saturating_sub_unsigned(200), i8::MIN);
    assert_eq!(i16::MAX.saturating_sub_unsigned(u16::MAX), i16::MIN);
    assert_eq!(5i8.checked_sub_unsigned(200), None);
    assert_eq!(0i8.checked_sub_unsigned(128), Some(i8::MIN));
    assert_eq!(0i8.checked_sub_unsigned(129), None);
    assert_eq!(i64::MIN.wrapping_sub_unsigned(1), i64::MAX);
    assert_eq!(1i16.overflowing_sub_unsigned(u16::MAX), (2, true));
    assert_eq!(i16::MIN.overflowing_sub_unsigned(1), (i16::MAX, true));
    assert_eq!((-1isize).overflowing_add_unsigned(1), (0, false));
}

#[test]
fn unsigned_with_signed() {
    assert_eq!(1u8.saturating_sub_signed(2), 0);
    assert_eq!(1u8.saturating_sub_signed(-2), 3);
    assert_eq!((u8::MAX - 2).saturating_sub_signed(-4), u8::MAX);
    assert_eq!(0u32.wrapping_sub_signed(1), u32::MAX);
    assert_eq!(0u32.wrapping_sub_signed(-1), 1);
    assert_eq!(1u64.overflowing_sub_signed(2), (u64::MAX, true));
    assert_eq!(1u64.overflowing_sub_signed(-2), (3, false));
    assert_eq!(1u16.overflowing_add_signed(-2), (u16::MAX, true));
    assert_eq!(1u16.overflowing_add_signed(2), (3, false));
    assert_eq!(u128::MAX.overflowing_add_signed(1), (0, true));
    assert_eq!(2usize.overflowing_add_signed(-2), (0, false));
}
