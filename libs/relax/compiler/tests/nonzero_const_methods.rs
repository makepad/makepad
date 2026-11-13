// relax issue 136: NonZero's arithmetic methods are const fns, as in real core, so they can be
// called in a const initializer (`NonZeroU32::MIN.saturating_add(2)` was E0015). Every
// generated method that real core makes const is called here in a const, for an unsigned and a
// signed type, and the values are checked at run time.
use std::num::{NonZeroI32, NonZeroI8, NonZeroU32, NonZeroU64, NonZeroU8};

const fn u32nz(v: u32) -> NonZeroU32 {
    match NonZeroU32::new(v) {
        Some(n) => n,
        None => panic!("zero"),
    }
}

const fn i32nz(v: i32) -> NonZeroI32 {
    match NonZeroI32::new(v) {
        Some(n) => n,
        None => panic!("zero"),
    }
}

const NEXT: NonZeroU32 = NonZeroU32::MIN.saturating_add(2);
const SATURATED: NonZeroU8 = NonZeroU8::MAX.saturating_add(1);
const CHECKED: Option<NonZeroU64> = NonZeroU64::MIN.checked_add(41);
const CHECKED_OVER: Option<NonZeroU64> = NonZeroU64::MAX.checked_add(1);
const POW2: Option<NonZeroU32> = u32nz(5).checked_next_power_of_two();
const LOG10: u32 = u32nz(1000).ilog10();
const SQRT: NonZeroU32 = u32nz(99).isqrt();
const MID: NonZeroU32 = u32nz(200).midpoint(u32nz(101));

const ABS: NonZeroI32 = i32nz(-7).abs();
const CHECKED_ABS: Option<NonZeroI32> = NonZeroI32::MIN.checked_abs();
const OVERFLOWING_ABS: (NonZeroI8, bool) = NonZeroI8::MIN.overflowing_abs();
const SATURATING_ABS: NonZeroI8 = NonZeroI8::MIN.saturating_abs();
const WRAPPING_ABS: NonZeroI8 = NonZeroI8::MIN.wrapping_abs();
const UNSIGNED_ABS: NonZeroU8 = NonZeroI8::MIN.unsigned_abs();
const CHECKED_NEG: Option<NonZeroI32> = i32nz(5).checked_neg();
const OVERFLOWING_NEG: (NonZeroI8, bool) = NonZeroI8::MIN.overflowing_neg();
const WRAPPING_NEG: NonZeroI8 = NonZeroI8::MIN.wrapping_neg();
const SATURATING_NEG: NonZeroI8 = NonZeroI8::MIN.saturating_neg();

#[test]
fn nonzero_methods_in_const_initializers() {
    assert_eq!(NEXT.get(), 3);
    assert_eq!(SATURATED.get(), 255);
    assert_eq!(CHECKED.map(NonZeroU64::get), Some(42));
    assert_eq!(CHECKED_OVER, None);
    assert_eq!(POW2.map(NonZeroU32::get), Some(8));
    assert_eq!(LOG10, 3);
    assert_eq!(SQRT.get(), 9);
    assert_eq!(MID.get(), 150);
    assert_eq!(ABS.get(), 7);
    assert_eq!(CHECKED_ABS, None);
    assert_eq!((OVERFLOWING_ABS.0.get(), OVERFLOWING_ABS.1), (i8::MIN, true));
    assert_eq!(SATURATING_ABS.get(), i8::MAX);
    assert_eq!(WRAPPING_ABS.get(), i8::MIN);
    assert_eq!(UNSIGNED_ABS.get(), 128);
    assert_eq!(CHECKED_NEG.map(NonZeroI32::get), Some(-5));
    assert_eq!((OVERFLOWING_NEG.0.get(), OVERFLOWING_NEG.1), (i8::MIN, true));
    assert_eq!(WRAPPING_NEG.get(), i8::MIN);
    assert_eq!(SATURATING_NEG.get(), i8::MAX);
}
