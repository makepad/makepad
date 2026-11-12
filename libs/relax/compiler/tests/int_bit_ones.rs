// relax issue 60: the integer bit queries Rust 1.97 made stable (`isolate_highest_one`,
// `isolate_lowest_one`, `highest_one`, `lowest_one`) were missing from the integer types and
// from `NonZero`, so every call to one failed to resolve.
use std::num::NonZero;

#[test]
fn unsigned_integers_isolate_their_highest_and_lowest_one() {
    assert_eq!(0b0110_1000u8.isolate_highest_one(), 0b0100_0000u8);
    assert_eq!(0b0110_1000u8.isolate_lowest_one(), 0b0000_1000u8);
    assert_eq!(0u16.isolate_highest_one(), 0u16);
    assert_eq!(0u16.isolate_lowest_one(), 0u16);
    assert_eq!(0x0003_0500u32.isolate_highest_one(), 0x0002_0000u32);
    assert_eq!(0x0003_0500u32.isolate_lowest_one(), 0x0000_0100u32);
    assert_eq!(u64::MAX.isolate_highest_one(), 1u64 << 63);
    assert_eq!(u64::MAX.isolate_lowest_one(), 1u64);
    assert_eq!((3u128 << 100).isolate_highest_one(), 1u128 << 101);
    assert_eq!((3u128 << 100).isolate_lowest_one(), 1u128 << 100);
    assert_eq!(96usize.isolate_highest_one(), 64usize);
    assert_eq!(96usize.isolate_lowest_one(), 32usize);
}

#[test]
fn signed_integers_isolate_their_highest_and_lowest_one() {
    assert_eq!(44i8.isolate_highest_one(), 32i8);
    assert_eq!(44i8.isolate_lowest_one(), 4i8);
    assert_eq!((-44i32).isolate_highest_one(), i32::MIN);
    assert_eq!((-44i32).isolate_lowest_one(), 4i32);
    assert_eq!(0i64.isolate_highest_one(), 0i64);
    assert_eq!(i128::MAX.isolate_highest_one(), 1i128 << 126);
    assert_eq!((-1isize).isolate_lowest_one(), 1isize);
}

#[test]
fn integers_name_their_highest_and_lowest_one() {
    assert_eq!(0u8.highest_one(), None);
    assert_eq!(0u8.lowest_one(), None);
    assert_eq!(0b0110_1000u8.highest_one(), Some(6));
    assert_eq!(0b0110_1000u8.lowest_one(), Some(3));
    assert_eq!(u128::MAX.highest_one(), Some(127));
    assert_eq!((-44i16).highest_one(), Some(15));
    assert_eq!((-44i16).lowest_one(), Some(2));
    assert_eq!(0isize.lowest_one(), None);
}

#[test]
fn nonzero_integers_have_the_same_bit_queries() {
    let a = NonZero::new(0b0110_1000u8).unwrap();
    assert_eq!(a.isolate_highest_one().get(), 0b0100_0000u8);
    assert_eq!(a.isolate_lowest_one().get(), 0b0000_1000u8);
    assert_eq!(a.highest_one(), 6);
    assert_eq!(a.lowest_one(), 3);
    let b = NonZero::new(-44i32).unwrap();
    assert_eq!(b.isolate_highest_one().get(), i32::MIN);
    assert_eq!(b.isolate_lowest_one().get(), 4i32);
    assert_eq!(b.highest_one(), 31);
    assert_eq!(b.lowest_one(), 2);
    let c = NonZero::new(1u64 << 40).unwrap();
    assert_eq!(c.isolate_highest_one(), c);
    assert_eq!(c.highest_one(), 40);
}
