// Compare stdout with rustc --test -O, also with MAKEPAD_RUN_O3=1 MAKEPAD_RUN_O3_CALLS=100.
use std::hint::black_box;

#[derive(Clone, Copy)]
struct Entry {
    a: u64,
    b: u32,
    c: u16,
}

const fn numbers() -> [u64; 704] {
    let mut a = [0; 704];
    let mut i = 0;
    while i < a.len() {
        a[i] = (i as u64).wrapping_mul(0x123456789abcdef).wrapping_add(17);
        i += 1;
    }
    a
}
const NUMBERS: [u64; 704] = numbers();
const fn entries() -> [Entry; 704] {
    let mut a = [Entry { a: 0, b: 0, c: 0 }; 704];
    let mut i = 0;
    while i < a.len() {
        a[i] = Entry {
            a: NUMBERS[i],
            b: (i * 301) as u32,
            c: (i * 13) as u16,
        };
        i += 1;
    }
    a
}
const ENTRIES: [Entry; 704] = entries();
const fn nested() -> [[u64; 11]; 64] {
    let mut a = [[0; 11]; 64];
    let mut i = 0;
    while i < 64 {
        let mut j = 0;
        while j < 11 {
            a[i][j] = NUMBERS[i * 11 + j];
            j += 1;
        }
        i += 1;
    }
    a
}
const NESTED: [[u64; 11]; 64] = nested();

#[inline(never)]
fn lookup(i: usize) -> u64 {
    let e = ENTRIES[i];
    NUMBERS[i] ^ e.a.rotate_left(7) ^ e.b as u64 ^ e.c as u64 ^ NESTED[i / 11][i % 11]
}
#[inline(never)]
fn by_value(mut a: [u64; 704], i: usize) -> u64 {
    a[i] ^= 123;
    a[i]
}
#[inline(never)]
fn by_ref(a: &[u64; 704], i: usize) -> u64 {
    a[i]
}
#[inline(never)]
fn changed(i: usize) -> u64 {
    let mut a = NUMBERS;
    a[i] ^= 99;
    // The mutable local must keep its own storage, independent of later const uses.
    a[i] ^ NUMBERS[i]
}
#[test]
fn const_indices() {
    let mut h = 0u64;
    for n in 0..4096 {
        let i = black_box((n * 137) % 704);
        h = h.rotate_left(3) ^ lookup(i);
        assert_eq!(by_value(NUMBERS, i), NUMBERS[i] ^ 123);
        assert_eq!(by_ref(&NUMBERS, i), NUMBERS[i]);
        assert_eq!(changed(i), 99);
        h = h.wrapping_add(lookup(i).rotate_left(17));
    }
    println!("const indices {:016x}", h);
}

fn mix(h: u64, x: u128) -> u64 {
    h.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(9) ^ x as u64 ^ ((x >> 64) as u64).rotate_left(23)
}

