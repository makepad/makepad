// The SSE4.2 explicit-length string compares _mm_cmpestri and _mm_cmpestrm (jetscii's byte-set and
// substring searches) for every control byte 0..=127: byte and word elements, signed and unsigned,
// the four aggregations, the four polarities and both index and mask forms, over lengths that are
// zero, partial, negative and past the element count; and _mm_extract_epi16 for every lane.
// Differential file: `| ` lines equal under rustc --test (the CPU's instructions) and makepad test.
#![cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

fn m128(b: &[u8; 16]) -> __m128i {
    unsafe { std::ptr::read_unaligned(b.as_ptr().cast()) }
}

const PAIRS: [(&[u8; 16], &[u8; 16]); 3] = [
    (b"<&az\x80\xff09AZ\0\0\0\0\0\0", b"ab<cd&ef\x80\x7f\xffZ09az"),
    (b"cd\0\0\0\0\0\0\0\0\0\0\0\0\0\0", b"abccdxcdxxxxxxxc"),
    (b"\x80\x7f\x01\xff\x00\x80\x7f\x01\x7f\x80\x01\x00\xff\x7f\x80\x01", b"\x7f\x80\x01\x00\xff\x7f\x80\x01\x80\x7f\x01\xff\x00\x80\x7f\x01"),
];

const LENGTHS: [(i32, i32); 9] = [(0, 0), (2, 8), (16, 16), (-3, 17), (17, -16), (i32::MIN, 5), (5, i32::MAX), (4, 4), (9, 12)];

fn row<const IMM: i32>() {
    let mut index = String::new();
    let mut mask: u64 = 0xcbf2_9ce4_8422_2325;
    for (a, b) in PAIRS {
        for (la, lb) in LENGTHS {
            let (a, b) = (m128(a), m128(b));
            let i = unsafe { _mm_cmpestri::<IMM>(a, la, b, lb) };
            index.push_str(&format!(" {i}"));
            let m: [u8; 16] = unsafe { std::mem::transmute(_mm_cmpestrm::<IMM>(a, la, b, lb)) };
            for x in m {
                mask = (mask ^ x as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    println!("| {IMM:3}{index} {mask:016x}");
}

macro_rules! rows {
    ($($imm:literal)*) => {$(row::<$imm>();)*};
}

#[test]
fn a01_cmpestr_every_control_byte() {
    rows!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
        32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63
        64 65 66 67 68 69 70 71 72 73 74 75 76 77 78 79 80 81 82 83 84 85 86 87 88 89 90 91 92 93 94 95
        96 97 98 99 100 101 102 103 104 105 106 107 108 109 110 111 112 113 114 115 116 117 118 119 120 121 122 123 124 125 126 127);
}

#[test]
fn a02_extract_epi16() {
    let v = m128(&[1, 0, 0, 128, 3, 0, 4, 0, 5, 0, 6, 0, 7, 0, 255, 255]);
    let lanes = unsafe {
        [_mm_extract_epi16::<0>(v), _mm_extract_epi16::<1>(v), _mm_extract_epi16::<2>(v), _mm_extract_epi16::<3>(v),
            _mm_extract_epi16::<4>(v), _mm_extract_epi16::<5>(v), _mm_extract_epi16::<6>(v), _mm_extract_epi16::<7>(v)]
    };
    println!("| {lanes:?}");
}
