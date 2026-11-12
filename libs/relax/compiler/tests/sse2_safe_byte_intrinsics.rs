//! Rust's value-only SSE2 intrinsics are safe within an SSE2-enabled function.
//! The memory load still needs an unsafe block. A byte scanner's classification, as a regression.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(target_arch = "x86_64")]

use std::arch::x86_64::*;

#[target_feature(enable = "sse2")]
unsafe fn masks(bytes: &[u8; 16]) -> (i32, i32) {
    let value = unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) };
    let low = _mm_set1_epi8(0x20);
    let high = _mm_set1_epi8(0x7e);
    let del = _mm_set1_epi8(0x7f);
    let minus_one = _mm_set1_epi8(-1);
    let ascii = _mm_or_si128(_mm_cmplt_epi8(value, low), _mm_cmpgt_epi8(value, high));
    let control = _mm_and_si128(_mm_cmplt_epi8(value, low), _mm_cmpgt_epi8(value, minus_one));
    let utf8 = _mm_or_si128(control, _mm_cmpeq_epi8(value, del));
    (_mm_movemask_epi8(ascii), _mm_movemask_epi8(utf8))
}

#[test]
fn safe_byte_operations_match_scalar_for_every_byte_and_lane() {
    for lane in 0..16 {
        for byte in 0..=255u8 {
            let mut bytes = [b'a'; 16];
            bytes[lane] = byte;
            let ascii = if !(0x20..=0x7e).contains(&byte) { 1 << lane } else { 0 };
            let utf8 = if byte < 0x20 || byte == 0x7f { 1 << lane } else { 0 };
            // SSE2 is part of the x86-64 baseline; the load is exactly 16 bytes.
            assert_eq!(unsafe { masks(&bytes) }, (ascii, utf8));
        }
    }
}
