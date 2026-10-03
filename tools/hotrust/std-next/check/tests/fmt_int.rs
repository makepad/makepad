//! Integer Display/Debug/hex/octal/binary: HotRust's fmt vs real format!.

mod fmt_common;

use fmt_common::*;
use hotrust_std_check::fmt as hf;
use hotrust_std_check::fmt::rt;
use std::fmt as sf;

const P: u32 = rt::FLAG_PLUS;
const Z: u32 = rt::FLAG_ZERO_PAD;
const A: u32 = rt::FLAG_ALTERNATE;
const XL: u32 = rt::FLAG_DEBUG_LOWER_HEX;
const XU: u32 = rt::FLAG_DEBUG_UPPER_HEX;
const L: u8 = rt::ALIGN_LEFT;
const C: u8 = rt::ALIGN_CENTER;
const U: u8 = rt::ALIGN_UNKNOWN;

#[derive(Clone, Copy)]
enum Tr {
    Display,
    Debug,
    LowerHex,
    UpperHex,
    Octal,
    Binary,
}

const NCASES: usize = 19;

fn case(i: usize, w: usize) -> (Tr, Spec) {
    match i {
        0 => (Tr::Display, DEF),
        1 => (Tr::Display, spec(' ', U, 0, Some(w), None)),
        2 => (Tr::Display, spec(' ', U, P, None, None)),
        3 => (Tr::Display, spec(' ', U, Z, Some(w), None)),
        4 => (Tr::LowerHex, DEF),
        5 => (Tr::LowerHex, spec(' ', U, A, None, None)),
        6 => (Tr::UpperHex, DEF),
        7 => (Tr::UpperHex, spec(' ', U, A | Z, Some(10), None)),
        8 => (Tr::Octal, DEF),
        9 => (Tr::Octal, spec(' ', U, A, None, None)),
        10 => (Tr::Binary, DEF),
        11 => (Tr::Binary, spec(' ', U, A, Some(w), None)),
        12 => (Tr::Debug, DEF),
        13 => (Tr::Debug, spec(' ', U, XL, None, None)),
        14 => (Tr::Debug, spec(' ', U, XU | A, None, None)),
        15 => (Tr::Display, spec(' ', L, 0, Some(w), None)),
        16 => (Tr::Display, spec('-', C, 0, Some(w), None)),
        17 => (Tr::Display, spec(' ', U, P | Z, Some(w), None)),
        _ => (Tr::Display, spec(' ', U, 0, Some(w), Some(3))),
    }
}

fn real_case<T: sf::Display + sf::Debug + sf::LowerHex + sf::UpperHex + sf::Octal + sf::Binary>(i: usize, v: T, w: usize) -> String {
    match i {
        0 => format!("{}", v),
        1 => format!("{:w$}", v, w = w),
        2 => format!("{:+}", v),
        3 => format!("{:0w$}", v, w = w),
        4 => format!("{:x}", v),
        5 => format!("{:#x}", v),
        6 => format!("{:X}", v),
        7 => format!("{:#010X}", v),
        8 => format!("{:o}", v),
        9 => format!("{:#o}", v),
        10 => format!("{:b}", v),
        11 => format!("{:#w$b}", v, w = w),
        12 => format!("{:?}", v),
        13 => format!("{:x?}", v),
        14 => format!("{:#X?}", v),
        15 => format!("{:<w$}", v, w = w),
        16 => format!("{:-^w$}", v, w = w),
        17 => format!("{:+0w$}", v, w = w),
        _ => format!("{:w$.3}", v, w = w),
    }
}

