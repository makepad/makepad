#![cfg(target_arch = "aarch64")]
#![allow(non_snake_case)]
include!("common/mod.rs");
include!("common/differential.rs");
use core::arch::aarch64 as real;
use relax_std_arch_check::hr_aarch64 as hr;

#[test]
fn __crc32b() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("crc"),
            "required native crc coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let p1: u8 = sample(&mut rng, case, 1);
            let got = hr::__crc32b(p0, p1);
            let expected = real::__crc32b(p0, p1);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn __crc32d() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("crc"),
            "required native crc coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let p1: u64 = sample(&mut rng, case, 1);
            let got = hr::__crc32d(p0, p1);
            let expected = real::__crc32d(p0, p1);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vaddq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            let got = hr::vaddq_s16(p0, p1);
            let expected = real::vaddq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vaddq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let p1: hr::int32x4_t = sample(&mut rng, case, 1);
            let got = hr::vaddq_s32(p0, p1);
            let expected = real::vaddq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vaddq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: hr::uint32x4_t = sample(&mut rng, case, 1);
            let got = hr::vaddq_u32(p0, p1);
            let expected = real::vaddq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vaddvq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let got = hr::vaddvq_u32(p0);
            let expected = real::vaddvq_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vaddw_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint16x8_t = sample(&mut rng, case, 0);
            let p1: hr::uint8x8_t = sample(&mut rng, case, 1);
            let got = hr::vaddw_u8(p0, p1);
            let expected = real::vaddw_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vcombine_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x4_t = sample(&mut rng, case, 0);
            let p1: hr::int16x4_t = sample(&mut rng, case, 1);
            let got = hr::vcombine_s16(p0, p1);
            let expected = real::vcombine_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vcombine_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint16x4_t = sample(&mut rng, case, 0);
            let p1: hr::uint16x4_t = sample(&mut rng, case, 1);
            let got = hr::vcombine_u16(p0, p1);
            let expected = real::vcombine_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vcombine_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x8_t = sample(&mut rng, case, 0);
            let p1: hr::uint8x8_t = sample(&mut rng, case, 1);
            let got = hr::vcombine_u8(p0, p1);
            let expected = real::vcombine_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vcreate_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u64 = sample(&mut rng, case, 0);
            let got = hr::vcreate_s16(p0);
            let expected = real::vcreate_s16(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vdupq_n_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: i16 = sample(&mut rng, case, 0);
            let got = hr::vdupq_n_s16(p0);
            let expected = real::vdupq_n_s16(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vdupq_n_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let got = hr::vdupq_n_s32(p0);
            let expected = real::vdupq_n_s32(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vdupq_n_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u16 = sample(&mut rng, case, 0);
            let got = hr::vdupq_n_u16(p0);
            let expected = real::vdupq_n_u16(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vdupq_n_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let got = hr::vdupq_n_u32(p0);
            let expected = real::vdupq_n_u32(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vdupq_n_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u8 = sample(&mut rng, case, 0);
            let got = hr::vdupq_n_u8(p0);
            let expected = real::vdupq_n_u8(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vget_high_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint16x8_t = sample(&mut rng, case, 0);
            let got = hr::vget_high_u16(p0);
            let expected = real::vget_high_u16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vget_high_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let got = hr::vget_high_u32(p0);
            let expected = real::vget_high_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vget_high_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x16_t = sample(&mut rng, case, 0);
            let got = hr::vget_high_u8(p0);
            let expected = real::vget_high_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vget_lane_u32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::uint32x2_t, usize)) {
        let got = hr::vget_lane_u32::<N>(p0);
        let expected = real::vget_lane_u32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x2_t = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case), [0, 1]);
        }
    }
}

#[test]
fn vget_low_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            let got = hr::vget_low_s16(p0);
            let expected = real::vget_low_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vget_low_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint16x8_t = sample(&mut rng, case, 0);
            let got = hr::vget_low_u16(p0);
            let expected = real::vget_low_u16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vget_low_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let got = hr::vget_low_u32(p0);
            let expected = real::vget_low_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vget_low_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x16_t = sample(&mut rng, case, 0);
            let got = hr::vget_low_u8(p0);
            let expected = real::vget_low_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vgetq_lane_u32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::uint32x4_t, usize)) {
        let got = hr::vgetq_lane_u32::<N>(p0);
        let expected = real::vgetq_lane_u32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case), [0, 1, 2, 3]);
        }
    }
}

#[test]
fn vld1_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::vld1_u16(p0.0.as_ptr().add((case % 4) * 2).cast());
            let expected = real::vld1_u16(q0.0.as_ptr().add((case % 4) * 2).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vld1q_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::vld1q_s16(p0.0.as_ptr().add((case % 4) * 2).cast());
            let expected = real::vld1q_s16(q0.0.as_ptr().add((case % 4) * 2).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vld1q_s16_x2() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::vld1q_s16_x2(p0.0.as_ptr().add((case % 4) * 2).cast());
            let expected = real::vld1q_s16_x2(q0.0.as_ptr().add((case % 4) * 2).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vld1q_s32_x2() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::vld1q_s32_x2(p0.0.as_ptr().add((case % 4) * 4).cast());
            let expected = real::vld1q_s32_x2(q0.0.as_ptr().add((case % 4) * 4).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vld1q_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::vld1q_u16(p0.0.as_ptr().add((case % 4) * 2).cast());
            let expected = real::vld1q_u16(q0.0.as_ptr().add((case % 4) * 2).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vld1q_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::vld1q_u32(p0.0.as_ptr().add((case % 4) * 4).cast());
            let expected = real::vld1q_u32(q0.0.as_ptr().add((case % 4) * 4).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vld1q_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::vld1q_u8(p0.0.as_ptr().add((case % 4) * 1).cast());
            let expected = real::vld1q_u8(q0.0.as_ptr().add((case % 4) * 1).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vmaxq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            let got = hr::vmaxq_s16(p0, p1);
            let expected = real::vmaxq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vmaxvq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let got = hr::vmaxvq_u32(p0);
            let expected = real::vmaxvq_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vminq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            let got = hr::vminq_s16(p0, p1);
            let expected = real::vminq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vmlal_high_laneq_s16() {
    unsafe fn compare<const N: i32>(
        (p0, p1, p2, case): (hr::int32x4_t, hr::int16x8_t, hr::int16x8_t, usize),
    ) {
        let got = hr::vmlal_high_laneq_s16::<N>(p0, p1, p2);
        let expected = real::vmlal_high_laneq_s16::<N>(cast(p0), cast(p1), cast(p2));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            let p2: hr::int16x8_t = sample(&mut rng, case, 2);
            immediate_cases!(compare, (p0, p1, p2, case), [0, 1, 2, 3, 4, 5, 6, 7]);
        }
    }
}

#[test]
fn vmlal_high_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: hr::uint16x8_t = sample(&mut rng, case, 1);
            let p2: hr::uint16x8_t = sample(&mut rng, case, 2);
            let got = hr::vmlal_high_u16(p0, p1, p2);
            let expected = real::vmlal_high_u16(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vmlal_laneq_s16() {
    unsafe fn compare<const N: i32>(
        (p0, p1, p2, case): (hr::int32x4_t, hr::int16x4_t, hr::int16x8_t, usize),
    ) {
        let got = hr::vmlal_laneq_s16::<N>(p0, p1, p2);
        let expected = real::vmlal_laneq_s16::<N>(cast(p0), cast(p1), cast(p2));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let p1: hr::int16x4_t = sample(&mut rng, case, 1);
            let p2: hr::int16x8_t = sample(&mut rng, case, 2);
            immediate_cases!(compare, (p0, p1, p2, case), [0, 1, 2, 3, 4, 5, 6, 7]);
        }
    }
}

#[test]
fn vmlal_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: hr::uint16x4_t = sample(&mut rng, case, 1);
            let p2: hr::uint16x4_t = sample(&mut rng, case, 2);
            let got = hr::vmlal_u16(p0, p1, p2);
            let expected = real::vmlal_u16(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vmulq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            let got = hr::vmulq_s16(p0, p1);
            let expected = real::vmulq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vmulq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let p1: hr::int32x4_t = sample(&mut rng, case, 1);
            let got = hr::vmulq_s32(p0, p1);
            let expected = real::vmulq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vorrq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let p1: hr::int32x4_t = sample(&mut rng, case, 1);
            let got = hr::vorrq_s32(p0, p1);
            let expected = real::vorrq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vpadalq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: hr::uint16x8_t = sample(&mut rng, case, 1);
            let got = hr::vpadalq_u16(p0, p1);
            let expected = real::vpadalq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vpadalq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint16x8_t = sample(&mut rng, case, 0);
            let p1: hr::uint8x16_t = sample(&mut rng, case, 1);
            let got = hr::vpadalq_u8(p0, p1);
            let expected = real::vpadalq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vpadd_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x2_t = sample(&mut rng, case, 0);
            let p1: hr::uint32x2_t = sample(&mut rng, case, 1);
            let got = hr::vpadd_u32(p0, p1);
            let expected = real::vpadd_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vpaddlq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x16_t = sample(&mut rng, case, 0);
            let got = hr::vpaddlq_u8(p0);
            let expected = real::vpaddlq_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vqmovn_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let got = hr::vqmovn_s32(p0);
            let expected = real::vqmovn_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vqmovn_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint16x8_t = sample(&mut rng, case, 0);
            let got = hr::vqmovn_u16(p0);
            let expected = real::vqmovn_u16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vqshlq_n_u32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::uint32x4_t, usize)) {
        let got = hr::vqshlq_n_u32::<N>(p0);
        let expected = real::vqshlq_n_u32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31
                ]
            );
        }
    }
}

#[test]
fn vqshrun_n_s32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::int32x4_t, usize)) {
        let got = hr::vqshrun_n_s32::<N>(p0);
        let expected = real::vqshrun_n_s32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
            );
        }
    }
}

