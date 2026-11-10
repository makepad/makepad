// Shl / Shr for the integer types take the shift amount and the value by reference, as every
// other operator does (core's forward_ref_binop! / forward_ref_op_assign! over shl_impl_all!).
// fearless_simd 0.4's generated fallback level calls the trait method by path on a borrowed lane,
// `i8::shl(a[0usize], &b[0usize])` (src/generated/fallback.rs, 112 calls per lane type), and the
// std here had only the by-value impls: "the trait bound `i8: Shl<&i8>` is not satisfied".
use std::ops::{Shl, ShlAssign, Shr, ShrAssign};

// fearless_simd's shape: the trait method by path, the amount borrowed from another array
fn shlv(a: [i8; 4], b: [i8; 4]) -> [i8; 4] {
    [i8::shl(a[0usize], &b[0usize]), i8::shl(a[1usize], &b[1usize]), i8::shl(a[2usize], &b[2usize]), i8::shl(a[3usize], &b[3usize])]
}
fn shrv(a: [u16; 2], b: [u16; 2]) -> [u16; 2] {
    [u16::shr(a[0usize], &b[0usize]), u16::shr(a[1usize], &b[1usize])]
}

fn generic<T: Copy, U>(x: T, by: U) -> (T, T)
where
    T: Shl<U, Output = T> + for<'a> Shr<&'a U, Output = T>,
    for<'a> &'a T: Shl<&'a U, Output = T>,
{
    let l = &x << &by;
    (l, l >> &by)
}

#[test]
fn trait_method_by_path_with_a_borrowed_amount() {
    assert_eq!(shlv([1, 2, 3, -1], [0, 1, 2, 3]), [1, 4, 12, -8]);
    assert_eq!(shrv([0x8000, 0x00f0], [15, 4]), [1, 0x000f]);
    assert_eq!(u64::shl(1, &40u8), 1 << 40);
    assert_eq!(i128::shr(-(1i128 << 100), &98u32), -4);
}

#[test]
fn every_reference_form() {
    let (x, s) = (0b1011u32, 3u8);
    assert_eq!(x << &s, 0b1011000);
    assert_eq!(&x << s, 0b1011000);
    assert_eq!(&x << &s, 0b1011000);
    assert_eq!(x >> &s, 1);
    assert_eq!(&x >> s, 1);
    assert_eq!(&x >> &s, 1);
    let (y, t) = (-64i16, 2i64);
    assert_eq!(y >> &t, -16);
    assert_eq!(&y << &t, -256);
    let (z, u) = (1usize, 5isize);
    assert_eq!(<&usize as Shl<&isize>>::shl(&z, &u), 32);
    assert_eq!(<u128 as Shr<&i8>>::shr(u128::MAX, &120i8), 0xff);
}

#[test]
fn assign_by_reference() {
    let mut a = 3u8;
    a <<= &2u32;
    assert_eq!(a, 12);
    a >>= &1i8;
    assert_eq!(a, 6);
    let mut b = -1i64;
    ShlAssign::shl_assign(&mut b, &62u16);
    assert_eq!(b, -(1i64 << 62));
    ShrAssign::shr_assign(&mut b, &60usize);
    assert_eq!(b, -4);
}

#[test]
fn through_generic_bounds() {
    assert_eq!(generic(5u32, 4u8), (80, 5));
    assert_eq!(generic(-3i64, 10i32), (-3072, -3));
}
