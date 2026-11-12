// The niche of a 128-bit NonZero is the whole value: `Option<NonZero<u128>>` is None only when all
// 16 bytes are zero, not when the low word is (and it stays 16 bytes, as rustc lays it out).
use core::hint::black_box;
use core::num::NonZero;

#[derive(Debug, PartialEq, Clone, Copy)]
enum Three { A(NonZero<u128>), B, C }

#[test]
fn a_zero_low_word_is_still_some() {
    let hi = black_box(1u128 << 64);
    assert!(NonZero::<u128>::new(hi).is_some());
    assert!(NonZero::<i128>::new(black_box(i128::MIN)).is_some());
    assert!(NonZero::<u128>::new(black_box(0)).is_none());
    assert!(NonZero::<i128>::new(black_box(0)).is_none());
    assert!(NonZero::<u128>::new(black_box(1)).is_some());
    let nz = NonZero::<u128>::new(hi).unwrap();
    assert_eq!(nz.get(), hi);
    let o: Option<NonZero<u128>> = black_box(Some(nz));
    assert_eq!(o, Some(nz));
    assert_eq!(match o { Some(v) => v.get() >> 64, None => 0 }, 1);
    let none: Option<NonZero<u128>> = black_box(None);
    assert!(none.is_none());
    assert_ne!(o, none);
}

#[test]
fn the_layout_is_rustcs() {
    assert_eq!(core::mem::size_of::<Option<NonZero<u128>>>(), 16);
    assert_eq!(core::mem::size_of::<Option<NonZero<i128>>>(), 16);
    let raw: u128 = unsafe { core::mem::transmute(Some(NonZero::<u128>::new(black_box(3u128 << 64)).unwrap())) };
    assert_eq!(raw, 3u128 << 64);
    let back: Option<NonZero<u128>> = unsafe { core::mem::transmute(black_box(0u128)) };
    assert!(back.is_none());
}

#[test]
fn values_in_memory_and_in_collections() {
    let v: Vec<Option<NonZero<u128>>> = [0u128, 1, 1 << 64, u128::MAX, 5 << 100].iter().map(|&x| NonZero::new(black_box(x))).collect();
    assert_eq!(v.iter().filter(|x| x.is_some()).count(), 4);
    assert_eq!(v.iter().flatten().map(|x| x.get().count_ones()).sum::<u32>(), 1 + 1 + 128 + 2);
    assert!(v[0].is_none() && v[2].is_some());
    let boxed: Box<Option<NonZero<u128>>> = Box::new(v[2]);
    assert_eq!(boxed.map(|x| x.get()), Some(1 << 64));
}

#[test]
fn an_enum_with_more_variants_than_the_niche_holds_is_tagged() {
    assert_eq!(core::mem::size_of::<Three>(), 32);
    let a = black_box(Three::A(NonZero::new(1u128 << 64).unwrap()));
    assert_eq!(a, Three::A(NonZero::new(1u128 << 64).unwrap()));
    assert!(matches!(a, Three::A(v) if v.get() == 1 << 64));
    assert!(matches!(black_box(Three::B), Three::B));
    assert!(matches!(black_box(Three::C), Three::C));
    assert_ne!(black_box(Three::B), black_box(Three::C));
}

const HIGH: Option<NonZero<u128>> = NonZero::new(1 << 64);
const ZERO: Option<NonZero<u128>> = NonZero::new(0);
static KEPT: Option<NonZero<i128>> = NonZero::new(i128::MIN);
const IS_SOME: bool = HIGH.is_some() && ZERO.is_none() && matches!(KEPT, Some(v) if v.get() == i128::MIN);

#[test]
fn constants_and_statics() {
    assert!(IS_SOME);
    assert_eq!(HIGH.map(|v| v.get()), Some(1 << 64));
    assert_eq!(black_box(ZERO), None);
    assert_eq!(black_box(&KEPT).map(|v| v.get()), Some(i128::MIN));
    let table = [HIGH, ZERO, NonZero::new(7)];
    assert_eq!(black_box(table).iter().flatten().count(), 2);
}