fn mine<T: hf::Display + hf::Debug + hf::LowerHex + hf::UpperHex + hf::Octal + hf::Binary>(v: T, i: usize, w: usize) -> String {
    let (tr, s) = case(i, w);
    match tr {
        Tr::Display => render(&v, <T as hf::Display>::fmt, &s),
        Tr::Debug => render(&v, <T as hf::Debug>::fmt, &s),
        Tr::LowerHex => render(&v, <T as hf::LowerHex>::fmt, &s),
        Tr::UpperHex => render(&v, <T as hf::UpperHex>::fmt, &s),
        Tr::Octal => render(&v, <T as hf::Octal>::fmt, &s),
        Tr::Binary => render(&v, <T as hf::Binary>::fmt, &s),
    }
}

fn check_all<T>(vals: &[T], fails: &mut usize, total: &mut usize)
where
    T: Copy + sf::Display + sf::Debug + sf::LowerHex + sf::UpperHex + sf::Octal + sf::Binary,
    T: hf::Display + hf::Debug + hf::LowerHex + hf::UpperHex + hf::Octal + hf::Binary,
{
    for &v in vals {
        for i in 0..NCASES {
            for w in [0usize, 3, 12, 45] {
                *total += 1;
                let got = mine(v, i, w);
                let want = real_case(i, v, w);
                if got != want {
                    *fails += 1;
                    if *fails < 30 {
                        eprintln!("MISMATCH {} case {} w{}: got {:?} want {:?}", want, i, w, got, want);
                    }
                }
            }
        }
    }
}

#[test]
fn ints_match_real() {
    let mut r = Rng(12345);
    let mut fails = 0;
    let mut total = 0;
    let mut u64s: Vec<u64> = vec![0, 1, 9, 10, 99, 100, u64::MAX, u64::MAX / 2, 1 << 63, 255, 256];
    for _ in 0..2000 {
        let x = r.next();
        u64s.push(x >> r.below(64));
    }
    let i8s: Vec<i8> = u64s.iter().map(|&x| x as i8).chain([i8::MIN, i8::MAX, -1]).collect();
    let i16s: Vec<i16> = u64s.iter().map(|&x| x as i16).chain([i16::MIN, i16::MAX, -1]).collect();
    let i32s: Vec<i32> = u64s.iter().map(|&x| x as i32).chain([i32::MIN, i32::MAX, -1]).collect();
    let i64s: Vec<i64> = u64s.iter().map(|&x| x as i64).chain([i64::MIN, i64::MAX, -1]).collect();
    let isizes: Vec<isize> = u64s.iter().map(|&x| x as isize).collect();
    let u8s: Vec<u8> = u64s.iter().map(|&x| x as u8).collect();
    let u16s: Vec<u16> = u64s.iter().map(|&x| x as u16).collect();
    let u32s: Vec<u32> = u64s.iter().map(|&x| x as u32).collect();
    let usizes: Vec<usize> = u64s.iter().map(|&x| x as usize).collect();
    let mut u128s: Vec<u128> = vec![0, u128::MAX, 1 << 127, 10_000_000_000_000_000_000, 9_999_999_999_999_999_999];
    let mut i128s: Vec<i128> = vec![0, i128::MIN, i128::MAX, -1, -10_000_000_000_000_000_000];
    for _ in 0..500 {
        let x = ((r.next() as u128) << 64) | r.next() as u128;
        let s = r.below(128) as u32;
        u128s.push(x >> s);
        i128s.push((x as i128) >> s);
    }
    check_all(&i8s, &mut fails, &mut total);
    check_all(&i16s, &mut fails, &mut total);
    check_all(&i32s, &mut fails, &mut total);
    check_all(&i64s, &mut fails, &mut total);
    check_all(&isizes, &mut fails, &mut total);
    check_all(&u8s, &mut fails, &mut total);
    check_all(&u16s, &mut fails, &mut total);
    check_all(&u32s, &mut fails, &mut total);
    check_all(&u64s, &mut fails, &mut total);
    check_all(&usizes, &mut fails, &mut total);
    check_all(&u128s, &mut fails, &mut total);
    check_all(&i128s, &mut fails, &mut total);
    eprintln!("ints_match_real: {} comparisons, {} mismatches", total, fails);
    assert_eq!(fails, 0);
}