macro_rules! unsigned {
    ($name:ident, $pair:ident, $ty:ty) => {
        // Called often enough to exercise the optimised body with MAKEPAD_RUN_O3_CALLS=100.
        #[inline(never)]
        fn $pair(mut h: u64, a: $ty, b: $ty, repeat: usize) -> u64 {
            let (r, o) = a.overflowing_mul(b);
            assert_eq!(a.wrapping_mul(b), r);
            assert_eq!(a.checked_mul(b), if o { None } else { Some(r) });
            let sat = a.saturating_mul(b);
            assert_eq!(sat, if o { <$ty>::MAX } else { r });
            h = mix(h, r as u128);
            h = mix(h, sat as u128);
            h = mix(h, o as u128 + repeat as u128);
            h
        }
        #[test]
        fn $name() {
            let bits = <$ty>::BITS;
            let values: [$ty; 12] = [
                0,
                1,
                2,
                3,
                <$ty>::MAX,
                <$ty>::MAX - 1,
                1 << (bits - 1),
                (1 << (bits - 1)) - 1,
                1 << (bits / 2),
                (1 << (bits / 2)) - 1,
                (1 << (bits / 2)) + 1,
                17,
            ];
            let mut h = 0u64;
            for repeat in 0usize..8 {
                for &a in &values {
                    for &b in &values {
                        let a = black_box(a);
                        let b = black_box(b);
                        h = $pair(h, a, b, repeat);
                    }
                }
            }
            println!("{} {:016x}", stringify!($ty), h);
        }
    };
}
macro_rules! signed {
    ($name:ident, $pair:ident, $ty:ty) => {
        // Called often enough to exercise the optimised body with MAKEPAD_RUN_O3_CALLS=100.
        #[inline(never)]
        fn $pair(mut h: u64, a: $ty, b: $ty, repeat: usize) -> u64 {
            let (r, o) = a.overflowing_mul(b);
            assert_eq!(a.wrapping_mul(b), r);
            assert_eq!(a.checked_mul(b), if o { None } else { Some(r) });
            let sat = a.saturating_mul(b);
            let limit = if (a < 0) != (b < 0) { <$ty>::MIN } else { <$ty>::MAX };
            assert_eq!(sat, if o { limit } else { r });
            h = mix(h, r as u128);
            h = mix(h, sat as u128);
            h = mix(h, o as u128 + repeat as u128);
            h
        }
        #[test]
        fn $name() {
            let bits = <$ty>::BITS;
            let half: $ty = 1 << (bits / 2);
            let values: [$ty; 16] = [0, 1, -1, 2, -2, 3, <$ty>::MAX, <$ty>::MIN, <$ty>::MAX - 1, <$ty>::MIN + 1, half, half - 1, half + 1, -half, -(half - 1), -(half + 1)];
            let mut h = 0u64;
            for repeat in 0usize..8 {
                for &a in &values {
                    for &b in &values {
                        let a = black_box(a);
                        let b = black_box(b);
                        h = $pair(h, a, b, repeat);
                    }
                }
            }
            println!("{} {:016x}", stringify!($ty), h);
        }
    };
}
unsigned!(mul_u8, pair_u8, u8);
unsigned!(mul_u16, pair_u16, u16);
unsigned!(mul_u32, pair_u32, u32);
unsigned!(mul_u64, pair_u64, u64);
unsigned!(mul_usize, pair_usize, usize);
unsigned!(mul_u128, pair_u128, u128);
signed!(mul_i8, pair_i8, i8);
signed!(mul_i16, pair_i16, i16);
signed!(mul_i32, pair_i32, i32);
signed!(mul_i64, pair_i64, i64);
signed!(mul_isize, pair_isize, isize);
signed!(mul_i128, pair_i128, i128);

#[test]
fn wide_carries() {
    // Distinguish cross-product high bits, high*high, and each carry in the high word.
    let values: [u128; 12] = [
        0,
        1,
        u64::MAX as u128,
        1u128 << 64,
        (1u128 << 64) + 1,
        (1u128 << 65) - 1,
        (1u128 << 96) - 1,
        (1u128 << 127) - 1,
        1u128 << 127,
        u128::MAX,
        0xffff_ffff_ffff_fffe_ffff_ffff_ffff_ffff,
        0x8000_0000_0000_0001,
    ];
    let mut h = 0u64;
    for &a in &values {
        for &b in &values {
            let a = black_box(a);
            let b = black_box(b);
            let (u, uo) = a.overflowing_mul(b);
            let (s, so) = (a as i128).overflowing_mul(b as i128);
            h = mix(mix(h, u), uo as u128);
            h = mix(mix(h, s as u128), so as u128);
        }
    }
    println!("wide carries {:016x}", h);
    assert_eq!(black_box(i128::MIN).overflowing_mul(-1), (i128::MIN, true));
    assert_eq!(black_box(i128::MIN).overflowing_mul(1), (i128::MIN, false));
    assert_eq!(black_box(1u128 << 127).overflowing_mul(2), (0, true));
}
