// std-core differential test (core_diff.sh): the formatting traits of NonZero<T> (relax issue 100: real
// core's impl_nonzero_fmt!) for every NonZero type, and the integer LowerExp/UpperExp they forward to
// (real core's impl_Exp!): precision, round half to even, width, fill, sign and zero padding.
use std::num::NonZero;

macro_rules! every_trait {
    ($($t:ty: $v:expr),*) => {$(
        let n = NonZero::<$t>::new($v).unwrap();
        println!("| {} {} {:?} {:x?} {:b} {:o} {:x} {:X} {:e} {:E} {:#x} [{:+012.2e}]", stringify!($t), n, n, n, n, n, n, n, n, n, n, n);
    )*};
}

#[test]
fn a01_nonzero_every_trait() {
    every_trait!(u8: 200, u16: 65_000, u32: 4_000_000_000, u64: u64::MAX, u128: u128::MAX, usize: 1_234_500,
        i8: -128, i16: -300, i32: -1_234_567, i64: i64::MIN, i128: i128::MIN, isize: -42);
    let n = std::num::NonZeroU32::new(255).unwrap();
    println!("| [{:>8x}] [{:#010b}] [{:<6o}] [{:^9E}]", n, n, n, n);
}

#[test]
fn a02_int_exp_precision() {
    println!("| {:e} {:E} {:e} {:e} {:e}", 0u8, 1u8, 1_000_000u32, 1_234_500i32, -7i8);
    println!("| {:.0e} {:.0e} {:.0e} {:.1e} {:.1e}", 15u32, 25u32, 35u32, 1_250u32, 1_350u32);
    println!("| {:.1e} {:.2e} {:.3e} {:.0e} {:.1e}", 9_960u32, 99_999u32, 1u8, 9u8, 1_251u16);
    println!("| {:.5e} {:.30e}", 123u64, u64::MAX);
    println!("| {:e} {:E} {:.3e} {:.0e}", u128::MAX, i128::MIN, i128::MAX, usize::MAX);
}

#[test]
fn a03_int_exp_padding() {
    println!("| [{:10e}] [{:<10e}] [{:^10e}] [{:>10E}]", 1_234i32, 1_234i32, -1_234i32, 5u8);
    println!("| [{:010e}] [{:+010e}] [{:010.3e}] [{:+.2e}] [{:*^12.4e}]", -1_234i32, 1_234i32, -5i16, 7u8, 42u64);
    println!("| [{:+e}] [{:+.0e}] [{:08.1e}] [{:+08.3E}]", 0i32, 95u8, -95i64, 12_345_678i64);
}