#[test]
fn vreinterpretq_s32_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int64x2_t = sample(&mut rng, case, 0);
            let got = hr::vreinterpretq_s32_s64(p0);
            let expected = real::vreinterpretq_s32_s64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vreinterpretq_s32_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let got = hr::vreinterpretq_s32_u32(p0);
            let expected = real::vreinterpretq_s32_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vreinterpretq_s64_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let got = hr::vreinterpretq_s64_s32(p0);
            let expected = real::vreinterpretq_s64_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vreinterpretq_u32_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let got = hr::vreinterpretq_u32_s32(p0);
            let expected = real::vreinterpretq_u32_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vreinterpretq_u32_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x16_t = sample(&mut rng, case, 0);
            let got = hr::vreinterpretq_u32_u8(p0);
            let expected = real::vreinterpretq_u32_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vrev32q_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x16_t = sample(&mut rng, case, 0);
            let got = hr::vrev32q_u8(p0);
            let expected = real::vrev32q_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vsetq_lane_u32() {
    unsafe fn compare<const N: i32>((p0, p1, case): (u32, hr::uint32x4_t, usize)) {
        let got = hr::vsetq_lane_u32::<N>(p0, p1);
        let expected = real::vsetq_lane_u32::<N>(p0, cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let p1: hr::uint32x4_t = sample(&mut rng, case, 1);
            immediate_cases!(compare, (p0, p1, case), [0, 1, 2, 3]);
        }
    }
}

#[test]
fn vsha1cq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: u32 = sample(&mut rng, case, 1);
            let p2: hr::uint32x4_t = sample(&mut rng, case, 2);
            let got = hr::vsha1cq_u32(p0, p1, p2);
            let expected = real::vsha1cq_u32(cast(p0), p1, cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vsha1h_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let got = hr::vsha1h_u32(p0);
            let expected = real::vsha1h_u32(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vsha1mq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: u32 = sample(&mut rng, case, 1);
            let p2: hr::uint32x4_t = sample(&mut rng, case, 2);
            let got = hr::vsha1mq_u32(p0, p1, p2);
            let expected = real::vsha1mq_u32(cast(p0), p1, cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vsha1pq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: u32 = sample(&mut rng, case, 1);
            let p2: hr::uint32x4_t = sample(&mut rng, case, 2);
            let got = hr::vsha1pq_u32(p0, p1, p2);
            let expected = real::vsha1pq_u32(cast(p0), p1, cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vsha1su0q_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: hr::uint32x4_t = sample(&mut rng, case, 1);
            let p2: hr::uint32x4_t = sample(&mut rng, case, 2);
            let got = hr::vsha1su0q_u32(p0, p1, p2);
            let expected = real::vsha1su0q_u32(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vsha1su1q_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            let p1: hr::uint32x4_t = sample(&mut rng, case, 1);
            let got = hr::vsha1su1q_u32(p0, p1);
            let expected = real::vsha1su1q_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vshlq_n_u32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::uint32x4_t, usize)) {
        let got = hr::vshlq_n_u32::<N>(p0);
        let expected = real::vshlq_n_u32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31
                ]
            );
        }
    }
}

#[test]
fn vshrq_n_s16() {
    unsafe fn compare<const N: i32>((p0, case): (hr::int16x8_t, usize)) {
        let got = hr::vshrq_n_s16::<N>(p0);
        let expected = real::vshrq_n_s16::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
            );
        }
    }
}

#[test]
fn vshrq_n_s32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::int32x4_t, usize)) {
        let got = hr::vshrq_n_s32::<N>(p0);
        let expected = real::vshrq_n_s32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [
                    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                    23, 24, 25, 26, 27, 28, 29, 30, 31, 32
                ]
            );
        }
    }
}

