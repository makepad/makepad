//! core::arch intrinsics with an immediate operand are called as stdarch spells them, with the
//! immediate as an argument (`rustc_legacy_const_generics`): aho-corasick's NEON Teddy calls
//! `vshrq_n_u8(v, BITS)` and `vextq_u8(a, b, N)` (relax issue 21). Each call is checked against
//! the turbofish form and a scalar model of the lanes.
#![allow(unused_unsafe)]

#[cfg(target_arch = "aarch64")]
#[test]
fn neon_shift_and_ext_immediates_are_arguments() {
    use std::arch::aarch64::*;
    use std::mem::transmute;
    let bytes: [u8; 16] = [0x80, 0x01, 0x7f, 0xff, 0x10, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc];
    let other: [u8; 16] = core::array::from_fn(|i| i as u8);
    unsafe {
        let u8s: uint8x16_t = transmute(bytes);
        let s8s: int8x16_t = transmute(bytes);
        let u16s: uint16x8_t = transmute(bytes);
        let s16s: int16x8_t = transmute(bytes);
        let u64s: uint64x2_t = transmute(bytes);
        let s64s: int64x2_t = transmute(bytes);
        let b8 = |v: [u8; 16]| v;
        assert_eq!(b8(transmute(vshrq_n_u8(u8s, 3))), b8(transmute(vshrq_n_u8::<3>(u8s))));
        assert_eq!(b8(transmute(vshrq_n_u8(u8s, 3))), bytes.map(|x| x >> 3));
        assert_eq!(b8(transmute(vshlq_n_u8(u8s, 2))), bytes.map(|x| x << 2));
        assert_eq!(b8(transmute(vshrq_n_s8(s8s, 2))), bytes.map(|x| ((x as i8) >> 2) as u8));
        assert_eq!(b8(transmute(vshlq_n_s8(s8s, 1))), bytes.map(|x| ((x as i8) << 1) as u8));
        let w16: [u16; 8] = transmute(bytes);
        let i16s: [i16; 8] = transmute(bytes);
        assert_eq!(transmute::<_, [u16; 8]>(vshrq_n_u16(u16s, 5)), w16.map(|x| x >> 5));
        assert_eq!(transmute::<_, [u16; 8]>(vshlq_n_u16(u16s, 4)), w16.map(|x| x << 4));
        assert_eq!(transmute::<_, [i16; 8]>(vshlq_n_s16(s16s, 3)), i16s.map(|x| x << 3));
        let w64: [u64; 2] = transmute(bytes);
        let i64s: [i64; 2] = transmute(bytes);
        assert_eq!(transmute::<_, [u64; 2]>(vshrq_n_u64(u64s, 9)), w64.map(|x| x >> 9));
        assert_eq!(transmute::<_, [u64; 2]>(vshlq_n_u64(u64s, 7)), w64.map(|x| x << 7));
        assert_eq!(transmute::<_, [i64; 2]>(vshrq_n_s64(s64s, 63)), i64s.map(|x| x >> 63));
        assert_eq!(transmute::<_, [i64; 2]>(vshlq_n_s64(s64s, 1)), i64s.map(|x| x << 1));
        let b: uint8x16_t = transmute(other);
        let ext = |n: usize| -> [u8; 16] { core::array::from_fn(|i| if i + n < 16 { bytes[i + n] } else { other[i + n - 16] }) };
        assert_eq!(b8(transmute(vextq_u8(u8s, b, 0))), ext(0));
        assert_eq!(b8(transmute(vextq_u8(u8s, b, 1))), ext(1));
        assert_eq!(b8(transmute(vextq_u8(u8s, b, 15))), ext(15));
        assert_eq!(b8(transmute(vextq_u8(u8s, b, 15))), b8(transmute(vextq_u8::<15>(u8s, b))));
        assert_eq!(b8(transmute(vextq_s8(s8s, transmute(b), 7))), ext(7));
        let d: [u8; 8] = transmute(vext_u8(vget_low_u8(u8s), vget_low_u8(b), 3));
        assert_eq!(d, [bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], other[0], other[1], other[2]]);
        let q16: [u16; 8] = transmute(vextq_u16(u16s, transmute(b), 6));
        let o16: [u16; 8] = transmute(other);
        assert_eq!(q16, [w16[6], w16[7], o16[0], o16[1], o16[2], o16[3], o16[4], o16[5]]);
        let q64: [u64; 2] = transmute(vextq_u64(u64s, transmute(b), 1));
        let o64: [u64; 2] = transmute(other);
        assert_eq!(q64, [w64[1], o64[0]]);
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn sse2_and_avx512_arithmetic_shift_immediates_are_arguments() {
    use std::arch::x86_64::*;
    use std::mem::transmute;
    let words: [i16; 8] = [-32768, -1, 0, 1, 255, -256, 32767, -12345];
    unsafe {
        let v: __m128i = transmute(words);
        assert_eq!(transmute::<_, [i16; 8]>(_mm_srai_epi16(v, 3)), words.map(|x| x >> 3));
        assert_eq!(transmute::<_, [i16; 8]>(_mm_srai_epi16(v, 3)), transmute::<_, [i16; 8]>(_mm_srai_epi16::<3>(v)));
        if is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("avx512vl") {
            srai_epi64(transmute([i64::MIN + 5, 77]));
        }
    }
    #[target_feature(enable = "avx512f,avx512vl")]
    unsafe fn srai_epi64(v: std::arch::x86_64::__m128i) {
        use std::arch::x86_64::*;
        use std::mem::transmute;
        assert_eq!(transmute::<_, [i64; 2]>(_mm_srai_epi64(v, 2)), [(i64::MIN + 5) >> 2, 77 >> 2]);
    }
}
