// TryFrom between every pair of integer types at and around each type's bounds (the generated
// impls once wrote their bounds as unsuffixed literals, i.e. i32: TryFrom<i128> for i64 and
// TryFrom<i64> for u32 answered wrong for in-range values). "| " lines = real std's.
use std::convert::TryFrom;

macro_rules! probe {
    ($h:ident, $from:ty, [$($to:ty),*]) => {{
        let vals: [i128; 13] = [0, 1, -1, 300, -300, i8::MIN as i128, u8::MAX as i128, i16::MIN as i128, u16::MAX as i128,
            i32::MIN as i128, u32::MAX as i128, i64::MIN as i128, u64::MAX as i128];
        for &x in vals.iter() {
            let v = x as $from; // wraps for values outside $from: still a value of $from
            $( $h = $h.wrapping_mul(1_000_003).wrapping_add(match <$to>::try_from(v) { Ok(r) => r as u64 ^ 0x5555, Err(_) => 7 }); )*
        }
    }};
}

#[test]
fn a01_int_try_from() {
    let mut h: u64 = 1;
    probe!(h, i8, [u8, i16, u16, i32, u32, i64, u64, i128, u128, isize, usize]);
    probe!(h, u8, [i8, i16, u16, i32, u32, i64, u64, i128, u128, isize, usize]);
    probe!(h, i16, [i8, u8, u16, i32, u32, i64, u64, i128, u128, isize, usize]);
    probe!(h, u16, [i8, u8, i16, i32, u32, i64, u64, i128, u128, isize, usize]);
    probe!(h, i32, [i8, u8, i16, u16, u32, i64, u64, i128, u128, isize, usize]);
    probe!(h, u32, [i8, u8, i16, u16, i32, i64, u64, i128, u128, isize, usize]);
    probe!(h, i64, [i8, u8, i16, u16, i32, u32, u64, i128, u128, isize, usize]);
    probe!(h, u64, [i8, u8, i16, u16, i32, u32, i64, i128, u128, isize, usize]);
    probe!(h, i128, [i8, u8, i16, u16, i32, u32, i64, u64, u128, isize, usize]);
    probe!(h, u128, [i8, u8, i16, u16, i32, u32, i64, u64, i128, isize, usize]);
    probe!(h, isize, [i8, u8, i16, u16, i32, u32, i64, u64, i128, u128, usize]);
    probe!(h, usize, [i8, u8, i16, u16, i32, u32, i64, u64, i128, u128, isize]);
    println!("| hash {}", h);
    println!("| i128->i64 {:?} {:?} {:?}", i64::try_from(300i128).ok(), i64::try_from(-300i128).ok(), i64::try_from(i128::MAX).ok());
    println!("| i64->u32 {:?} {:?}", u32::try_from(90_000i64).ok(), u32::try_from(5_000_000_000i64).ok());
}
