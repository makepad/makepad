// NonZero<T> per-type API: MIN / MAX / BITS, bit queries, checked/saturating arithmetic, signed
// helpers, From<NonZero<T>> for T, BitOr (real core's nonzero.rs).
use std::num::NonZero;

#[test]
fn a01_consts() {
    println!("| u64 {} {} {}", NonZero::<u64>::MIN, NonZero::<u64>::MAX, NonZero::<u64>::BITS);
    println!("| i8 {} {} {}", NonZero::<i8>::MIN, NonZero::<i8>::MAX, NonZero::<i8>::BITS);
    println!("| u8 {} usize {} i128 {}", std::num::NonZeroU8::MIN, std::num::NonZeroUsize::MAX, std::num::NonZeroI128::MIN);
    const ZERO_TIME: NonZero<u64> = NonZero::<u64>::MIN;
    println!("| const {}", ZERO_TIME);
}

#[test]
fn a02_methods() {
    let a = NonZero::new(40u32).unwrap();
    let b = NonZero::new(7u32).unwrap();
    println!("| lz {} tz {} ones {}", a.leading_zeros(), a.trailing_zeros(), a.count_ones());
    println!("| mul {:?} {:?} sat {}", a.checked_mul(b), NonZero::<u32>::MAX.checked_mul(b), NonZero::<u32>::MAX.saturating_mul(b));
    println!("| pow {:?} {:?} sat {}", b.checked_pow(3), a.checked_pow(9), a.saturating_pow(9));
    println!("| add {:?} {:?} sat {}", a.checked_add(5), NonZero::<u32>::MAX.checked_add(1), NonZero::<u32>::MAX.saturating_add(9));
    println!("| pow2 {:?} {:?} ilog2 {} ilog10 {} is {} {}", a.checked_next_power_of_two(), NonZero::<u32>::MAX.checked_next_power_of_two(), a.ilog2(), a.ilog10(), a.is_power_of_two(), NonZero::new(64u32).unwrap().is_power_of_two());
    println!("| isqrt {}", NonZero::new(99u64).unwrap().isqrt());
    let n = NonZero::new(-5i32).unwrap();
    println!("| abs {} {:?} {:?} sat {} wrap {} uabs {}", n.abs(), n.checked_abs(), NonZero::<i32>::MIN.checked_abs(), NonZero::<i32>::MIN.saturating_abs(), NonZero::<i32>::MIN.wrapping_abs(), NonZero::<i32>::MIN.unsigned_abs());
    println!("| sign {} {} neg {:?} {:?} {:?} {} {}", n.is_positive(), n.is_negative(), n.checked_neg(), NonZero::<i32>::MIN.checked_neg(), NonZero::<i32>::MIN.overflowing_neg(), NonZero::<i32>::MIN.wrapping_neg(), NonZero::<i32>::MIN.saturating_neg());
    println!("| overflowing_abs {:?}", NonZero::<i8>::MIN.overflowing_abs());
}

#[test]
fn a03_conversions_bitor() {
    let a = NonZero::new(0b1010u16).unwrap();
    let x: u16 = a.into();
    let y = u16::from(a);
    println!("| into {} {}", x, y);
    println!("| bitor {} {} {}", a | NonZero::new(1u16).unwrap(), a | 0u16, 4u16 | a);
    let mut m = a;
    m |= 0x100u16;
    m |= NonZero::new(1u16).unwrap();
    println!("| bitor_assign {}", m);
    let i: i64 = NonZero::new(-3i64).unwrap().into();
    let u: u128 = NonZero::new(9u128).unwrap().into();
    println!("| widths {} {}", i, u);
}