#[test]
fn vst1q_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            hr::vst1q_s16(p0.0.as_mut_ptr().add((case % 4) * 2).cast(), p1);
            real::vst1q_s16(q0.0.as_mut_ptr().add((case % 4) * 2).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vst1q_s16_x2() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::int16x8x2_t = sample(&mut rng, case, 1);
            hr::vst1q_s16_x2(p0.0.as_mut_ptr().add((case % 4) * 2).cast(), p1);
            real::vst1q_s16_x2(q0.0.as_mut_ptr().add((case % 4) * 2).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vst1q_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::uint32x4_t = sample(&mut rng, case, 1);
            hr::vst1q_u32(p0.0.as_mut_ptr().add((case % 4) * 4).cast(), p1);
            real::vst1q_u32(q0.0.as_mut_ptr().add((case % 4) * 4).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vst3q_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::uint8x16x3_t = sample(&mut rng, case, 1);
            hr::vst3q_u8(p0.0.as_mut_ptr().add((case % 4) * 1).cast(), p1);
            real::vst3q_u8(q0.0.as_mut_ptr().add((case % 4) * 1).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vst4q_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::uint8x16x4_t = sample(&mut rng, case, 1);
            hr::vst4q_u8(p0.0.as_mut_ptr().add((case % 4) * 1).cast(), p1);
            real::vst4q_u8(q0.0.as_mut_ptr().add((case % 4) * 1).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vsubq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            let got = hr::vsubq_s16(p0, p1);
            let expected = real::vsubq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vsubq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let p1: hr::int32x4_t = sample(&mut rng, case, 1);
            let got = hr::vsubq_s32(p0, p1);
            let expected = real::vsubq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vtrn1q_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int64x2_t = sample(&mut rng, case, 0);
            let p1: hr::int64x2_t = sample(&mut rng, case, 1);
            let got = hr::vtrn1q_s64(p0, p1);
            let expected = real::vtrn1q_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vtrn2q_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int64x2_t = sample(&mut rng, case, 0);
            let p1: hr::int64x2_t = sample(&mut rng, case, 1);
            let got = hr::vtrn2q_s64(p0, p1);
            let expected = real::vtrn2q_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vtrnq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int32x4_t = sample(&mut rng, case, 0);
            let p1: hr::int32x4_t = sample(&mut rng, case, 1);
            let got = hr::vtrnq_s32(p0, p1);
            let expected = real::vtrnq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vzip1q_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            let got = hr::vzip1q_s16(p0, p1);
            let expected = real::vzip1q_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vzip2q_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int16x8_t = sample(&mut rng, case, 0);
            let p1: hr::int16x8_t = sample(&mut rng, case, 1);
            let got = hr::vzip2q_s16(p0, p1);
            let expected = real::vzip2q_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

// Intel SHA places A/W0 in the high word; Arm places them in lane zero.
// These cross-ISA checks give the scalar x86 SHA code thousands of native
// comparisons even when no x86 SHA-capable machine is available.
use relax_std_arch_check::hr_x86 as x86;

#[test]
fn x86_sha1msg1_against_arm() {
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let mut a: [u32; 4] = sample(&mut rng, case, 0);
            let mut b: [u32; 4] = sample(&mut rng, case, 1);
            let got = x86::_mm_sha1msg1_epu32(cast(a), cast(b));
            a.reverse();
            b.reverse();
            let mut expected: [u32; 4] =
                cast(real::vsha1su0q_u32(cast(a), cast(b), real::vdupq_n_u32(0)));
            expected.reverse();
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn x86_sha1msg2_against_arm() {
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let mut a: [u32; 4] = sample(&mut rng, case, 0);
            let mut b: [u32; 4] = sample(&mut rng, case, 1);
            let got = x86::_mm_sha1msg2_epu32(cast(a), cast(b));
            a.reverse();
            b.reverse();
            let mut expected: [u32; 4] = cast(real::vsha1su1q_u32(cast(a), cast(b)));
            expected.reverse();
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn x86_sha1nexte_against_arm() {
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let a: [u32; 4] = sample(&mut rng, case, 0);
            let b: [u32; 4] = sample(&mut rng, case, 1);
            let got = x86::_mm_sha1nexte_epu32(cast(a), cast(b));
            let mut expected = b;
            expected[3] = expected[3].wrapping_add(real::vsha1h_u32(a[3]));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn x86_sha1rnds4_against_arm() {
    unsafe fn check<const FUNC: i32>(a: [u32; 4], b: [u32; 4], case: usize) {
        let got = x86::_mm_sha1rnds4_epu32::<FUNC>(cast(a), cast(b));
        let mut state = a;
        state.reverse();
        let mut wk = b;
        wk.reverse();
        let k: u32 = [0x5a827999, 0x6ed9eba1, 0x8f1bbcdc, 0xca62c1d6][FUNC as usize];
        for w in &mut wk {
            *w = w.wrapping_add(k);
        }
        let native = match FUNC {
            0 => real::vsha1cq_u32(cast(state), 0, cast(wk)),
            2 => real::vsha1mq_u32(cast(state), 0, cast(wk)),
            _ => real::vsha1pq_u32(cast(state), 0, cast(wk)),
        };
        let mut expected: [u32; 4] = cast(native);
        expected.reverse();
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, FUNC {FUNC}");
    }
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let a = sample(&mut rng, case, 0);
            let b = sample(&mut rng, case, 1);
            check::<0>(a, b, case);
            check::<1>(a, b, case);
            check::<2>(a, b, case);
            check::<3>(a, b, case);
        }
    }
}

#[test]
fn vceq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x8_t = sample(&mut rng, case, 0);
            let p1: hr::uint8x8_t = sample(&mut rng, case, 1);
            let got = hr::vceq_u8(p0, p1);
            let expected = real::vceq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vdup_n_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u8 = sample(&mut rng, case, 0);
            let got = hr::vdup_n_u8(p0);
            let expected = real::vdup_n_u8(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vget_lane_u64() {
    unsafe fn compare<const N: i32>((p0, case): (hr::uint64x1_t, usize)) {
        let got = hr::vget_lane_u64::<N>(p0);
        let expected = real::vget_lane_u64::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint64x1_t = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case), [0]);
        }
    }
}

#[test]
fn vld1_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::vld1_u8(p0.0.as_ptr().add(case % 8).cast());
            let expected = real::vld1_u8(q0.0.as_ptr().add(case % 8).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vorr_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x8_t = sample(&mut rng, case, 0);
            let p1: hr::uint8x8_t = sample(&mut rng, case, 1);
            let got = hr::vorr_u8(p0, p1);
            let expected = real::vorr_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vreinterpret_s8_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x8_t = sample(&mut rng, case, 0);
            let got = hr::vreinterpret_s8_u8(p0);
            let expected = real::vreinterpret_s8_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vreinterpret_u64_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::uint8x8_t = sample(&mut rng, case, 0);
            let got = hr::vreinterpret_u64_u8(p0);
            let expected = real::vreinterpret_u64_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vst1_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::uint8x8_t = sample(&mut rng, case, 1);
            hr::vst1_u8(p0.0.as_mut_ptr().add(case % 8).cast(), p1);
            real::vst1_u8(q0.0.as_mut_ptr().add(case % 8).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vcgez_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int8x8_t = sample(&mut rng, case, 0);
            let got = hr::vcgez_s8(p0);
            let expected = real::vcgez_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn vcltz_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::int8x8_t = sample(&mut rng, case, 0);
            let got = hr::vcltz_s8(p0);
            let expected = real::vcltz_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

// Lane and duplicate pointer intrinsics use scalar-aligned addresses, as required
// by stdarch. Full-vector loads/stores also exercise byte-unaligned addresses.
// Raw float bits include quiet/signaling NaNs, infinities, signed zeros,
// subnormals, rounding ties and cancellation. The edge cases include the Cartesian
// product of all 24 patterns for up to three operands. Random cases retain all bits.
unsafe fn neon_sample<T: Copy>(rng: &mut Rng, case: usize, arg: usize, width: usize) -> T {
    if width == 0 || case >= 64 + 24 * 24 * 24 {
        return sample(rng, case, arg);
    }
    let mut value = MaybeUninit::<T>::uninit();
    let ptr = value.as_mut_ptr().cast::<u8>();
    for lane in 0..size_of::<T>() / width {
        let index = if case >= 64 {
            ((case - 64) / 24usize.pow(arg as u32)) % 24
        } else if case < 24 {
            case
        } else {
            (case + lane + arg * 7) % 24
        };
        if width == 4 {
            let bits: [u32; 24] = [
                0, 0x80000000, 0x7f800000, 0xff800000, 0x7fc00001, 0xffc12345, 0x7f800001,
                0xff812345, 1, 0x80000001, 0x007fffff, 0x00800000, 0x7f7fffff, 0xff7fffff,
                0x3f800000, 0xbf800000, 0x3f800001, 0x3f7fffff, 0x3f000000, 0xbf000000, 0x3fc00000,
                0xbfc00000, 0x4b800000, 0xcb800000,
            ];
            std::ptr::write_unaligned(ptr.add(lane * width).cast::<u32>(), bits[index]);
        } else {
            let bits: [u64; 24] = [
                0,
                0x8000000000000000,
                0x7ff0000000000000,
                0xfff0000000000000,
                0x7ff8000000000001,
                0xfff8123456789abc,
                0x7ff0000000000001,
                0xfff0123456789abc,
                1,
                0x8000000000000001,
                0x000fffffffffffff,
                0x0010000000000000,
                0x7fefffffffffffff,
                0xffefffffffffffff,
                0x3ff0000000000000,
                0xbff0000000000000,
                0x3ff0000000000001,
                0x3fefffffffffffff,
                0x3fe0000000000000,
                0xbfe0000000000000,
                0x3ff8000000000000,
                0xbff8000000000000,
                0x4340000000000000,
                0xc340000000000000,
            ];
            std::ptr::write_unaligned(ptr.add(lane * width).cast::<u64>(), bits[index]);
        }
    }
    value.assume_init()
}

#[test]
fn float32x2_t() {
    assert_eq!(size_of::<hr::float32x2_t>(), size_of::<real::float32x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::float32x2_t>(),
        std::mem::align_of::<real::float32x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float32x2x2_t() {
    assert_eq!(
        size_of::<hr::float32x2x2_t>(),
        size_of::<real::float32x2x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float32x2x2_t>(),
        std::mem::align_of::<real::float32x2x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float32x2x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x2x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float32x2x3_t() {
    assert_eq!(
        size_of::<hr::float32x2x3_t>(),
        size_of::<real::float32x2x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float32x2x3_t>(),
        std::mem::align_of::<real::float32x2x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float32x2x3_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x2x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float32x2x4_t() {
    assert_eq!(
        size_of::<hr::float32x2x4_t>(),
        size_of::<real::float32x2x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float32x2x4_t>(),
        std::mem::align_of::<real::float32x2x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float32x2x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x2x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float32x4_t() {
    assert_eq!(size_of::<hr::float32x4_t>(), size_of::<real::float32x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::float32x4_t>(),
        std::mem::align_of::<real::float32x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float32x4x2_t() {
    assert_eq!(
        size_of::<hr::float32x4x2_t>(),
        size_of::<real::float32x4x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float32x4x2_t>(),
        std::mem::align_of::<real::float32x4x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float32x4x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x4x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float32x4x3_t() {
    assert_eq!(
        size_of::<hr::float32x4x3_t>(),
        size_of::<real::float32x4x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float32x4x3_t>(),
        std::mem::align_of::<real::float32x4x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float32x4x3_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x4x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float32x4x4_t() {
    assert_eq!(
        size_of::<hr::float32x4x4_t>(),
        size_of::<real::float32x4x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float32x4x4_t>(),
        std::mem::align_of::<real::float32x4x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float32x4x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x4x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float64x1_t() {
    assert_eq!(size_of::<hr::float64x1_t>(), size_of::<real::float64x1_t>());
    assert_eq!(
        std::mem::align_of::<hr::float64x1_t>(),
        std::mem::align_of::<real::float64x1_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float64x1_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x1_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float64x1x2_t() {
    assert_eq!(
        size_of::<hr::float64x1x2_t>(),
        size_of::<real::float64x1x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float64x1x2_t>(),
        std::mem::align_of::<real::float64x1x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float64x1x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x1x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float64x1x3_t() {
    assert_eq!(
        size_of::<hr::float64x1x3_t>(),
        size_of::<real::float64x1x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float64x1x3_t>(),
        std::mem::align_of::<real::float64x1x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float64x1x3_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x1x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float64x1x4_t() {
    assert_eq!(
        size_of::<hr::float64x1x4_t>(),
        size_of::<real::float64x1x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float64x1x4_t>(),
        std::mem::align_of::<real::float64x1x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float64x1x4_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x1x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float64x2_t() {
    assert_eq!(size_of::<hr::float64x2_t>(), size_of::<real::float64x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::float64x2_t>(),
        std::mem::align_of::<real::float64x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float64x2x2_t() {
    assert_eq!(
        size_of::<hr::float64x2x2_t>(),
        size_of::<real::float64x2x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float64x2x2_t>(),
        std::mem::align_of::<real::float64x2x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float64x2x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x2x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float64x2x3_t() {
    assert_eq!(
        size_of::<hr::float64x2x3_t>(),
        size_of::<real::float64x2x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float64x2x3_t>(),
        std::mem::align_of::<real::float64x2x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float64x2x3_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x2x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn float64x2x4_t() {
    assert_eq!(
        size_of::<hr::float64x2x4_t>(),
        size_of::<real::float64x2x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::float64x2x4_t>(),
        std::mem::align_of::<real::float64x2x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::float64x2x4_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x2x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int16x4x2_t() {
    assert_eq!(size_of::<hr::int16x4x2_t>(), size_of::<real::int16x4x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::int16x4x2_t>(),
        std::mem::align_of::<real::int16x4x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int16x4x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x4x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int16x4x3_t() {
    assert_eq!(size_of::<hr::int16x4x3_t>(), size_of::<real::int16x4x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::int16x4x3_t>(),
        std::mem::align_of::<real::int16x4x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int16x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x4x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int16x4x4_t() {
    assert_eq!(size_of::<hr::int16x4x4_t>(), size_of::<real::int16x4x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::int16x4x4_t>(),
        std::mem::align_of::<real::int16x4x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int16x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x4x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int16x8x3_t() {
    assert_eq!(size_of::<hr::int16x8x3_t>(), size_of::<real::int16x8x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::int16x8x3_t>(),
        std::mem::align_of::<real::int16x8x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int16x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x8x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int16x8x4_t() {
    assert_eq!(size_of::<hr::int16x8x4_t>(), size_of::<real::int16x8x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::int16x8x4_t>(),
        std::mem::align_of::<real::int16x8x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int16x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x8x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int32x2x2_t() {
    assert_eq!(size_of::<hr::int32x2x2_t>(), size_of::<real::int32x2x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::int32x2x2_t>(),
        std::mem::align_of::<real::int32x2x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int32x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x2x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int32x2x3_t() {
    assert_eq!(size_of::<hr::int32x2x3_t>(), size_of::<real::int32x2x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::int32x2x3_t>(),
        std::mem::align_of::<real::int32x2x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int32x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x2x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int32x2x4_t() {
    assert_eq!(size_of::<hr::int32x2x4_t>(), size_of::<real::int32x2x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::int32x2x4_t>(),
        std::mem::align_of::<real::int32x2x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int32x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x2x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int32x4x3_t() {
    assert_eq!(size_of::<hr::int32x4x3_t>(), size_of::<real::int32x4x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::int32x4x3_t>(),
        std::mem::align_of::<real::int32x4x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int32x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x4x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int32x4x4_t() {
    assert_eq!(size_of::<hr::int32x4x4_t>(), size_of::<real::int32x4x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::int32x4x4_t>(),
        std::mem::align_of::<real::int32x4x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int32x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x4x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int64x1_t() {
    assert_eq!(size_of::<hr::int64x1_t>(), size_of::<real::int64x1_t>());
    assert_eq!(
        std::mem::align_of::<hr::int64x1_t>(),
        std::mem::align_of::<real::int64x1_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int64x1_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x1_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int64x1x2_t() {
    assert_eq!(size_of::<hr::int64x1x2_t>(), size_of::<real::int64x1x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::int64x1x2_t>(),
        std::mem::align_of::<real::int64x1x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int64x1x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x1x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int64x1x3_t() {
    assert_eq!(size_of::<hr::int64x1x3_t>(), size_of::<real::int64x1x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::int64x1x3_t>(),
        std::mem::align_of::<real::int64x1x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int64x1x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x1x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int64x1x4_t() {
    assert_eq!(size_of::<hr::int64x1x4_t>(), size_of::<real::int64x1x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::int64x1x4_t>(),
        std::mem::align_of::<real::int64x1x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int64x1x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x1x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int64x2x2_t() {
    assert_eq!(size_of::<hr::int64x2x2_t>(), size_of::<real::int64x2x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::int64x2x2_t>(),
        std::mem::align_of::<real::int64x2x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int64x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x2x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int64x2x3_t() {
    assert_eq!(size_of::<hr::int64x2x3_t>(), size_of::<real::int64x2x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::int64x2x3_t>(),
        std::mem::align_of::<real::int64x2x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int64x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x2x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int64x2x4_t() {
    assert_eq!(size_of::<hr::int64x2x4_t>(), size_of::<real::int64x2x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::int64x2x4_t>(),
        std::mem::align_of::<real::int64x2x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int64x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x2x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int8x16_t() {
    assert_eq!(size_of::<hr::int8x16_t>(), size_of::<real::int8x16_t>());
    assert_eq!(
        std::mem::align_of::<hr::int8x16_t>(),
        std::mem::align_of::<real::int8x16_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x16_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int8x16x2_t() {
    assert_eq!(size_of::<hr::int8x16x2_t>(), size_of::<real::int8x16x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::int8x16x2_t>(),
        std::mem::align_of::<real::int8x16x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int8x16x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x16x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int8x16x3_t() {
    assert_eq!(size_of::<hr::int8x16x3_t>(), size_of::<real::int8x16x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::int8x16x3_t>(),
        std::mem::align_of::<real::int8x16x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int8x16x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x16x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int8x16x4_t() {
    assert_eq!(size_of::<hr::int8x16x4_t>(), size_of::<real::int8x16x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::int8x16x4_t>(),
        std::mem::align_of::<real::int8x16x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int8x16x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x16x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int8x8x2_t() {
    assert_eq!(size_of::<hr::int8x8x2_t>(), size_of::<real::int8x8x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::int8x8x2_t>(),
        std::mem::align_of::<real::int8x8x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int8x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x8x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int8x8x3_t() {
    assert_eq!(size_of::<hr::int8x8x3_t>(), size_of::<real::int8x8x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::int8x8x3_t>(),
        std::mem::align_of::<real::int8x8x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int8x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x8x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn int8x8x4_t() {
    assert_eq!(size_of::<hr::int8x8x4_t>(), size_of::<real::int8x8x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::int8x8x4_t>(),
        std::mem::align_of::<real::int8x8x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::int8x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x8x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly16x4_t() {
    assert_eq!(size_of::<hr::poly16x4_t>(), size_of::<real::poly16x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly16x4_t>(),
        std::mem::align_of::<real::poly16x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly16x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly16x4x2_t() {
    assert_eq!(
        size_of::<hr::poly16x4x2_t>(),
        size_of::<real::poly16x4x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly16x4x2_t>(),
        std::mem::align_of::<real::poly16x4x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly16x4x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x4x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly16x4x3_t() {
    assert_eq!(
        size_of::<hr::poly16x4x3_t>(),
        size_of::<real::poly16x4x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly16x4x3_t>(),
        std::mem::align_of::<real::poly16x4x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly16x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x4x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly16x4x4_t() {
    assert_eq!(
        size_of::<hr::poly16x4x4_t>(),
        size_of::<real::poly16x4x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly16x4x4_t>(),
        std::mem::align_of::<real::poly16x4x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly16x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x4x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly16x8_t() {
    assert_eq!(size_of::<hr::poly16x8_t>(), size_of::<real::poly16x8_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly16x8_t>(),
        std::mem::align_of::<real::poly16x8_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x8_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly16x8x2_t() {
    assert_eq!(
        size_of::<hr::poly16x8x2_t>(),
        size_of::<real::poly16x8x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly16x8x2_t>(),
        std::mem::align_of::<real::poly16x8x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly16x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x8x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly16x8x3_t() {
    assert_eq!(
        size_of::<hr::poly16x8x3_t>(),
        size_of::<real::poly16x8x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly16x8x3_t>(),
        std::mem::align_of::<real::poly16x8x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly16x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x8x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly16x8x4_t() {
    assert_eq!(
        size_of::<hr::poly16x8x4_t>(),
        size_of::<real::poly16x8x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly16x8x4_t>(),
        std::mem::align_of::<real::poly16x8x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly16x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x8x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly64x1_t() {
    assert_eq!(size_of::<hr::poly64x1_t>(), size_of::<real::poly64x1_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly64x1_t>(),
        std::mem::align_of::<real::poly64x1_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly64x1_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x1_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly64x1x2_t() {
    assert_eq!(
        size_of::<hr::poly64x1x2_t>(),
        size_of::<real::poly64x1x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly64x1x2_t>(),
        std::mem::align_of::<real::poly64x1x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly64x1x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x1x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly64x1x3_t() {
    assert_eq!(
        size_of::<hr::poly64x1x3_t>(),
        size_of::<real::poly64x1x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly64x1x3_t>(),
        std::mem::align_of::<real::poly64x1x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly64x1x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x1x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly64x1x4_t() {
    assert_eq!(
        size_of::<hr::poly64x1x4_t>(),
        size_of::<real::poly64x1x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly64x1x4_t>(),
        std::mem::align_of::<real::poly64x1x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly64x1x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x1x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly64x2_t() {
    assert_eq!(size_of::<hr::poly64x2_t>(), size_of::<real::poly64x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly64x2_t>(),
        std::mem::align_of::<real::poly64x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly64x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly64x2x2_t() {
    assert_eq!(
        size_of::<hr::poly64x2x2_t>(),
        size_of::<real::poly64x2x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly64x2x2_t>(),
        std::mem::align_of::<real::poly64x2x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly64x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x2x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly64x2x3_t() {
    assert_eq!(
        size_of::<hr::poly64x2x3_t>(),
        size_of::<real::poly64x2x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly64x2x3_t>(),
        std::mem::align_of::<real::poly64x2x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly64x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x2x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly64x2x4_t() {
    assert_eq!(
        size_of::<hr::poly64x2x4_t>(),
        size_of::<real::poly64x2x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly64x2x4_t>(),
        std::mem::align_of::<real::poly64x2x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly64x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x2x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly8x16_t() {
    assert_eq!(size_of::<hr::poly8x16_t>(), size_of::<real::poly8x16_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly8x16_t>(),
        std::mem::align_of::<real::poly8x16_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x16_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly8x16x2_t() {
    assert_eq!(
        size_of::<hr::poly8x16x2_t>(),
        size_of::<real::poly8x16x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly8x16x2_t>(),
        std::mem::align_of::<real::poly8x16x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly8x16x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x16x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly8x16x3_t() {
    assert_eq!(
        size_of::<hr::poly8x16x3_t>(),
        size_of::<real::poly8x16x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly8x16x3_t>(),
        std::mem::align_of::<real::poly8x16x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly8x16x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x16x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly8x16x4_t() {
    assert_eq!(
        size_of::<hr::poly8x16x4_t>(),
        size_of::<real::poly8x16x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::poly8x16x4_t>(),
        std::mem::align_of::<real::poly8x16x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly8x16x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x16x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly8x8_t() {
    assert_eq!(size_of::<hr::poly8x8_t>(), size_of::<real::poly8x8_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly8x8_t>(),
        std::mem::align_of::<real::poly8x8_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly8x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x8_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly8x8x2_t() {
    assert_eq!(size_of::<hr::poly8x8x2_t>(), size_of::<real::poly8x8x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly8x8x2_t>(),
        std::mem::align_of::<real::poly8x8x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly8x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x8x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly8x8x3_t() {
    assert_eq!(size_of::<hr::poly8x8x3_t>(), size_of::<real::poly8x8x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly8x8x3_t>(),
        std::mem::align_of::<real::poly8x8x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly8x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x8x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn poly8x8x4_t() {
    assert_eq!(size_of::<hr::poly8x8x4_t>(), size_of::<real::poly8x8x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::poly8x8x4_t>(),
        std::mem::align_of::<real::poly8x8x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::poly8x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x8x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint16x4x2_t() {
    assert_eq!(
        size_of::<hr::uint16x4x2_t>(),
        size_of::<real::uint16x4x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint16x4x2_t>(),
        std::mem::align_of::<real::uint16x4x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint16x4x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x4x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint16x4x3_t() {
    assert_eq!(
        size_of::<hr::uint16x4x3_t>(),
        size_of::<real::uint16x4x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint16x4x3_t>(),
        std::mem::align_of::<real::uint16x4x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint16x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x4x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint16x4x4_t() {
    assert_eq!(
        size_of::<hr::uint16x4x4_t>(),
        size_of::<real::uint16x4x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint16x4x4_t>(),
        std::mem::align_of::<real::uint16x4x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint16x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x4x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint16x8x2_t() {
    assert_eq!(
        size_of::<hr::uint16x8x2_t>(),
        size_of::<real::uint16x8x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint16x8x2_t>(),
        std::mem::align_of::<real::uint16x8x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint16x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x8x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint16x8x3_t() {
    assert_eq!(
        size_of::<hr::uint16x8x3_t>(),
        size_of::<real::uint16x8x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint16x8x3_t>(),
        std::mem::align_of::<real::uint16x8x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint16x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x8x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint16x8x4_t() {
    assert_eq!(
        size_of::<hr::uint16x8x4_t>(),
        size_of::<real::uint16x8x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint16x8x4_t>(),
        std::mem::align_of::<real::uint16x8x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint16x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x8x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint32x2x2_t() {
    assert_eq!(
        size_of::<hr::uint32x2x2_t>(),
        size_of::<real::uint32x2x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint32x2x2_t>(),
        std::mem::align_of::<real::uint32x2x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint32x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x2x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint32x2x3_t() {
    assert_eq!(
        size_of::<hr::uint32x2x3_t>(),
        size_of::<real::uint32x2x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint32x2x3_t>(),
        std::mem::align_of::<real::uint32x2x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint32x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x2x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint32x2x4_t() {
    assert_eq!(
        size_of::<hr::uint32x2x4_t>(),
        size_of::<real::uint32x2x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint32x2x4_t>(),
        std::mem::align_of::<real::uint32x2x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint32x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x2x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint32x4x2_t() {
    assert_eq!(
        size_of::<hr::uint32x4x2_t>(),
        size_of::<real::uint32x4x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint32x4x2_t>(),
        std::mem::align_of::<real::uint32x4x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint32x4x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x4x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint32x4x3_t() {
    assert_eq!(
        size_of::<hr::uint32x4x3_t>(),
        size_of::<real::uint32x4x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint32x4x3_t>(),
        std::mem::align_of::<real::uint32x4x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint32x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x4x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint32x4x4_t() {
    assert_eq!(
        size_of::<hr::uint32x4x4_t>(),
        size_of::<real::uint32x4x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint32x4x4_t>(),
        std::mem::align_of::<real::uint32x4x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint32x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x4x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint64x1x2_t() {
    assert_eq!(
        size_of::<hr::uint64x1x2_t>(),
        size_of::<real::uint64x1x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint64x1x2_t>(),
        std::mem::align_of::<real::uint64x1x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint64x1x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x1x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint64x1x3_t() {
    assert_eq!(
        size_of::<hr::uint64x1x3_t>(),
        size_of::<real::uint64x1x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint64x1x3_t>(),
        std::mem::align_of::<real::uint64x1x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint64x1x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x1x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint64x1x4_t() {
    assert_eq!(
        size_of::<hr::uint64x1x4_t>(),
        size_of::<real::uint64x1x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint64x1x4_t>(),
        std::mem::align_of::<real::uint64x1x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint64x1x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x1x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint64x2x2_t() {
    assert_eq!(
        size_of::<hr::uint64x2x2_t>(),
        size_of::<real::uint64x2x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint64x2x2_t>(),
        std::mem::align_of::<real::uint64x2x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint64x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x2x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint64x2x3_t() {
    assert_eq!(
        size_of::<hr::uint64x2x3_t>(),
        size_of::<real::uint64x2x3_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint64x2x3_t>(),
        std::mem::align_of::<real::uint64x2x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint64x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x2x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint64x2x4_t() {
    assert_eq!(
        size_of::<hr::uint64x2x4_t>(),
        size_of::<real::uint64x2x4_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint64x2x4_t>(),
        std::mem::align_of::<real::uint64x2x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint64x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x2x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint8x16x2_t() {
    assert_eq!(
        size_of::<hr::uint8x16x2_t>(),
        size_of::<real::uint8x16x2_t>()
    );
    assert_eq!(
        std::mem::align_of::<hr::uint8x16x2_t>(),
        std::mem::align_of::<real::uint8x16x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint8x16x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint8x16x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint8x8x2_t() {
    assert_eq!(size_of::<hr::uint8x8x2_t>(), size_of::<real::uint8x8x2_t>());
    assert_eq!(
        std::mem::align_of::<hr::uint8x8x2_t>(),
        std::mem::align_of::<real::uint8x8x2_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint8x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint8x8x2_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint8x8x3_t() {
    assert_eq!(size_of::<hr::uint8x8x3_t>(), size_of::<real::uint8x8x3_t>());
    assert_eq!(
        std::mem::align_of::<hr::uint8x8x3_t>(),
        std::mem::align_of::<real::uint8x8x3_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint8x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint8x8x3_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn uint8x8x4_t() {
    assert_eq!(size_of::<hr::uint8x8x4_t>(), size_of::<real::uint8x8x4_t>());
    assert_eq!(
        std::mem::align_of::<hr::uint8x8x4_t>(),
        std::mem::align_of::<real::uint8x8x4_t>()
    );
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: hr::uint8x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint8x8x4_t = cast(got);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vabsq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vabsq_f32(p0);
            let expected = real::vabsq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vabsq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let got = hr::vabsq_f64(p0);
            let expected = real::vabsq_f64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vabsq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vabsq_s16(p0);
            let expected = real::vabsq_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vabsq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vabsq_s32(p0);
            let expected = real::vabsq_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vabsq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vabsq_s64(p0);
            let expected = real::vabsq_s64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vabsq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vabsq_s8(p0);
            let expected = real::vabsq_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vaddq_f32(p0, p1);
            let expected = real::vaddq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vaddq_f64(p0, p1);
            let expected = real::vaddq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vaddq_s64(p0, p1);
            let expected = real::vaddq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vaddq_s8(p0, p1);
            let expected = real::vaddq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vaddq_u16(p0, p1);
            let expected = real::vaddq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vaddq_u64(p0, p1);
            let expected = real::vaddq_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vaddq_u8(p0, p1);
            let expected = real::vaddq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddvq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vaddvq_f32(p0);
            let expected = real::vaddvq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
    unsafe {
        for a in [
            [16777216.0, 1.0, -16777216.0, 1.0],
            [1.0e30, -1.0e30, 1.0, 1.0],
            [-0.0, -0.0, -0.0, -0.0],
        ] {
            let a = std::hint::black_box(a);
            let got = hr::vaddvq_f32(hr::float32x4_t(a));
            let expected = real::vaddvq_f32(cast(a));
            assert_eq!(got.to_bits(), expected.to_bits(), "pairwise reduction");
        }
    }
}

#[test]
fn vaddvq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vaddvq_s16(p0);
            let expected = real::vaddvq_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddvq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vaddvq_s32(p0);
            let expected = real::vaddvq_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vaddvq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vaddvq_u16(p0);
            let expected = real::vaddvq_u16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vandq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vandq_s16(p0, p1);
            let expected = real::vandq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vandq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vandq_s32(p0, p1);
            let expected = real::vandq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vandq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vandq_s64(p0, p1);
            let expected = real::vandq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vandq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vandq_s8(p0, p1);
            let expected = real::vandq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vandq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vandq_u16(p0, p1);
            let expected = real::vandq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vandq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vandq_u32(p0, p1);
            let expected = real::vandq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vandq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vandq_u64(p0, p1);
            let expected = real::vandq_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vandq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vandq_u8(p0, p1);
            let expected = real::vandq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbicq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vbicq_u32(p0, p1);
            let expected = real::vbicq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: hr::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            let got = hr::vbslq_f32(p0, p1, p2);
            let expected = real::vbslq_f32(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::int16x8_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vbslq_s16(p0, p1, p2);
            let expected = real::vbslq_s16(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::int32x4_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vbslq_s32(p0, p1, p2);
            let expected = real::vbslq_s32(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::int64x2_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vbslq_s64(p0, p1, p2);
            let expected = real::vbslq_s64(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::int8x16_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vbslq_s8(p0, p1, p2);
            let expected = real::vbslq_s8(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::uint16x8_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vbslq_u16(p0, p1, p2);
            let expected = real::vbslq_u16(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::uint32x4_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vbslq_u32(p0, p1, p2);
            let expected = real::vbslq_u32(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::uint64x2_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vbslq_u64(p0, p1, p2);
            let expected = real::vbslq_u64(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vbslq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::uint8x16_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vbslq_u8(p0, p1, p2);
            let expected = real::vbslq_u8(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vceqq_f32(p0, p1);
            let expected = real::vceqq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vceqq_f64(p0, p1);
            let expected = real::vceqq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vceqq_s16(p0, p1);
            let expected = real::vceqq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vceqq_s32(p0, p1);
            let expected = real::vceqq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vceqq_s64(p0, p1);
            let expected = real::vceqq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vceqq_s8(p0, p1);
            let expected = real::vceqq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vceqq_u16(p0, p1);
            let expected = real::vceqq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vceqq_u32(p0, p1);
            let expected = real::vceqq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vceqq_u64(p0, p1);
            let expected = real::vceqq_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vceqq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vceqq_u8(p0, p1);
            let expected = real::vceqq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgeq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vcgeq_f32(p0, p1);
            let expected = real::vcgeq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgeq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vcgeq_f64(p0, p1);
            let expected = real::vcgeq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgtq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vcgtq_f32(p0, p1);
            let expected = real::vcgtq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgtq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vcgtq_f64(p0, p1);
            let expected = real::vcgtq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgtq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcgtq_s16(p0, p1);
            let expected = real::vcgtq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgtq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcgtq_s32(p0, p1);
            let expected = real::vcgtq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgtq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcgtq_s64(p0, p1);
            let expected = real::vcgtq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgtq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcgtq_s8(p0, p1);
            let expected = real::vcgtq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgtq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcgtq_u32(p0, p1);
            let expected = real::vcgtq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcgtq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcgtq_u64(p0, p1);
            let expected = real::vcgtq_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcleq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vcleq_f32(p0, p1);
            let expected = real::vcleq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcleq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vcleq_f64(p0, p1);
            let expected = real::vcleq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcltq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vcltq_f32(p0, p1);
            let expected = real::vcltq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcltq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vcltq_f64(p0, p1);
            let expected = real::vcltq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcltq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcltq_s16(p0, p1);
            let expected = real::vcltq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcltq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcltq_s32(p0, p1);
            let expected = real::vcltq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcltq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcltq_s64(p0, p1);
            let expected = real::vcltq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcltq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcltq_s8(p0, p1);
            let expected = real::vcltq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcombine_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcombine_s8(p0, p1);
            let expected = real::vcombine_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcombine_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vcombine_u32(p0, p1);
            let expected = real::vcombine_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcvtnq_s32_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vcvtnq_s32_f32(p0);
            let expected = real::vcvtnq_s32_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcvtq_f32_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vcvtq_f32_s32(p0);
            let expected = real::vcvtq_f32_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcvtq_f64_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vcvtq_f64_s64(p0);
            let expected = real::vcvtq_f64_s64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vcvtq_s32_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vcvtq_s32_f32(p0);
            let expected = real::vcvtq_s32_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vdivq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vdivq_f32(p0, p1);
            let expected = real::vdivq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vdivq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vdivq_f64(p0, p1);
            let expected = real::vdivq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vdupq_laneq_f32() {
    unsafe fn compare<const N: i32>(p0: hr::float32x4_t, case: usize) {
        let got = hr::vdupq_laneq_f32::<N>(p0);
        let expected = real::vdupq_laneq_f32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            compare::<0>(p0, case);
            compare::<1>(p0, case);
            compare::<2>(p0, case);
            compare::<3>(p0, case);
        }
    }
}

#[test]
fn vdupq_n_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f32 = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vdupq_n_f32(p0);
            let expected = real::vdupq_n_f32(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vdupq_n_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f64 = neon_sample(&mut rng, case, 0, 8);
            let got = hr::vdupq_n_f64(p0);
            let expected = real::vdupq_n_f64(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vdupq_n_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i64 = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vdupq_n_s64(p0);
            let expected = real::vdupq_n_s64(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vdupq_n_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i8 = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vdupq_n_s8(p0);
            let expected = real::vdupq_n_s8(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vdupq_n_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: u64 = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vdupq_n_u64(p0);
            let expected = real::vdupq_n_u64(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vdups_laneq_f32() {
    unsafe fn compare<const N: i32>(p0: hr::float32x4_t, case: usize) {
        let got = hr::vdups_laneq_f32::<N>(p0);
        let expected = real::vdups_laneq_f32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            compare::<0>(p0, case);
            compare::<1>(p0, case);
            compare::<2>(p0, case);
            compare::<3>(p0, case);
        }
    }
}

#[test]
fn veorq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::veorq_s16(p0, p1);
            let expected = real::veorq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn veorq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::veorq_s32(p0, p1);
            let expected = real::veorq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn veorq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::veorq_s64(p0, p1);
            let expected = real::veorq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn veorq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::veorq_s8(p0, p1);
            let expected = real::veorq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn veorq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::veorq_u16(p0, p1);
            let expected = real::veorq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn veorq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::veorq_u32(p0, p1);
            let expected = real::veorq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn veorq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::veorq_u64(p0, p1);
            let expected = real::veorq_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn veorq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::veorq_u8(p0, p1);
            let expected = real::veorq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vextq_f32() {
    unsafe fn compare<const N: i32>(p0: hr::float32x4_t, p1: hr::float32x4_t, case: usize) {
        let got = hr::vextq_f32::<N>(p0, p1);
        let expected = real::vextq_f32::<N>(cast(p0), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
            compare::<2>(p0, p1, case);
            compare::<3>(p0, p1, case);
        }
    }
}

#[test]
fn vextq_u32() {
    unsafe fn compare<const N: i32>(p0: hr::uint32x4_t, p1: hr::uint32x4_t, case: usize) {
        let got = hr::vextq_u32::<N>(p0, p1);
        let expected = real::vextq_u32::<N>(cast(p0), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
            compare::<2>(p0, p1, case);
            compare::<3>(p0, p1, case);
        }
    }
}

#[test]
fn vfmaq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: hr::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            let got = hr::vfmaq_f32(p0, p1, p2);
            let expected = real::vfmaq_f32(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vfmaq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let p2: hr::float64x2_t = neon_sample(&mut rng, case, 2, 8);
            let got = hr::vfmaq_f64(p0, p1, p2);
            let expected = real::vfmaq_f64(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vfmaq_laneq_f32() {
    unsafe fn compare<const LANE: i32>(
        p0: hr::float32x4_t,
        p1: hr::float32x4_t,
        p2: hr::float32x4_t,
        case: usize,
    ) {
        let got = hr::vfmaq_laneq_f32::<LANE>(p0, p1, p2);
        let expected = real::vfmaq_laneq_f32::<LANE>(cast(p0), cast(p1), cast(p2));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: hr::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            compare::<0>(p0, p1, p2, case);
            compare::<1>(p0, p1, p2, case);
            compare::<2>(p0, p1, p2, case);
            compare::<3>(p0, p1, p2, case);
        }
    }
}

#[test]
fn vfmaq_laneq_f64() {
    unsafe fn compare<const LANE: i32>(
        p0: hr::float64x2_t,
        p1: hr::float64x2_t,
        p2: hr::float64x2_t,
        case: usize,
    ) {
        let got = hr::vfmaq_laneq_f64::<LANE>(p0, p1, p2);
        let expected = real::vfmaq_laneq_f64::<LANE>(cast(p0), cast(p1), cast(p2));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let p2: hr::float64x2_t = neon_sample(&mut rng, case, 2, 8);
            compare::<0>(p0, p1, p2, case);
            compare::<1>(p0, p1, p2, case);
        }
    }
}

#[test]
fn vfmsq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: hr::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            let got = hr::vfmsq_f32(p0, p1, p2);
            let expected = real::vfmsq_f32(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vget_high_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vget_high_s16(p0);
            let expected = real::vget_high_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vget_high_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vget_high_s32(p0);
            let expected = real::vget_high_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vget_high_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vget_high_s8(p0);
            let expected = real::vget_high_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vget_low_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vget_low_s32(p0);
            let expected = real::vget_low_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vget_low_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vget_low_s8(p0);
            let expected = real::vget_low_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vgetq_lane_f32() {
    unsafe fn compare<const IMM5: i32>(p0: hr::float32x4_t, case: usize) {
        let got = hr::vgetq_lane_f32::<IMM5>(p0);
        let expected = real::vgetq_lane_f32::<IMM5>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            compare::<0>(p0, case);
            compare::<1>(p0, case);
            compare::<2>(p0, case);
            compare::<3>(p0, case);
        }
    }
}

#[test]
fn vgetq_lane_f64() {
    unsafe fn compare<const IMM5: i32>(p0: hr::float64x2_t, case: usize) {
        let got = hr::vgetq_lane_f64::<IMM5>(p0);
        let expected = real::vgetq_lane_f64::<IMM5>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            compare::<0>(p0, case);
            compare::<1>(p0, case);
        }
    }
}

#[test]
fn vgetq_lane_s64() {
    unsafe fn compare<const IMM5: i32>(p0: hr::int64x2_t, case: usize) {
        let got = hr::vgetq_lane_s64::<IMM5>(p0);
        let expected = real::vgetq_lane_s64::<IMM5>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            compare::<0>(p0, case);
            compare::<1>(p0, case);
        }
    }
}

#[test]
fn vgetq_lane_u64() {
    unsafe fn compare<const IMM5: i32>(p0: hr::uint64x2_t, case: usize) {
        let got = hr::vgetq_lane_u64::<IMM5>(p0);
        let expected = real::vgetq_lane_u64::<IMM5>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            compare::<0>(p0, case);
            compare::<1>(p0, case);
        }
    }
}

#[test]
fn vld1_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let q0 = p0;
            let got = hr::vld1_s8(p0.0.as_ptr().add(case % 16).cast());
            let expected = real::vld1_s8(q0.0.as_ptr().add(case % 16).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vld1q_dup_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let q0 = p0;
            let got = hr::vld1q_dup_f32(p0.0.as_ptr().add((case % 4) * 4).cast());
            let expected = real::vld1q_dup_f32(q0.0.as_ptr().add((case % 4) * 4).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vld1q_dup_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let q0 = p0;
            let got = hr::vld1q_dup_f64(p0.0.as_ptr().add((case % 4) * 8).cast());
            let expected = real::vld1q_dup_f64(q0.0.as_ptr().add((case % 4) * 8).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vld1q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let q0 = p0;
            let got = hr::vld1q_f32(p0.0.as_ptr().add(case % 16).cast());
            let expected = real::vld1q_f32(q0.0.as_ptr().add(case % 16).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vld1q_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let q0 = p0;
            let got = hr::vld1q_f64(p0.0.as_ptr().add(case % 16).cast());
            let expected = real::vld1q_f64(q0.0.as_ptr().add(case % 16).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vld1q_lane_f32() {
    unsafe fn compare<const LANE: i32>(p0: Buffer, p1: hr::float32x4_t, case: usize) {
        let q0 = p0;
        let got = hr::vld1q_lane_f32::<LANE>(p0.0.as_ptr().add((case % 4) * 4).cast(), p1);
        let expected =
            real::vld1q_lane_f32::<LANE>(q0.0.as_ptr().add((case % 4) * 4).cast(), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
            compare::<2>(p0, p1, case);
            compare::<3>(p0, p1, case);
        }
    }
}

#[test]
fn vld1q_lane_f64() {
    unsafe fn compare<const LANE: i32>(p0: Buffer, p1: hr::float64x2_t, case: usize) {
        let q0 = p0;
        let got = hr::vld1q_lane_f64::<LANE>(p0.0.as_ptr().add((case % 4) * 8).cast(), p1);
        let expected =
            real::vld1q_lane_f64::<LANE>(q0.0.as_ptr().add((case % 4) * 8).cast(), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
        }
    }
}

#[test]
fn vld1q_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let q0 = p0;
            let got = hr::vld1q_s32(p0.0.as_ptr().add(case % 16).cast());
            let expected = real::vld1q_s32(q0.0.as_ptr().add(case % 16).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vld1q_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let q0 = p0;
            let got = hr::vld1q_s8(p0.0.as_ptr().add(case % 16).cast());
            let expected = real::vld1q_s8(q0.0.as_ptr().add(case % 16).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxnmq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vmaxnmq_f32(p0, p1);
            let expected = real::vmaxnmq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxnmq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vmaxnmq_f64(p0, p1);
            let expected = real::vmaxnmq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxnmvq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vmaxnmvq_f32(p0);
            let expected = real::vmaxnmvq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vmaxq_f32(p0, p1);
            let expected = real::vmaxq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vmaxq_f64(p0, p1);
            let expected = real::vmaxq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmaxq_s8(p0, p1);
            let expected = real::vmaxq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmaxq_u16(p0, p1);
            let expected = real::vmaxq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmaxq_u32(p0, p1);
            let expected = real::vmaxq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmaxq_u8(p0, p1);
            let expected = real::vmaxq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxvq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmaxvq_s16(p0);
            let expected = real::vmaxvq_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxvq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmaxvq_s32(p0);
            let expected = real::vmaxvq_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmaxvq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmaxvq_s8(p0);
            let expected = real::vmaxvq_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminnmq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vminnmq_f32(p0, p1);
            let expected = real::vminnmq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminnmq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vminnmq_f64(p0, p1);
            let expected = real::vminnmq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminnmvq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vminnmvq_f32(p0);
            let expected = real::vminnmvq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vminq_f32(p0, p1);
            let expected = real::vminq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vminq_f64(p0, p1);
            let expected = real::vminq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vminq_s32(p0, p1);
            let expected = real::vminq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vminq_s8(p0, p1);
            let expected = real::vminq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vminq_u16(p0, p1);
            let expected = real::vminq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vminq_u32(p0, p1);
            let expected = real::vminq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vminq_u8(p0, p1);
            let expected = real::vminq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminvq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vminvq_s16(p0);
            let expected = real::vminvq_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminvq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vminvq_s32(p0);
            let expected = real::vminvq_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminvq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vminvq_s8(p0);
            let expected = real::vminvq_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminvq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vminvq_u16(p0);
            let expected = real::vminvq_u16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminvq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vminvq_u32(p0);
            let expected = real::vminvq_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vminvq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vminvq_u8(p0);
            let expected = real::vminvq_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmlaq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let p2: hr::int32x4_t = neon_sample(&mut rng, case, 2, 0);
            let got = hr::vmlaq_s32(p0, p1, p2);
            let expected = real::vmlaq_s32(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmlsq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: hr::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            let got = hr::vmlsq_f32(p0, p1, p2);
            let expected = real::vmlsq_f32(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovl_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmovl_s16(p0);
            let expected = real::vmovl_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovl_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x2_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmovl_s32(p0);
            let expected = real::vmovl_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovl_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmovl_s8(p0);
            let expected = real::vmovl_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovq_n_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f32 = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vmovq_n_f32(p0);
            let expected = real::vmovq_n_f32(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovq_n_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f64 = neon_sample(&mut rng, case, 0, 8);
            let got = hr::vmovq_n_f64(p0);
            let expected = real::vmovq_n_f64(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovq_n_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i16 = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmovq_n_s16(p0);
            let expected = real::vmovq_n_s16(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovq_n_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i32 = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmovq_n_s32(p0);
            let expected = real::vmovq_n_s32(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovq_n_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i64 = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmovq_n_s64(p0);
            let expected = real::vmovq_n_s64(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmovq_n_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: u32 = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmovq_n_u32(p0);
            let expected = real::vmovq_n_u32(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmull_high_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmull_high_s16(p0, p1);
            let expected = real::vmull_high_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmull_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmull_s16(p0, p1);
            let expected = real::vmull_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmull_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmull_s32(p0, p1);
            let expected = real::vmull_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmull_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmull_u16(p0, p1);
            let expected = real::vmull_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmull_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmull_u32(p0, p1);
            let expected = real::vmull_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmulq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vmulq_f32(p0, p1);
            let expected = real::vmulq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmulq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vmulq_f64(p0, p1);
            let expected = real::vmulq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmulq_laneq_f32() {
    unsafe fn compare<const LANE: i32>(p0: hr::float32x4_t, p1: hr::float32x4_t, case: usize) {
        let got = hr::vmulq_laneq_f32::<LANE>(p0, p1);
        let expected = real::vmulq_laneq_f32::<LANE>(cast(p0), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
            compare::<2>(p0, p1, case);
            compare::<3>(p0, p1, case);
        }
    }
}

#[test]
fn vmulq_n_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: f32 = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vmulq_n_f32(p0, p1);
            let expected = real::vmulq_n_f32(cast(p0), p1);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmulq_n_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: f64 = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vmulq_n_f64(p0, p1);
            let expected = real::vmulq_n_f64(cast(p0), p1);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmulq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmulq_u16(p0, p1);
            let expected = real::vmulq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmulq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vmulq_u32(p0, p1);
            let expected = real::vmulq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vmuls_laneq_f32() {
    unsafe fn compare<const LANE: i32>(p0: f32, p1: hr::float32x4_t, case: usize) {
        let got = hr::vmuls_laneq_f32::<LANE>(p0, p1);
        let expected = real::vmuls_laneq_f32::<LANE>(p0, cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f32 = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
            compare::<2>(p0, p1, case);
            compare::<3>(p0, p1, case);
        }
    }
}

#[test]
fn vmvnq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vmvnq_u32(p0);
            let expected = real::vmvnq_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vnegq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vnegq_f32(p0);
            let expected = real::vnegq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vnegq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vnegq_s32(p0);
            let expected = real::vnegq_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vorrq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vorrq_s16(p0, p1);
            let expected = real::vorrq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vorrq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vorrq_s64(p0, p1);
            let expected = real::vorrq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vorrq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vorrq_s8(p0, p1);
            let expected = real::vorrq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vorrq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vorrq_u16(p0, p1);
            let expected = real::vorrq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vorrq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vorrq_u32(p0, p1);
            let expected = real::vorrq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vorrq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vorrq_u64(p0, p1);
            let expected = real::vorrq_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vorrq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vorrq_u8(p0, p1);
            let expected = real::vorrq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vpaddlq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vpaddlq_s16(p0);
            let expected = real::vpaddlq_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vpaddq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vpaddq_f32(p0, p1);
            let expected = real::vpaddq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
    unsafe {
        let a = std::hint::black_box([16777216.0, 1.0, -16777216.0, 1.0]);
        let b = std::hint::black_box([1.0e30, -1.0e30, -0.0, -0.0]);
        let got = hr::vpaddq_f32(hr::float32x4_t(a), hr::float32x4_t(b));
        let expected = real::vpaddq_f32(cast(a), cast(b));
        assert_eq!(
            bytes(&got),
            bytes(&expected),
            "adjacent pairs from each vector"
        );
    }
}

#[test]
fn vpaddq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vpaddq_s32(p0, p1);
            let expected = real::vpaddq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vpmaxq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vpmaxq_u8(p0, p1);
            let expected = real::vpmaxq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqaddq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqaddq_s16(p0, p1);
            let expected = real::vqaddq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqaddq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqaddq_s8(p0, p1);
            let expected = real::vqaddq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqaddq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqaddq_u16(p0, p1);
            let expected = real::vqaddq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqaddq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqaddq_u8(p0, p1);
            let expected = real::vqaddq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqmovn_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vqmovn_s16(p0);
            let expected = real::vqmovn_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqmovun_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vqmovun_s16(p0);
            let expected = real::vqmovun_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqrdmulhq_n_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: i16 = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqrdmulhq_n_s16(p0, p1);
            let expected = real::vqrdmulhq_n_s16(cast(p0), p1);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqrdmulhq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqrdmulhq_s16(p0, p1);
            let expected = real::vqrdmulhq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqsubq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqsubq_s16(p0, p1);
            let expected = real::vqsubq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqsubq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqsubq_s8(p0, p1);
            let expected = real::vqsubq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqsubq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqsubq_u16(p0, p1);
            let expected = real::vqsubq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqsubq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqsubq_u8(p0, p1);
            let expected = real::vqsubq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqtbl1q_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqtbl1q_s8(p0, p1);
            let expected = real::vqtbl1q_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqtbl1q_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqtbl1q_u8(p0, p1);
            let expected = real::vqtbl1q_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vqtbl4q_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vqtbl4q_u8(p0, p1);
            let expected = real::vqtbl4q_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_f32_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_f32_s32(p0);
            let expected = real::vreinterpretq_f32_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_f32_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_f32_u32(p0);
            let expected = real::vreinterpretq_f32_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_f32_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_f32_u64(p0);
            let expected = real::vreinterpretq_f32_u64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_f32_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_f32_u8(p0);
            let expected = real::vreinterpretq_f32_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_f64_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_f64_u32(p0);
            let expected = real::vreinterpretq_f64_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_f64_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_f64_u64(p0);
            let expected = real::vreinterpretq_f64_u64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_s16_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_s16_s32(p0);
            let expected = real::vreinterpretq_s16_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_s16_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_s16_u16(p0);
            let expected = real::vreinterpretq_s16_u16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_s32_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vreinterpretq_s32_f32(p0);
            let expected = real::vreinterpretq_s32_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_s32_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_s32_s16(p0);
            let expected = real::vreinterpretq_s32_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_s64_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_s64_u64(p0);
            let expected = real::vreinterpretq_s64_u64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_s8_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_s8_u8(p0);
            let expected = real::vreinterpretq_s8_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u16_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_u16_s16(p0);
            let expected = real::vreinterpretq_u16_s16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u16_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_u16_s32(p0);
            let expected = real::vreinterpretq_u16_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u16_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_u16_u32(p0);
            let expected = real::vreinterpretq_u16_u32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u16_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_u16_u8(p0);
            let expected = real::vreinterpretq_u16_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u32_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vreinterpretq_u32_f32(p0);
            let expected = real::vreinterpretq_u32_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u32_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let got = hr::vreinterpretq_u32_f64(p0);
            let expected = real::vreinterpretq_u32_f64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u64_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vreinterpretq_u64_f32(p0);
            let expected = real::vreinterpretq_u64_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u64_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let got = hr::vreinterpretq_u64_f64(p0);
            let expected = real::vreinterpretq_u64_f64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u64_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_u64_s64(p0);
            let expected = real::vreinterpretq_u64_s64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u64_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_u64_u8(p0);
            let expected = real::vreinterpretq_u64_u8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u8_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_u8_s32(p0);
            let expected = real::vreinterpretq_u8_s32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vreinterpretq_u8_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let got = hr::vreinterpretq_u8_s8(p0);
            let expected = real::vreinterpretq_u8_s8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vrev64q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vrev64q_f32(p0);
            let expected = real::vrev64q_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vrndmq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vrndmq_f32(p0);
            let expected = real::vrndmq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vrndmq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let got = hr::vrndmq_f64(p0);
            let expected = real::vrndmq_f64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vrndnq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vrndnq_f32(p0);
            let expected = real::vrndnq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vrndpq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vrndpq_f32(p0);
            let expected = real::vrndpq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vrndpq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let got = hr::vrndpq_f64(p0);
            let expected = real::vrndpq_f64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vrndq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vrndq_f32(p0);
            let expected = real::vrndq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsetq_lane_f32() {
    unsafe fn compare<const LANE: i32>(p0: f32, p1: hr::float32x4_t, case: usize) {
        let got = hr::vsetq_lane_f32::<LANE>(p0, p1);
        let expected = real::vsetq_lane_f32::<LANE>(p0, cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f32 = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
            compare::<2>(p0, p1, case);
            compare::<3>(p0, p1, case);
        }
    }
}

#[test]
fn vsetq_lane_u64() {
    unsafe fn compare<const LANE: i32>(p0: u64, p1: hr::uint64x2_t, case: usize) {
        let got = hr::vsetq_lane_u64::<LANE>(p0, p1);
        let expected = real::vsetq_lane_u64::<LANE>(p0, cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: u64 = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
        }
    }
}

#[test]
fn vshlq_n_s32() {
    unsafe fn compare<const N: i32>(p0: hr::int32x4_t, case: usize) {
        let got = hr::vshlq_n_s32::<N>(p0);
        let expected = real::vshlq_n_s32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            compare::<0>(p0, case);
            compare::<1>(p0, case);
            compare::<2>(p0, case);
            compare::<3>(p0, case);
            compare::<4>(p0, case);
            compare::<5>(p0, case);
            compare::<6>(p0, case);
            compare::<7>(p0, case);
            compare::<8>(p0, case);
            compare::<9>(p0, case);
            compare::<10>(p0, case);
            compare::<11>(p0, case);
            compare::<12>(p0, case);
            compare::<13>(p0, case);
            compare::<14>(p0, case);
            compare::<15>(p0, case);
            compare::<16>(p0, case);
            compare::<17>(p0, case);
            compare::<18>(p0, case);
            compare::<19>(p0, case);
            compare::<20>(p0, case);
            compare::<21>(p0, case);
            compare::<22>(p0, case);
            compare::<23>(p0, case);
            compare::<24>(p0, case);
            compare::<25>(p0, case);
            compare::<26>(p0, case);
            compare::<27>(p0, case);
            compare::<28>(p0, case);
            compare::<29>(p0, case);
            compare::<30>(p0, case);
            compare::<31>(p0, case);
        }
    }
}

#[test]
fn vshlq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vshlq_s16(p0, p1);
            let expected = real::vshlq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vshlq_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vshlq_s32(p0, p1);
            let expected = real::vshlq_s32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vshlq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vshlq_s64(p0, p1);
            let expected = real::vshlq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vshlq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vshlq_u16(p0, p1);
            let expected = real::vshlq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vshlq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vshlq_u32(p0, p1);
            let expected = real::vshlq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vshlq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vshlq_u64(p0, p1);
            let expected = real::vshlq_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vshrn_n_u16() {
    unsafe fn compare<const N: i32>(p0: hr::uint16x8_t, case: usize) {
        let got = hr::vshrn_n_u16::<N>(p0);
        let expected = real::vshrn_n_u16::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            compare::<1>(p0, case);
            compare::<2>(p0, case);
            compare::<3>(p0, case);
            compare::<4>(p0, case);
            compare::<5>(p0, case);
            compare::<6>(p0, case);
            compare::<7>(p0, case);
            compare::<8>(p0, case);
        }
    }
}

#[test]
fn vshrn_n_u64() {
    unsafe fn compare<const N: i32>(p0: hr::uint64x2_t, case: usize) {
        let got = hr::vshrn_n_u64::<N>(p0);
        let expected = real::vshrn_n_u64::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            compare::<1>(p0, case);
            compare::<2>(p0, case);
            compare::<3>(p0, case);
            compare::<4>(p0, case);
            compare::<5>(p0, case);
            compare::<6>(p0, case);
            compare::<7>(p0, case);
            compare::<8>(p0, case);
            compare::<9>(p0, case);
            compare::<10>(p0, case);
            compare::<11>(p0, case);
            compare::<12>(p0, case);
            compare::<13>(p0, case);
            compare::<14>(p0, case);
            compare::<15>(p0, case);
            compare::<16>(p0, case);
            compare::<17>(p0, case);
            compare::<18>(p0, case);
            compare::<19>(p0, case);
            compare::<20>(p0, case);
            compare::<21>(p0, case);
            compare::<22>(p0, case);
            compare::<23>(p0, case);
            compare::<24>(p0, case);
            compare::<25>(p0, case);
            compare::<26>(p0, case);
            compare::<27>(p0, case);
            compare::<28>(p0, case);
            compare::<29>(p0, case);
            compare::<30>(p0, case);
            compare::<31>(p0, case);
            compare::<32>(p0, case);
        }
    }
}

#[test]
fn vshrq_n_u32() {
    unsafe fn compare<const N: i32>(p0: hr::uint32x4_t, case: usize) {
        let got = hr::vshrq_n_u32::<N>(p0);
        let expected = real::vshrq_n_u32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            compare::<1>(p0, case);
            compare::<2>(p0, case);
            compare::<3>(p0, case);
            compare::<4>(p0, case);
            compare::<5>(p0, case);
            compare::<6>(p0, case);
            compare::<7>(p0, case);
            compare::<8>(p0, case);
            compare::<9>(p0, case);
            compare::<10>(p0, case);
            compare::<11>(p0, case);
            compare::<12>(p0, case);
            compare::<13>(p0, case);
            compare::<14>(p0, case);
            compare::<15>(p0, case);
            compare::<16>(p0, case);
            compare::<17>(p0, case);
            compare::<18>(p0, case);
            compare::<19>(p0, case);
            compare::<20>(p0, case);
            compare::<21>(p0, case);
            compare::<22>(p0, case);
            compare::<23>(p0, case);
            compare::<24>(p0, case);
            compare::<25>(p0, case);
            compare::<26>(p0, case);
            compare::<27>(p0, case);
            compare::<28>(p0, case);
            compare::<29>(p0, case);
            compare::<30>(p0, case);
            compare::<31>(p0, case);
            compare::<32>(p0, case);
        }
    }
}

#[test]
fn vsqrtq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let got = hr::vsqrtq_f32(p0);
            let expected = real::vsqrtq_f32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsqrtq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let got = hr::vsqrtq_f64(p0);
            let expected = real::vsqrtq_f64(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vst1q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let mut q0 = p0;
            let got = hr::vst1q_f32(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            let expected = real::vst1q_f32(q0.0.as_mut_ptr().add(case % 16).cast(), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vst1q_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let mut q0 = p0;
            let got = hr::vst1q_f64(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            let expected = real::vst1q_f64(q0.0.as_mut_ptr().add(case % 16).cast(), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vst1q_lane_f32() {
    unsafe fn compare<const LANE: i32>(mut p0: Buffer, p1: hr::float32x4_t, case: usize) {
        let mut q0 = p0;
        let got = hr::vst1q_lane_f32::<LANE>(p0.0.as_mut_ptr().add((case % 4) * 4).cast(), p1);
        let expected =
            real::vst1q_lane_f32::<LANE>(q0.0.as_mut_ptr().add((case % 4) * 4).cast(), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        assert_eq!(p0.0, q0.0, "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
            compare::<2>(p0, p1, case);
            compare::<3>(p0, p1, case);
        }
    }
}

#[test]
fn vst1q_lane_f64() {
    unsafe fn compare<const LANE: i32>(mut p0: Buffer, p1: hr::float64x2_t, case: usize) {
        let mut q0 = p0;
        let got = hr::vst1q_lane_f64::<LANE>(p0.0.as_mut_ptr().add((case % 4) * 8).cast(), p1);
        let expected =
            real::vst1q_lane_f64::<LANE>(q0.0.as_mut_ptr().add((case % 4) * 8).cast(), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        assert_eq!(p0.0, q0.0, "case {case}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            compare::<0>(p0, p1, case);
            compare::<1>(p0, p1, case);
        }
    }
}

#[test]
fn vst1q_s32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let mut q0 = p0;
            let got = hr::vst1q_s32(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            let expected = real::vst1q_s32(q0.0.as_mut_ptr().add(case % 16).cast(), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vst1q_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let mut q0 = p0;
            let got = hr::vst1q_u8(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            let expected = real::vst1q_u8(q0.0.as_mut_ptr().add(case % 16).cast(), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vst3q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4x3_t = neon_sample(&mut rng, case, 1, 4);
            let mut q0 = p0;
            let got = hr::vst3q_f32(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            let expected = real::vst3q_f32(q0.0.as_mut_ptr().add(case % 16).cast(), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn vsubq_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vsubq_f32(p0, p1);
            let expected = real::vsubq_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsubq_f64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: hr::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let got = hr::vsubq_f64(p0, p1);
            let expected = real::vsubq_f64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsubq_s64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vsubq_s64(p0, p1);
            let expected = real::vsubq_s64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsubq_s8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vsubq_s8(p0, p1);
            let expected = real::vsubq_s8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsubq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vsubq_u16(p0, p1);
            let expected = real::vsubq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsubq_u32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vsubq_u32(p0, p1);
            let expected = real::vsubq_u32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsubq_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vsubq_u64(p0, p1);
            let expected = real::vsubq_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vsubq_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vsubq_u8(p0, p1);
            let expected = real::vsubq_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vtrn1q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vtrn1q_f32(p0, p1);
            let expected = real::vtrn1q_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vtrn2q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vtrn2q_f32(p0, p1);
            let expected = real::vtrn2q_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vtrnq_s16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vtrnq_s16(p0, p1);
            let expected = real::vtrnq_s16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vuzp1q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vuzp1q_f32(p0, p1);
            let expected = real::vuzp1q_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vuzp2q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vuzp2q_f32(p0, p1);
            let expected = real::vuzp2q_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vuzpq_u16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vuzpq_u16(p0, p1);
            let expected = real::vuzpq_u16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vzip1q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vzip1q_f32(p0, p1);
            let expected = real::vzip1q_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vzip2q_f32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: hr::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let got = hr::vzip2q_f32(p0, p1);
            let expected = real::vzip2q_f32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vzip2q_u64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vzip2q_u64(p0, p1);
            let expected = real::vzip2q_u64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[test]
fn vzip_u8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: hr::uint8x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: hr::uint8x8_t = neon_sample(&mut rng, case, 1, 0);
            let got = hr::vzip_u8(p0, p1);
            let expected = real::vzip_u8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}");
        }
    }
}

#[allow(dead_code)]
mod sha256_oracle {
    include!("common/sha256_oracle.rs");
}
include!("common/sha256_cases.rs");
