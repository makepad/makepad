#![cfg(target_arch = "aarch64")]
#![allow(non_snake_case)]
include!("arch/common.rs");
use core::arch::aarch64 as real;

#[test]
fn a001___crc32b() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("crc"),
            "required native crc coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let p1: u8 = sample(&mut rng, case, 1);
            let expected = real::__crc32b(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("__crc32b", "none");
}

#[test]
fn a002___crc32d() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("crc"),
            "required native crc coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let p1: u64 = sample(&mut rng, case, 1);
            let expected = real::__crc32d(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("__crc32d", "none");
}

#[test]
fn a003_vaddq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vaddq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_s16", "none");
}

#[test]
fn a004_vaddq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int32x4_t = sample(&mut rng, case, 1);
            let expected = real::vaddq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_s32", "none");
}

#[test]
fn a005_vaddq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint32x4_t = sample(&mut rng, case, 1);
            let expected = real::vaddq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_u32", "none");
}

#[test]
fn a006_vaddvq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let expected = real::vaddvq_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddvq_u32", "none");
}

#[test]
fn a007_vaddw_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let p1: real::uint8x8_t = sample(&mut rng, case, 1);
            let expected = real::vaddw_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddw_u8", "none");
}

#[test]
fn a008_vcombine_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x4_t = sample(&mut rng, case, 0);
            let p1: real::int16x4_t = sample(&mut rng, case, 1);
            let expected = real::vcombine_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcombine_s16", "none");
}

#[test]
fn a009_vcombine_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x4_t = sample(&mut rng, case, 0);
            let p1: real::uint16x4_t = sample(&mut rng, case, 1);
            let expected = real::vcombine_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcombine_u16", "none");
}

#[test]
fn a010_vcombine_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x8_t = sample(&mut rng, case, 0);
            let p1: real::uint8x8_t = sample(&mut rng, case, 1);
            let expected = real::vcombine_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcombine_u8", "none");
}

#[test]
fn a011_vcreate_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u64 = sample(&mut rng, case, 0);
            let expected = real::vcreate_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcreate_s16", "none");
}

#[test]
fn a012_vdupq_n_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: i16 = sample(&mut rng, case, 0);
            let expected = real::vdupq_n_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_s16", "none");
}

#[test]
fn a013_vdupq_n_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let expected = real::vdupq_n_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_s32", "none");
}

#[test]
fn a014_vdupq_n_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u16 = sample(&mut rng, case, 0);
            let expected = real::vdupq_n_u16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_u16", "none");
}

#[test]
fn a015_vdupq_n_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let expected = real::vdupq_n_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_u32", "none");
}

#[test]
fn a016_vdupq_n_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u8 = sample(&mut rng, case, 0);
            let expected = real::vdupq_n_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_u8", "none");
}

#[test]
fn a017_vget_high_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let expected = real::vget_high_u16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_high_u16", "none");
}

#[test]
fn a018_vget_high_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let expected = real::vget_high_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_high_u32", "none");
}

#[test]
fn a019_vget_high_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x16_t = sample(&mut rng, case, 0);
            let expected = real::vget_high_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_high_u8", "none");
}

#[test]
fn a020_vget_lane_u32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, _case, d): (real::uint32x2_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vget_lane_u32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x2_t = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case, &mut d), [0, 1]);
        }
    }
    d.line("vget_lane_u32", "0,1");
}

#[test]
fn a021_vget_low_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let expected = real::vget_low_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_low_s16", "none");
}

#[test]
fn a022_vget_low_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let expected = real::vget_low_u16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_low_u16", "none");
}

#[test]
fn a023_vget_low_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let expected = real::vget_low_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_low_u32", "none");
}

#[test]
fn a024_vget_low_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x16_t = sample(&mut rng, case, 0);
            let expected = real::vget_low_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_low_u8", "none");
}

#[test]
fn a025_vgetq_lane_u32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, _case, d): (real::uint32x4_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vgetq_lane_u32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case, &mut d), [0, 1, 2, 3]);
        }
    }
    d.line("vgetq_lane_u32", "0,1,2,3");
}

#[test]
fn a026_vld1_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::vld1_u16(p0.0.as_ptr().add((case % 4) * 2).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1_u16", "none");
}

#[test]
fn a027_vld1q_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::vld1q_s16(p0.0.as_ptr().add((case % 4) * 2).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_s16", "none");
}

#[test]
fn a028_vld1q_s16_x2() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::vld1q_s16_x2(p0.0.as_ptr().add((case % 4) * 2).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_s16_x2", "none");
}

#[test]
fn a029_vld1q_s32_x2() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::vld1q_s32_x2(p0.0.as_ptr().add((case % 4) * 4).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_s32_x2", "none");
}

#[test]
fn a030_vld1q_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::vld1q_u16(p0.0.as_ptr().add((case % 4) * 2).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_u16", "none");
}

#[test]
fn a031_vld1q_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::vld1q_u32(p0.0.as_ptr().add((case % 4) * 4).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_u32", "none");
}

#[test]
fn a032_vld1q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::vld1q_u8(p0.0.as_ptr().add((case % 4) * 1).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_u8", "none");
}

#[test]
fn a033_vmaxq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vmaxq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxq_s16", "none");
}

#[test]
fn a034_vmaxvq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let expected = real::vmaxvq_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxvq_u32", "none");
}

#[test]
fn a035_vminq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vminq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminq_s16", "none");
}

#[test]
fn a036_vmlal_high_laneq_s16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, p1, p2, _case, d): (real::int32x4_t, real::int16x8_t, real::int16x8_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vmlal_high_laneq_s16::<N>(p0, p1, p2);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let p2: real::int16x8_t = sample(&mut rng, case, 2);
            immediate_cases!(compare, (p0, p1, p2, case, &mut d), [0, 1, 2, 3, 4, 5, 6, 7]);
        }
    }
    d.line("vmlal_high_laneq_s16", "0,1,2,3,4,5,6,7");
}

#[test]
fn a037_vmlal_high_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint16x8_t = sample(&mut rng, case, 1);
            let p2: real::uint16x8_t = sample(&mut rng, case, 2);
            let expected = real::vmlal_high_u16(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlal_high_u16", "none");
}

#[test]
fn a038_vmlal_laneq_s16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, p1, p2, _case, d): (real::int32x4_t, real::int16x4_t, real::int16x8_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vmlal_laneq_s16::<N>(p0, p1, p2);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int16x4_t = sample(&mut rng, case, 1);
            let p2: real::int16x8_t = sample(&mut rng, case, 2);
            immediate_cases!(compare, (p0, p1, p2, case, &mut d), [0, 1, 2, 3, 4, 5, 6, 7]);
        }
    }
    d.line("vmlal_laneq_s16", "0,1,2,3,4,5,6,7");
}

#[test]
fn a039_vmlal_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint16x4_t = sample(&mut rng, case, 1);
            let p2: real::uint16x4_t = sample(&mut rng, case, 2);
            let expected = real::vmlal_u16(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlal_u16", "none");
}

#[test]
fn a040_vmulq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vmulq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmulq_s16", "none");
}

#[test]
fn a041_vmulq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int32x4_t = sample(&mut rng, case, 1);
            let expected = real::vmulq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmulq_s32", "none");
}

#[test]
fn a042_vorrq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int32x4_t = sample(&mut rng, case, 1);
            let expected = real::vorrq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorrq_s32", "none");
}

#[test]
fn a043_vpadalq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint16x8_t = sample(&mut rng, case, 1);
            let expected = real::vpadalq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vpadalq_u16", "none");
}

#[test]
fn a044_vpadalq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let p1: real::uint8x16_t = sample(&mut rng, case, 1);
            let expected = real::vpadalq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vpadalq_u8", "none");
}

#[test]
fn a045_vpadd_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x2_t = sample(&mut rng, case, 0);
            let p1: real::uint32x2_t = sample(&mut rng, case, 1);
            let expected = real::vpadd_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vpadd_u32", "none");
}

#[test]
fn a046_vpaddlq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x16_t = sample(&mut rng, case, 0);
            let expected = real::vpaddlq_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vpaddlq_u8", "none");
}

#[test]
fn a047_vqmovn_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let expected = real::vqmovn_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vqmovn_s32", "none");
}

#[test]
fn a048_vqmovn_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let expected = real::vqmovn_u16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vqmovn_u16", "none");
}

#[test]
fn a049_vqshlq_n_u32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, _case, d): (real::uint32x4_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vqshlq_n_u32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31
                ]
            );
        }
    }
    d.line("vqshlq_n_u32", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31");
}

#[test]
fn a050_vqshrun_n_s32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, _case, d): (real::int32x4_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vqshrun_n_s32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
            );
        }
    }
    d.line("vqshrun_n_s32", "1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16");
}

#[test]
fn a051_vreinterpretq_s32_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int64x2_t = sample(&mut rng, case, 0);
            let expected = real::vreinterpretq_s32_s64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s32_s64", "none");
}

#[test]
fn a052_vreinterpretq_s32_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let expected = real::vreinterpretq_s32_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s32_u32", "none");
}

#[test]
fn a053_vreinterpretq_s64_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let expected = real::vreinterpretq_s64_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s64_s32", "none");
}

#[test]
fn a054_vreinterpretq_u32_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let expected = real::vreinterpretq_u32_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u32_s32", "none");
}

#[test]
fn a055_vreinterpretq_u32_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x16_t = sample(&mut rng, case, 0);
            let expected = real::vreinterpretq_u32_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u32_u8", "none");
}

#[test]
fn a056_vrev32q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x16_t = sample(&mut rng, case, 0);
            let expected = real::vrev32q_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrev32q_u8", "none");
}

#[test]
fn a057_vsetq_lane_u32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, p1, _case, d): (u32, real::uint32x4_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vsetq_lane_u32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let p1: real::uint32x4_t = sample(&mut rng, case, 1);
            immediate_cases!(compare, (p0, p1, case, &mut d), [0, 1, 2, 3]);
        }
    }
    d.line("vsetq_lane_u32", "0,1,2,3");
}

#[test]
fn a058_vsha1cq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: u32 = sample(&mut rng, case, 1);
            let p2: real::uint32x4_t = sample(&mut rng, case, 2);
            let expected = real::vsha1cq_u32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vsha1cq_u32", "none");
}

#[test]
fn a059_vsha1h_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let expected = real::vsha1h_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vsha1h_u32", "none");
}

#[test]
fn a060_vsha1mq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: u32 = sample(&mut rng, case, 1);
            let p2: real::uint32x4_t = sample(&mut rng, case, 2);
            let expected = real::vsha1mq_u32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vsha1mq_u32", "none");
}

#[test]
fn a061_vsha1pq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: u32 = sample(&mut rng, case, 1);
            let p2: real::uint32x4_t = sample(&mut rng, case, 2);
            let expected = real::vsha1pq_u32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vsha1pq_u32", "none");
}

#[test]
fn a062_vsha1su0q_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint32x4_t = sample(&mut rng, case, 1);
            let p2: real::uint32x4_t = sample(&mut rng, case, 2);
            let expected = real::vsha1su0q_u32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vsha1su0q_u32", "none");
}

#[test]
fn a063_vsha1su1q_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("sha2"),
            "required native sha2 coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint32x4_t = sample(&mut rng, case, 1);
            let expected = real::vsha1su1q_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsha1su1q_u32", "none");
}

#[test]
fn a064_vshlq_n_u32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, _case, d): (real::uint32x4_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vshlq_n_u32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31
                ]
            );
        }
    }
    d.line("vshlq_n_u32", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31");
}

#[test]
fn a065_vshrq_n_s16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, _case, d): (real::int16x8_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vshrq_n_s16::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
            );
        }
    }
    d.line("vshrq_n_s16", "1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16");
}

#[test]
fn a066_vshrq_n_s32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, _case, d): (real::int32x4_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vshrq_n_s32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case, &mut d),
                [
                    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                    23, 24, 25, 26, 27, 28, 29, 30, 31, 32
                ]
            );
        }
    }
    d.line("vshrq_n_s32", "1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32");
}

#[test]
fn a067_vst1q_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            real::vst1q_s16(p0.0.as_mut_ptr().add((case % 4) * 2).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("vst1q_s16", "none");
}

#[test]
fn a068_vst1q_s16_x2() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::int16x8x2_t = sample(&mut rng, case, 1);
            real::vst1q_s16_x2(p0.0.as_mut_ptr().add((case % 4) * 2).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("vst1q_s16_x2", "none");
}

#[test]
fn a069_vst1q_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::uint32x4_t = sample(&mut rng, case, 1);
            real::vst1q_u32(p0.0.as_mut_ptr().add((case % 4) * 4).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("vst1q_u32", "none");
}

#[test]
fn a070_vst3q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::uint8x16x3_t = sample(&mut rng, case, 1);
            real::vst3q_u8(p0.0.as_mut_ptr().add((case % 4) * 1).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("vst3q_u8", "none");
}

#[test]
fn a071_vst4q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::uint8x16x4_t = sample(&mut rng, case, 1);
            real::vst4q_u8(p0.0.as_mut_ptr().add((case % 4) * 1).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("vst4q_u8", "none");
}

#[test]
fn a072_vsubq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vsubq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_s16", "none");
}

#[test]
fn a073_vsubq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int32x4_t = sample(&mut rng, case, 1);
            let expected = real::vsubq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_s32", "none");
}

#[test]
fn a074_vtrn1q_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int64x2_t = sample(&mut rng, case, 0);
            let p1: real::int64x2_t = sample(&mut rng, case, 1);
            let expected = real::vtrn1q_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_s64", "none");
}

#[test]
fn a075_vtrn2q_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int64x2_t = sample(&mut rng, case, 0);
            let p1: real::int64x2_t = sample(&mut rng, case, 1);
            let expected = real::vtrn2q_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_s64", "none");
}

#[test]
fn a076_vtrnq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int32x4_t = sample(&mut rng, case, 1);
            let expected = real::vtrnq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrnq_s32", "none");
}

#[test]
fn a077_vzip1q_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vzip1q_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vzip1q_s16", "none");
}

#[test]
fn a078_vzip2q_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vzip2q_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vzip2q_s16", "none");
}

#[test]
fn a083_vceq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x8_t = sample(&mut rng, case, 0);
            let p1: real::uint8x8_t = sample(&mut rng, case, 1);
            let expected = real::vceq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceq_u8", "none");
}

#[test]
fn a084_vdup_n_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: u8 = sample(&mut rng, case, 0);
            let expected = real::vdup_n_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdup_n_u8", "none");
}

#[test]
fn a085_vget_lane_u64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>((p0, _case, d): (real::uint64x1_t, usize, &mut Digest)) {
        d.add(bytes(&N));
        let expected = real::vget_lane_u64::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint64x1_t = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case, &mut d), [0]);
        }
    }
    d.line("vget_lane_u64", "0");
}

#[test]
fn a086_vld1_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let expected = real::vld1_u8(p0.0.as_ptr().add(case % 8).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1_u8", "none");
}

#[test]
fn a087_vorr_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x8_t = sample(&mut rng, case, 0);
            let p1: real::uint8x8_t = sample(&mut rng, case, 1);
            let expected = real::vorr_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorr_u8", "none");
}

#[test]
fn a088_vreinterpret_s8_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x8_t = sample(&mut rng, case, 0);
            let expected = real::vreinterpret_s8_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpret_s8_u8", "none");
}

#[test]
fn a089_vreinterpret_u64_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x8_t = sample(&mut rng, case, 0);
            let expected = real::vreinterpret_u64_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpret_u64_u8", "none");
}

#[test]
fn a090_vst1_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let p1: real::uint8x8_t = sample(&mut rng, case, 1);
            real::vst1_u8(p0.0.as_mut_ptr().add(case % 8).cast(), p1);
            d.add(&p0.0);
        }
    }
    d.line("vst1_u8", "none");
}

#[test]
fn a091_vcgez_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int8x8_t = sample(&mut rng, case, 0);
            let expected = real::vcgez_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgez_s8", "none");
}

#[test]
fn a092_vcltz_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int8x8_t = sample(&mut rng, case, 0);
            let expected = real::vcltz_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcltz_s8", "none");
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
fn a093_float32x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float32x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::float32x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float32x2_t", "none");
}

#[test]
fn a094_float32x2x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float32x2x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::float32x2x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float32x2x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x2x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float32x2x2_t", "none");
}

#[test]
fn a095_float32x2x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float32x2x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::float32x2x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float32x2x3_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x2x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float32x2x3_t", "none");
}

#[test]
fn a096_float32x2x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float32x2x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::float32x2x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float32x2x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x2x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float32x2x4_t", "none");
}

#[test]
fn a097_float32x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float32x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::float32x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float32x4_t", "none");
}

#[test]
fn a098_float32x4x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float32x4x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::float32x4x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float32x4x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x4x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float32x4x2_t", "none");
}

#[test]
fn a099_float32x4x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float32x4x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::float32x4x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float32x4x3_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x4x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float32x4x3_t", "none");
}

#[test]
fn a100_float32x4x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float32x4x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::float32x4x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float32x4x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected: real::float32x4x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float32x4x4_t", "none");
}

#[test]
fn a101_float64x1_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float64x1_t>()));
    d.add(bytes(&std::mem::align_of::<real::float64x1_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float64x1_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x1_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float64x1_t", "none");
}

#[test]
fn a102_float64x1x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float64x1x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::float64x1x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float64x1x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x1x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float64x1x2_t", "none");
}

#[test]
fn a103_float64x1x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float64x1x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::float64x1x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float64x1x3_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x1x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float64x1x3_t", "none");
}

#[test]
fn a104_float64x1x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float64x1x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::float64x1x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float64x1x4_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x1x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float64x1x4_t", "none");
}

#[test]
fn a105_float64x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float64x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::float64x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float64x2_t", "none");
}

#[test]
fn a106_float64x2x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float64x2x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::float64x2x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float64x2x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x2x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float64x2x2_t", "none");
}

#[test]
fn a107_float64x2x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float64x2x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::float64x2x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float64x2x3_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x2x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float64x2x3_t", "none");
}

#[test]
fn a108_float64x2x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::float64x2x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::float64x2x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::float64x2x4_t = neon_sample(&mut rng, case, 0, 8);
            let expected: real::float64x2x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("float64x2x4_t", "none");
}

#[test]
fn a109_int16x4x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int16x4x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::int16x4x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int16x4x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x4x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int16x4x2_t", "none");
}

#[test]
fn a110_int16x4x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int16x4x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::int16x4x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int16x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x4x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int16x4x3_t", "none");
}

#[test]
fn a111_int16x4x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int16x4x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::int16x4x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int16x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x4x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int16x4x4_t", "none");
}

#[test]
fn a112_int16x8x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int16x8x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::int16x8x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int16x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x8x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int16x8x3_t", "none");
}

#[test]
fn a113_int16x8x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int16x8x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::int16x8x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int16x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int16x8x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int16x8x4_t", "none");
}

#[test]
fn a114_int32x2x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int32x2x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::int32x2x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int32x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x2x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int32x2x2_t", "none");
}

#[test]
fn a115_int32x2x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int32x2x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::int32x2x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int32x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x2x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int32x2x3_t", "none");
}

#[test]
fn a116_int32x2x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int32x2x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::int32x2x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int32x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x2x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int32x2x4_t", "none");
}

#[test]
fn a117_int32x4x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int32x4x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::int32x4x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int32x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x4x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int32x4x3_t", "none");
}

#[test]
fn a118_int32x4x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int32x4x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::int32x4x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int32x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int32x4x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int32x4x4_t", "none");
}

#[test]
fn a119_int64x1_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int64x1_t>()));
    d.add(bytes(&std::mem::align_of::<real::int64x1_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int64x1_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x1_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int64x1_t", "none");
}

#[test]
fn a120_int64x1x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int64x1x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::int64x1x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int64x1x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x1x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int64x1x2_t", "none");
}

#[test]
fn a121_int64x1x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int64x1x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::int64x1x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int64x1x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x1x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int64x1x3_t", "none");
}

#[test]
fn a122_int64x1x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int64x1x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::int64x1x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int64x1x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x1x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int64x1x4_t", "none");
}

#[test]
fn a123_int64x2x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int64x2x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::int64x2x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int64x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x2x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int64x2x2_t", "none");
}

#[test]
fn a124_int64x2x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int64x2x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::int64x2x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int64x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x2x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int64x2x3_t", "none");
}

#[test]
fn a125_int64x2x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int64x2x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::int64x2x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int64x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int64x2x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int64x2x4_t", "none");
}

#[test]
fn a126_int8x16_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int8x16_t>()));
    d.add(bytes(&std::mem::align_of::<real::int8x16_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x16_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int8x16_t", "none");
}

#[test]
fn a127_int8x16x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int8x16x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::int8x16x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int8x16x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x16x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int8x16x2_t", "none");
}

#[test]
fn a128_int8x16x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int8x16x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::int8x16x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int8x16x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x16x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int8x16x3_t", "none");
}

#[test]
fn a129_int8x16x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int8x16x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::int8x16x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int8x16x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x16x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int8x16x4_t", "none");
}

#[test]
fn a130_int8x8x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int8x8x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::int8x8x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int8x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x8x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int8x8x2_t", "none");
}

#[test]
fn a131_int8x8x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int8x8x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::int8x8x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int8x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x8x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int8x8x3_t", "none");
}

#[test]
fn a132_int8x8x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::int8x8x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::int8x8x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::int8x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::int8x8x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("int8x8x4_t", "none");
}

#[test]
fn a133_poly16x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly16x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly16x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly16x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly16x4_t", "none");
}

#[test]
fn a134_poly16x4x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly16x4x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly16x4x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly16x4x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x4x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly16x4x2_t", "none");
}

#[test]
fn a135_poly16x4x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly16x4x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly16x4x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly16x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x4x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly16x4x3_t", "none");
}

#[test]
fn a136_poly16x4x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly16x4x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly16x4x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly16x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x4x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly16x4x4_t", "none");
}

#[test]
fn a137_poly16x8_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly16x8_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly16x8_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x8_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly16x8_t", "none");
}

#[test]
fn a138_poly16x8x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly16x8x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly16x8x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly16x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x8x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly16x8x2_t", "none");
}

#[test]
fn a139_poly16x8x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly16x8x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly16x8x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly16x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x8x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly16x8x3_t", "none");
}

#[test]
fn a140_poly16x8x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly16x8x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly16x8x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly16x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly16x8x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly16x8x4_t", "none");
}

#[test]
fn a141_poly64x1_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly64x1_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly64x1_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly64x1_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x1_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly64x1_t", "none");
}

#[test]
fn a142_poly64x1x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly64x1x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly64x1x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly64x1x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x1x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly64x1x2_t", "none");
}

#[test]
fn a143_poly64x1x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly64x1x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly64x1x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly64x1x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x1x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly64x1x3_t", "none");
}

#[test]
fn a144_poly64x1x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly64x1x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly64x1x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly64x1x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x1x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly64x1x4_t", "none");
}

#[test]
fn a145_poly64x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly64x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly64x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly64x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly64x2_t", "none");
}

#[test]
fn a146_poly64x2x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly64x2x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly64x2x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly64x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x2x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly64x2x2_t", "none");
}

#[test]
fn a147_poly64x2x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly64x2x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly64x2x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly64x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x2x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly64x2x3_t", "none");
}

#[test]
fn a148_poly64x2x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly64x2x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly64x2x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly64x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly64x2x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly64x2x4_t", "none");
}

#[test]
fn a149_poly8x16_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly8x16_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly8x16_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x16_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly8x16_t", "none");
}

#[test]
fn a150_poly8x16x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly8x16x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly8x16x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly8x16x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x16x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly8x16x2_t", "none");
}

#[test]
fn a151_poly8x16x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly8x16x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly8x16x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly8x16x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x16x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly8x16x3_t", "none");
}

#[test]
fn a152_poly8x16x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly8x16x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly8x16x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly8x16x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x16x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly8x16x4_t", "none");
}

#[test]
fn a153_poly8x8_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly8x8_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly8x8_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly8x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x8_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly8x8_t", "none");
}

#[test]
fn a154_poly8x8x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly8x8x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly8x8x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly8x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x8x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly8x8x2_t", "none");
}

#[test]
fn a155_poly8x8x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly8x8x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly8x8x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly8x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x8x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly8x8x3_t", "none");
}

#[test]
fn a156_poly8x8x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::poly8x8x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::poly8x8x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::poly8x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::poly8x8x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("poly8x8x4_t", "none");
}

#[test]
fn a157_uint16x4x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint16x4x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint16x4x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint16x4x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x4x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint16x4x2_t", "none");
}

#[test]
fn a158_uint16x4x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint16x4x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint16x4x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint16x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x4x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint16x4x3_t", "none");
}

#[test]
fn a159_uint16x4x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint16x4x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint16x4x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint16x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x4x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint16x4x4_t", "none");
}

#[test]
fn a160_uint16x8x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint16x8x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint16x8x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint16x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x8x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint16x8x2_t", "none");
}

#[test]
fn a161_uint16x8x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint16x8x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint16x8x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint16x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x8x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint16x8x3_t", "none");
}

#[test]
fn a162_uint16x8x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint16x8x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint16x8x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint16x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint16x8x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint16x8x4_t", "none");
}

#[test]
fn a163_uint32x2x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint32x2x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint32x2x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint32x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x2x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint32x2x2_t", "none");
}

#[test]
fn a164_uint32x2x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint32x2x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint32x2x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint32x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x2x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint32x2x3_t", "none");
}

#[test]
fn a165_uint32x2x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint32x2x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint32x2x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint32x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x2x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint32x2x4_t", "none");
}

#[test]
fn a166_uint32x4x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint32x4x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint32x4x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint32x4x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x4x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint32x4x2_t", "none");
}

#[test]
fn a167_uint32x4x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint32x4x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint32x4x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint32x4x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x4x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint32x4x3_t", "none");
}

#[test]
fn a168_uint32x4x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint32x4x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint32x4x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint32x4x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint32x4x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint32x4x4_t", "none");
}

#[test]
fn a169_uint64x1x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint64x1x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint64x1x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint64x1x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x1x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint64x1x2_t", "none");
}

#[test]
fn a170_uint64x1x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint64x1x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint64x1x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint64x1x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x1x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint64x1x3_t", "none");
}

#[test]
fn a171_uint64x1x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint64x1x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint64x1x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint64x1x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x1x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint64x1x4_t", "none");
}

#[test]
fn a172_uint64x2x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint64x2x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint64x2x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint64x2x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x2x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint64x2x2_t", "none");
}

#[test]
fn a173_uint64x2x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint64x2x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint64x2x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint64x2x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x2x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint64x2x3_t", "none");
}

#[test]
fn a174_uint64x2x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint64x2x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint64x2x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint64x2x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint64x2x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint64x2x4_t", "none");
}

#[test]
fn a175_uint8x16x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint8x16x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint8x16x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint8x16x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint8x16x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint8x16x2_t", "none");
}

#[test]
fn a176_uint8x8x2_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint8x8x2_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint8x8x2_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint8x8x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint8x8x2_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint8x8x2_t", "none");
}

#[test]
fn a177_uint8x8x3_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint8x8x3_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint8x8x3_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint8x8x3_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint8x8x3_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint8x8x3_t", "none");
}

#[test]
fn a178_uint8x8x4_t() {
    let mut d = Digest::new();
    d.add(bytes(&size_of::<real::uint8x8x4_t>()));
    d.add(bytes(&std::mem::align_of::<real::uint8x8x4_t>()));
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let got: real::uint8x8x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected: real::uint8x8x4_t = got;
            d.add(bytes(&expected));
        }
    }
    d.line("uint8x8x4_t", "none");
}

#[test]
fn a179_vabsq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vabsq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vabsq_f32", "none");
}

#[test]
fn a180_vabsq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vabsq_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vabsq_f64", "none");
}

#[test]
fn a181_vabsq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vabsq_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vabsq_s16", "none");
}

#[test]
fn a182_vabsq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vabsq_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vabsq_s32", "none");
}

#[test]
fn a183_vabsq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vabsq_s64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vabsq_s64", "none");
}

#[test]
fn a184_vabsq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vabsq_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vabsq_s8", "none");
}

#[test]
fn a185_vaddq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vaddq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_f32", "none");
}

#[test]
fn a186_vaddq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vaddq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_f64", "none");
}

#[test]
fn a187_vaddq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vaddq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_s64", "none");
}

#[test]
fn a188_vaddq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vaddq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_s8", "none");
}

#[test]
fn a189_vaddq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vaddq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_u16", "none");
}

#[test]
fn a190_vaddq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vaddq_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_u64", "none");
}

#[test]
fn a191_vaddq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vaddq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddq_u8", "none");
}

#[test]
fn a192_vaddvq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vaddvq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    unsafe {
        for a in [
            [16777216.0, 1.0, -16777216.0, 1.0],
            [1.0e30, -1.0e30, 1.0, 1.0],
            [-0.0, -0.0, -0.0, -0.0],
        ] {
            let a: [f32; 4] = std::hint::black_box(a);
            let expected = real::vaddvq_f32(cast(a));
            d.add(bytes(&expected));
        }
    }
    d.line("vaddvq_f32", "none");
}

#[test]
fn a193_vaddvq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vaddvq_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddvq_s16", "none");
}

#[test]
fn a194_vaddvq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vaddvq_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddvq_s32", "none");
}

#[test]
fn a195_vaddvq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vaddvq_u16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vaddvq_u16", "none");
}

#[test]
fn a196_vandq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vandq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vandq_s16", "none");
}

#[test]
fn a197_vandq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vandq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vandq_s32", "none");
}

#[test]
fn a198_vandq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vandq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vandq_s64", "none");
}

#[test]
fn a199_vandq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vandq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vandq_s8", "none");
}

#[test]
fn a200_vandq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vandq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vandq_u16", "none");
}

#[test]
fn a201_vandq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vandq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vandq_u32", "none");
}

#[test]
fn a202_vandq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vandq_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vandq_u64", "none");
}

#[test]
fn a203_vandq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vandq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vandq_u8", "none");
}

#[test]
fn a204_vbicq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vbicq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vbicq_u32", "none");
}

#[test]
fn a205_vbslq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: real::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            let expected = real::vbslq_f32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_f32", "none");
}

#[test]
fn a206_vbslq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::int16x8_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vbslq_s16(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_s16", "none");
}

#[test]
fn a207_vbslq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::int32x4_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vbslq_s32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_s32", "none");
}

#[test]
fn a208_vbslq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::int64x2_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vbslq_s64(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_s64", "none");
}

#[test]
fn a209_vbslq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::int8x16_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vbslq_s8(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_s8", "none");
}

#[test]
fn a210_vbslq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::uint16x8_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vbslq_u16(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_u16", "none");
}

#[test]
fn a211_vbslq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::uint32x4_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vbslq_u32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_u32", "none");
}

#[test]
fn a212_vbslq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::uint64x2_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vbslq_u64(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_u64", "none");
}

#[test]
fn a213_vbslq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::uint8x16_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vbslq_u8(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbslq_u8", "none");
}

#[test]
fn a214_vceqq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vceqq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_f32", "none");
}

#[test]
fn a215_vceqq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vceqq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_f64", "none");
}

#[test]
fn a216_vceqq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vceqq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_s16", "none");
}

#[test]
fn a217_vceqq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vceqq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_s32", "none");
}

#[test]
fn a218_vceqq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vceqq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_s64", "none");
}

#[test]
fn a219_vceqq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vceqq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_s8", "none");
}

#[test]
fn a220_vceqq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vceqq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_u16", "none");
}

#[test]
fn a221_vceqq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vceqq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_u32", "none");
}

#[test]
fn a222_vceqq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vceqq_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_u64", "none");
}

#[test]
fn a223_vceqq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vceqq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vceqq_u8", "none");
}

#[test]
fn a224_vcgeq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vcgeq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgeq_f32", "none");
}

#[test]
fn a225_vcgeq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vcgeq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgeq_f64", "none");
}

#[test]
fn a226_vcgtq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vcgtq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgtq_f32", "none");
}

#[test]
fn a227_vcgtq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vcgtq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgtq_f64", "none");
}

#[test]
fn a228_vcgtq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcgtq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgtq_s16", "none");
}

#[test]
fn a229_vcgtq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcgtq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgtq_s32", "none");
}

#[test]
fn a230_vcgtq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcgtq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgtq_s64", "none");
}

#[test]
fn a231_vcgtq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcgtq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgtq_s8", "none");
}

#[test]
fn a232_vcgtq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcgtq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgtq_u32", "none");
}

#[test]
fn a233_vcgtq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcgtq_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcgtq_u64", "none");
}

#[test]
fn a234_vcleq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vcleq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcleq_f32", "none");
}

#[test]
fn a235_vcleq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vcleq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcleq_f64", "none");
}

#[test]
fn a236_vcltq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vcltq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcltq_f32", "none");
}

#[test]
fn a237_vcltq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vcltq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcltq_f64", "none");
}

#[test]
fn a238_vcltq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcltq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcltq_s16", "none");
}

#[test]
fn a239_vcltq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcltq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcltq_s32", "none");
}

#[test]
fn a240_vcltq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcltq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcltq_s64", "none");
}

#[test]
fn a241_vcltq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcltq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcltq_s8", "none");
}

#[test]
fn a242_vcombine_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcombine_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcombine_s8", "none");
}

#[test]
fn a243_vcombine_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vcombine_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vcombine_u32", "none");
}

#[test]
fn a244_vcvtnq_s32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vcvtnq_s32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtnq_s32_f32", "none");
}

#[test]
fn a245_vcvtq_f32_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vcvtq_f32_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtq_f32_s32", "none");
}

#[test]
fn a246_vcvtq_f64_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vcvtq_f64_s64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtq_f64_s64", "none");
}

#[test]
fn a247_vcvtq_s32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vcvtq_s32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtq_s32_f32", "none");
}

#[test]
fn a248_vdivq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vdivq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vdivq_f32", "none");
}

#[test]
fn a249_vdivq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vdivq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vdivq_f64", "none");
}

#[test]
fn a250_vdupq_laneq_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::float32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vdupq_laneq_f32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            compare::<0>(p0, case, &mut d);
            compare::<1>(p0, case, &mut d);
            compare::<2>(p0, case, &mut d);
            compare::<3>(p0, case, &mut d);
        }
    }
    d.line("vdupq_laneq_f32", "0,1,2,3");
}

#[test]
fn a251_vdupq_n_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f32 = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vdupq_n_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_f32", "none");
}

#[test]
fn a252_vdupq_n_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f64 = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vdupq_n_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_f64", "none");
}

#[test]
fn a253_vdupq_n_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i64 = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vdupq_n_s64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_s64", "none");
}

#[test]
fn a254_vdupq_n_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i8 = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vdupq_n_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_s8", "none");
}

#[test]
fn a255_vdupq_n_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: u64 = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vdupq_n_u64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vdupq_n_u64", "none");
}

#[test]
fn a256_vdups_laneq_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::float32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vdups_laneq_f32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            compare::<0>(p0, case, &mut d);
            compare::<1>(p0, case, &mut d);
            compare::<2>(p0, case, &mut d);
            compare::<3>(p0, case, &mut d);
        }
    }
    d.line("vdups_laneq_f32", "0,1,2,3");
}

#[test]
fn a257_veorq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::veorq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("veorq_s16", "none");
}

#[test]
fn a258_veorq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::veorq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("veorq_s32", "none");
}

#[test]
fn a259_veorq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::veorq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("veorq_s64", "none");
}

#[test]
fn a260_veorq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::veorq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("veorq_s8", "none");
}

#[test]
fn a261_veorq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::veorq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("veorq_u16", "none");
}

#[test]
fn a262_veorq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::veorq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("veorq_u32", "none");
}

#[test]
fn a263_veorq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::veorq_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("veorq_u64", "none");
}

#[test]
fn a264_veorq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::veorq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("veorq_u8", "none");
}

#[test]
fn a265_vextq_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::float32x4_t, p1: real::float32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_f32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_f32", "0,1,2,3");
}

#[test]
fn a266_vextq_u32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint32x4_t, p1: real::uint32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_u32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_u32", "0,1,2,3");
}

#[test]
fn a267_vfmaq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: real::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            let expected = real::vfmaq_f32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vfmaq_f32", "none");
}

#[test]
fn a268_vfmaq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let p2: real::float64x2_t = neon_sample(&mut rng, case, 2, 8);
            let expected = real::vfmaq_f64(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vfmaq_f64", "none");
}

#[test]
fn a269_vfmaq_laneq_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(
        p0: real::float32x4_t,
        p1: real::float32x4_t,
        p2: real::float32x4_t,
        _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vfmaq_laneq_f32::<LANE>(p0, p1, p2);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: real::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            compare::<0>(p0, p1, p2, case, &mut d);
            compare::<1>(p0, p1, p2, case, &mut d);
            compare::<2>(p0, p1, p2, case, &mut d);
            compare::<3>(p0, p1, p2, case, &mut d);
        }
    }
    d.line("vfmaq_laneq_f32", "0,1,2,3");
}

#[test]
fn a270_vfmaq_laneq_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(
        p0: real::float64x2_t,
        p1: real::float64x2_t,
        p2: real::float64x2_t,
        _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vfmaq_laneq_f64::<LANE>(p0, p1, p2);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let p2: real::float64x2_t = neon_sample(&mut rng, case, 2, 8);
            compare::<0>(p0, p1, p2, case, &mut d);
            compare::<1>(p0, p1, p2, case, &mut d);
        }
    }
    d.line("vfmaq_laneq_f64", "0,1");
}

#[test]
fn a271_vfmsq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: real::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            let expected = real::vfmsq_f32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vfmsq_f32", "none");
}

#[test]
fn a272_vget_high_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vget_high_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_high_s16", "none");
}

#[test]
fn a273_vget_high_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vget_high_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_high_s32", "none");
}

#[test]
fn a274_vget_high_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vget_high_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_high_s8", "none");
}

#[test]
fn a275_vget_low_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vget_low_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_low_s32", "none");
}

#[test]
fn a276_vget_low_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vget_low_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vget_low_s8", "none");
}

#[test]
fn a277_vgetq_lane_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const IMM5: i32>(p0: real::float32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&IMM5));
        let expected = real::vgetq_lane_f32::<IMM5>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            compare::<0>(p0, case, &mut d);
            compare::<1>(p0, case, &mut d);
            compare::<2>(p0, case, &mut d);
            compare::<3>(p0, case, &mut d);
        }
    }
    d.line("vgetq_lane_f32", "0,1,2,3");
}

#[test]
fn a278_vgetq_lane_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const IMM5: i32>(p0: real::float64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&IMM5));
        let expected = real::vgetq_lane_f64::<IMM5>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            compare::<0>(p0, case, &mut d);
            compare::<1>(p0, case, &mut d);
        }
    }
    d.line("vgetq_lane_f64", "0,1");
}

#[test]
fn a279_vgetq_lane_s64() {
    let mut d = Digest::new();
    unsafe fn compare<const IMM5: i32>(p0: real::int64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&IMM5));
        let expected = real::vgetq_lane_s64::<IMM5>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            compare::<0>(p0, case, &mut d);
            compare::<1>(p0, case, &mut d);
        }
    }
    d.line("vgetq_lane_s64", "0,1");
}

#[test]
fn a280_vgetq_lane_u64() {
    let mut d = Digest::new();
    unsafe fn compare<const IMM5: i32>(p0: real::uint64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&IMM5));
        let expected = real::vgetq_lane_u64::<IMM5>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            compare::<0>(p0, case, &mut d);
            compare::<1>(p0, case, &mut d);
        }
    }
    d.line("vgetq_lane_u64", "0,1");
}

#[test]
fn a281_vld1_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vld1_s8(p0.0.as_ptr().add(case % 16).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1_s8", "none");
}

#[test]
fn a282_vld1q_dup_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vld1q_dup_f32(p0.0.as_ptr().add((case % 4) * 4).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_dup_f32", "none");
}

#[test]
fn a283_vld1q_dup_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vld1q_dup_f64(p0.0.as_ptr().add((case % 4) * 8).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_dup_f64", "none");
}

#[test]
fn a284_vld1q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vld1q_f32(p0.0.as_ptr().add(case % 16).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_f32", "none");
}

#[test]
fn a285_vld1q_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vld1q_f64(p0.0.as_ptr().add(case % 16).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_f64", "none");
}

#[test]
fn a286_vld1q_lane_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: Buffer, p1: real::float32x4_t, case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected =
            real::vld1q_lane_f32::<LANE>(p0.0.as_ptr().add((case % 4) * 4).cast(), p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vld1q_lane_f32", "0,1,2,3");
}

#[test]
fn a287_vld1q_lane_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: Buffer, p1: real::float64x2_t, case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected =
            real::vld1q_lane_f64::<LANE>(p0.0.as_ptr().add((case % 4) * 8).cast(), p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vld1q_lane_f64", "0,1");
}

#[test]
fn a288_vld1q_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vld1q_s32(p0.0.as_ptr().add(case % 16).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_s32", "none");
}

#[test]
fn a289_vld1q_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vld1q_s8(p0.0.as_ptr().add(case % 16).cast());
            d.add(bytes(&expected));
        }
    }
    d.line("vld1q_s8", "none");
}

#[test]
fn a290_vmaxnmq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vmaxnmq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxnmq_f32", "none");
}

#[test]
fn a291_vmaxnmq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vmaxnmq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxnmq_f64", "none");
}

#[test]
fn a292_vmaxnmvq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vmaxnmvq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxnmvq_f32", "none");
}

#[test]
fn a293_vmaxq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vmaxq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxq_f32", "none");
}

#[test]
fn a294_vmaxq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vmaxq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxq_f64", "none");
}

#[test]
fn a295_vmaxq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmaxq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxq_s8", "none");
}

#[test]
fn a296_vmaxq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmaxq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxq_u16", "none");
}

#[test]
fn a297_vmaxq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmaxq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxq_u32", "none");
}

#[test]
fn a298_vmaxq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmaxq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxq_u8", "none");
}

#[test]
fn a299_vmaxvq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmaxvq_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxvq_s16", "none");
}

#[test]
fn a300_vmaxvq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmaxvq_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxvq_s32", "none");
}

#[test]
fn a301_vmaxvq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmaxvq_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmaxvq_s8", "none");
}

#[test]
fn a302_vminnmq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vminnmq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminnmq_f32", "none");
}

#[test]
fn a303_vminnmq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vminnmq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminnmq_f64", "none");
}

#[test]
fn a304_vminnmvq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vminnmvq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vminnmvq_f32", "none");
}

#[test]
fn a305_vminq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vminq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminq_f32", "none");
}

#[test]
fn a306_vminq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vminq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminq_f64", "none");
}

#[test]
fn a307_vminq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vminq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminq_s32", "none");
}

#[test]
fn a308_vminq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vminq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminq_s8", "none");
}

#[test]
fn a309_vminq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vminq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminq_u16", "none");
}

#[test]
fn a310_vminq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vminq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminq_u32", "none");
}

#[test]
fn a311_vminq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vminq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vminq_u8", "none");
}

#[test]
fn a312_vminvq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vminvq_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vminvq_s16", "none");
}

#[test]
fn a313_vminvq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vminvq_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vminvq_s32", "none");
}

#[test]
fn a314_vminvq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vminvq_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vminvq_s8", "none");
}

#[test]
fn a315_vminvq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vminvq_u16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vminvq_u16", "none");
}

#[test]
fn a316_vminvq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vminvq_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vminvq_u32", "none");
}

#[test]
fn a317_vminvq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vminvq_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vminvq_u8", "none");
}

#[test]
fn a318_vmlaq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let p2: real::int32x4_t = neon_sample(&mut rng, case, 2, 0);
            let expected = real::vmlaq_s32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlaq_s32", "none");
}

#[test]
fn a319_vmlsq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let p2: real::float32x4_t = neon_sample(&mut rng, case, 2, 4);
            let expected = real::vmlsq_f32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlsq_f32", "none");
}

#[test]
fn a320_vmovl_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmovl_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovl_s16", "none");
}

#[test]
fn a321_vmovl_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmovl_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovl_s32", "none");
}

#[test]
fn a322_vmovl_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmovl_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovl_s8", "none");
}

#[test]
fn a323_vmovq_n_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f32 = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vmovq_n_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovq_n_f32", "none");
}

#[test]
fn a324_vmovq_n_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f64 = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vmovq_n_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovq_n_f64", "none");
}

#[test]
fn a325_vmovq_n_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i16 = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmovq_n_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovq_n_s16", "none");
}

#[test]
fn a326_vmovq_n_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i32 = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmovq_n_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovq_n_s32", "none");
}

#[test]
fn a327_vmovq_n_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: i64 = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmovq_n_s64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovq_n_s64", "none");
}

#[test]
fn a328_vmovq_n_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: u32 = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmovq_n_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmovq_n_u32", "none");
}

#[test]
fn a329_vmull_high_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmull_high_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmull_high_s16", "none");
}

#[test]
fn a330_vmull_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmull_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmull_s16", "none");
}

#[test]
fn a331_vmull_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmull_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmull_s32", "none");
}

#[test]
fn a332_vmull_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmull_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmull_u16", "none");
}

#[test]
fn a333_vmull_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmull_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmull_u32", "none");
}

#[test]
fn a334_vmulq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vmulq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmulq_f32", "none");
}

#[test]
fn a335_vmulq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vmulq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmulq_f64", "none");
}

#[test]
fn a336_vmulq_laneq_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: real::float32x4_t, p1: real::float32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmulq_laneq_f32::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vmulq_laneq_f32", "0,1,2,3");
}

#[test]
fn a337_vmulq_n_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: f32 = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vmulq_n_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmulq_n_f32", "none");
}

#[test]
fn a338_vmulq_n_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: f64 = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vmulq_n_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmulq_n_f64", "none");
}

#[test]
fn a339_vmulq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmulq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmulq_u16", "none");
}

#[test]
fn a340_vmulq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vmulq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vmulq_u32", "none");
}

#[test]
fn a341_vmuls_laneq_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: f32, p1: real::float32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmuls_laneq_f32::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f32 = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vmuls_laneq_f32", "0,1,2,3");
}

#[test]
fn a342_vmvnq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vmvnq_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vmvnq_u32", "none");
}

#[test]
fn a343_vnegq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vnegq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vnegq_f32", "none");
}

#[test]
fn a344_vnegq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vnegq_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vnegq_s32", "none");
}

#[test]
fn a345_vorrq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vorrq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorrq_s16", "none");
}

#[test]
fn a346_vorrq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vorrq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorrq_s64", "none");
}

#[test]
fn a347_vorrq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vorrq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorrq_s8", "none");
}

#[test]
fn a348_vorrq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vorrq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorrq_u16", "none");
}

#[test]
fn a349_vorrq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vorrq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorrq_u32", "none");
}

#[test]
fn a350_vorrq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vorrq_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorrq_u64", "none");
}

#[test]
fn a351_vorrq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vorrq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vorrq_u8", "none");
}

#[test]
fn a352_vpaddlq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vpaddlq_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vpaddlq_s16", "none");
}

#[test]
fn a353_vpaddq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vpaddq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    unsafe {
        let a: [f32; 4] = std::hint::black_box([16777216.0, 1.0, -16777216.0, 1.0]);
        let b: [f32; 4] = std::hint::black_box([1.0e30, -1.0e30, -0.0, -0.0]);
        let expected = real::vpaddq_f32(cast(a), cast(b));
        d.add(bytes(&expected));
    }
    d.line("vpaddq_f32", "none");
}

#[test]
fn a354_vpaddq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vpaddq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vpaddq_s32", "none");
}

#[test]
fn a355_vpmaxq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vpmaxq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vpmaxq_u8", "none");
}

#[test]
fn a356_vqaddq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqaddq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqaddq_s16", "none");
}

#[test]
fn a357_vqaddq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqaddq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqaddq_s8", "none");
}

#[test]
fn a358_vqaddq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqaddq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqaddq_u16", "none");
}

#[test]
fn a359_vqaddq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqaddq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqaddq_u8", "none");
}

#[test]
fn a360_vqmovn_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vqmovn_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vqmovn_s16", "none");
}

#[test]
fn a361_vqmovun_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vqmovun_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vqmovun_s16", "none");
}

#[test]
fn a362_vqrdmulhq_n_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: i16 = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqrdmulhq_n_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqrdmulhq_n_s16", "none");
}

#[test]
fn a363_vqrdmulhq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqrdmulhq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqrdmulhq_s16", "none");
}

#[test]
fn a364_vqsubq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqsubq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqsubq_s16", "none");
}

#[test]
fn a365_vqsubq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqsubq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqsubq_s8", "none");
}

#[test]
fn a366_vqsubq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqsubq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqsubq_u16", "none");
}

#[test]
fn a367_vqsubq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqsubq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqsubq_u8", "none");
}

#[test]
fn a368_vqtbl1q_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqtbl1q_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqtbl1q_s8", "none");
}

#[test]
fn a369_vqtbl1q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqtbl1q_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqtbl1q_u8", "none");
}

#[test]
fn a370_vqtbl4q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vqtbl4q_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vqtbl4q_u8", "none");
}

#[test]
fn a371_vreinterpretq_f32_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_f32_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_f32_s32", "none");
}

#[test]
fn a372_vreinterpretq_f32_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_f32_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_f32_u32", "none");
}

#[test]
fn a373_vreinterpretq_f32_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_f32_u64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_f32_u64", "none");
}

#[test]
fn a374_vreinterpretq_f32_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_f32_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_f32_u8", "none");
}

#[test]
fn a375_vreinterpretq_f64_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_f64_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_f64_u32", "none");
}

#[test]
fn a376_vreinterpretq_f64_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_f64_u64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_f64_u64", "none");
}

#[test]
fn a377_vreinterpretq_s16_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_s16_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s16_s32", "none");
}

#[test]
fn a378_vreinterpretq_s16_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_s16_u16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s16_u16", "none");
}

#[test]
fn a379_vreinterpretq_s32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vreinterpretq_s32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s32_f32", "none");
}

#[test]
fn a380_vreinterpretq_s32_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_s32_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s32_s16", "none");
}

#[test]
fn a381_vreinterpretq_s64_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_s64_u64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s64_u64", "none");
}

#[test]
fn a382_vreinterpretq_s8_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_s8_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_s8_u8", "none");
}

#[test]
fn a383_vreinterpretq_u16_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_u16_s16(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u16_s16", "none");
}

#[test]
fn a384_vreinterpretq_u16_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_u16_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u16_s32", "none");
}

#[test]
fn a385_vreinterpretq_u16_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_u16_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u16_u32", "none");
}

#[test]
fn a386_vreinterpretq_u16_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_u16_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u16_u8", "none");
}

#[test]
fn a387_vreinterpretq_u32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vreinterpretq_u32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u32_f32", "none");
}

#[test]
fn a388_vreinterpretq_u32_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vreinterpretq_u32_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u32_f64", "none");
}

#[test]
fn a389_vreinterpretq_u64_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vreinterpretq_u64_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u64_f32", "none");
}

#[test]
fn a390_vreinterpretq_u64_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vreinterpretq_u64_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u64_f64", "none");
}

#[test]
fn a391_vreinterpretq_u64_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_u64_s64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u64_s64", "none");
}

#[test]
fn a392_vreinterpretq_u64_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_u64_u8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u64_u8", "none");
}

#[test]
fn a393_vreinterpretq_u8_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_u8_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u8_s32", "none");
}

#[test]
fn a394_vreinterpretq_u8_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let expected = real::vreinterpretq_u8_s8(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vreinterpretq_u8_s8", "none");
}

#[test]
fn a395_vrev64q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vrev64q_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrev64q_f32", "none");
}

#[test]
fn a396_vrndmq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vrndmq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndmq_f32", "none");
}

#[test]
fn a397_vrndmq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vrndmq_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndmq_f64", "none");
}

#[test]
fn a398_vrndnq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vrndnq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndnq_f32", "none");
}

#[test]
fn a399_vrndpq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vrndpq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndpq_f32", "none");
}

#[test]
fn a400_vrndpq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vrndpq_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndpq_f64", "none");
}

#[test]
fn a401_vrndq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vrndq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndq_f32", "none");
}

#[test]
fn a402_vsetq_lane_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: f32, p1: real::float32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vsetq_lane_f32::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: f32 = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vsetq_lane_f32", "0,1,2,3");
}

#[test]
fn a403_vsetq_lane_u64() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: u64, p1: real::uint64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vsetq_lane_u64::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: u64 = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vsetq_lane_u64", "0,1");
}

#[test]
fn a404_vshlq_n_s32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vshlq_n_s32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            compare::<0>(p0, case, &mut d);
            compare::<1>(p0, case, &mut d);
            compare::<2>(p0, case, &mut d);
            compare::<3>(p0, case, &mut d);
            compare::<4>(p0, case, &mut d);
            compare::<5>(p0, case, &mut d);
            compare::<6>(p0, case, &mut d);
            compare::<7>(p0, case, &mut d);
            compare::<8>(p0, case, &mut d);
            compare::<9>(p0, case, &mut d);
            compare::<10>(p0, case, &mut d);
            compare::<11>(p0, case, &mut d);
            compare::<12>(p0, case, &mut d);
            compare::<13>(p0, case, &mut d);
            compare::<14>(p0, case, &mut d);
            compare::<15>(p0, case, &mut d);
            compare::<16>(p0, case, &mut d);
            compare::<17>(p0, case, &mut d);
            compare::<18>(p0, case, &mut d);
            compare::<19>(p0, case, &mut d);
            compare::<20>(p0, case, &mut d);
            compare::<21>(p0, case, &mut d);
            compare::<22>(p0, case, &mut d);
            compare::<23>(p0, case, &mut d);
            compare::<24>(p0, case, &mut d);
            compare::<25>(p0, case, &mut d);
            compare::<26>(p0, case, &mut d);
            compare::<27>(p0, case, &mut d);
            compare::<28>(p0, case, &mut d);
            compare::<29>(p0, case, &mut d);
            compare::<30>(p0, case, &mut d);
            compare::<31>(p0, case, &mut d);
        }
    }
    d.line("vshlq_n_s32", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31");
}

#[test]
fn a405_vshlq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vshlq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vshlq_s16", "none");
}

#[test]
fn a406_vshlq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vshlq_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vshlq_s32", "none");
}

#[test]
fn a407_vshlq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vshlq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vshlq_s64", "none");
}

#[test]
fn a408_vshlq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vshlq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vshlq_u16", "none");
}

#[test]
fn a409_vshlq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vshlq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vshlq_u32", "none");
}

#[test]
fn a410_vshlq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vshlq_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vshlq_u64", "none");
}

#[test]
fn a411_vshrn_n_u16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint16x8_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vshrn_n_u16::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            compare::<1>(p0, case, &mut d);
            compare::<2>(p0, case, &mut d);
            compare::<3>(p0, case, &mut d);
            compare::<4>(p0, case, &mut d);
            compare::<5>(p0, case, &mut d);
            compare::<6>(p0, case, &mut d);
            compare::<7>(p0, case, &mut d);
            compare::<8>(p0, case, &mut d);
        }
    }
    d.line("vshrn_n_u16", "1,2,3,4,5,6,7,8");
}

#[test]
fn a412_vshrn_n_u64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vshrn_n_u64::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            compare::<1>(p0, case, &mut d);
            compare::<2>(p0, case, &mut d);
            compare::<3>(p0, case, &mut d);
            compare::<4>(p0, case, &mut d);
            compare::<5>(p0, case, &mut d);
            compare::<6>(p0, case, &mut d);
            compare::<7>(p0, case, &mut d);
            compare::<8>(p0, case, &mut d);
            compare::<9>(p0, case, &mut d);
            compare::<10>(p0, case, &mut d);
            compare::<11>(p0, case, &mut d);
            compare::<12>(p0, case, &mut d);
            compare::<13>(p0, case, &mut d);
            compare::<14>(p0, case, &mut d);
            compare::<15>(p0, case, &mut d);
            compare::<16>(p0, case, &mut d);
            compare::<17>(p0, case, &mut d);
            compare::<18>(p0, case, &mut d);
            compare::<19>(p0, case, &mut d);
            compare::<20>(p0, case, &mut d);
            compare::<21>(p0, case, &mut d);
            compare::<22>(p0, case, &mut d);
            compare::<23>(p0, case, &mut d);
            compare::<24>(p0, case, &mut d);
            compare::<25>(p0, case, &mut d);
            compare::<26>(p0, case, &mut d);
            compare::<27>(p0, case, &mut d);
            compare::<28>(p0, case, &mut d);
            compare::<29>(p0, case, &mut d);
            compare::<30>(p0, case, &mut d);
            compare::<31>(p0, case, &mut d);
            compare::<32>(p0, case, &mut d);
        }
    }
    d.line("vshrn_n_u64", "1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32");
}

#[test]
fn a413_vshrq_n_u32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vshrq_n_u32::<N>(p0);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            compare::<1>(p0, case, &mut d);
            compare::<2>(p0, case, &mut d);
            compare::<3>(p0, case, &mut d);
            compare::<4>(p0, case, &mut d);
            compare::<5>(p0, case, &mut d);
            compare::<6>(p0, case, &mut d);
            compare::<7>(p0, case, &mut d);
            compare::<8>(p0, case, &mut d);
            compare::<9>(p0, case, &mut d);
            compare::<10>(p0, case, &mut d);
            compare::<11>(p0, case, &mut d);
            compare::<12>(p0, case, &mut d);
            compare::<13>(p0, case, &mut d);
            compare::<14>(p0, case, &mut d);
            compare::<15>(p0, case, &mut d);
            compare::<16>(p0, case, &mut d);
            compare::<17>(p0, case, &mut d);
            compare::<18>(p0, case, &mut d);
            compare::<19>(p0, case, &mut d);
            compare::<20>(p0, case, &mut d);
            compare::<21>(p0, case, &mut d);
            compare::<22>(p0, case, &mut d);
            compare::<23>(p0, case, &mut d);
            compare::<24>(p0, case, &mut d);
            compare::<25>(p0, case, &mut d);
            compare::<26>(p0, case, &mut d);
            compare::<27>(p0, case, &mut d);
            compare::<28>(p0, case, &mut d);
            compare::<29>(p0, case, &mut d);
            compare::<30>(p0, case, &mut d);
            compare::<31>(p0, case, &mut d);
            compare::<32>(p0, case, &mut d);
        }
    }
    d.line("vshrq_n_u32", "1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32");
}

#[test]
fn a414_vsqrtq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vsqrtq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vsqrtq_f32", "none");
}

#[test]
fn a415_vsqrtq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vsqrtq_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vsqrtq_f64", "none");
}

#[test]
fn a416_vst1q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vst1q_f32(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            d.add(bytes(&expected));
            d.add(&p0.0);
        }
    }
    d.line("vst1q_f32", "none");
}

#[test]
fn a417_vst1q_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vst1q_f64(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            d.add(bytes(&expected));
            d.add(&p0.0);
        }
    }
    d.line("vst1q_f64", "none");
}

#[test]
fn a418_vst1q_lane_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(mut p0: Buffer, p1: real::float32x4_t, case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected =
            real::vst1q_lane_f32::<LANE>(p0.0.as_mut_ptr().add((case % 4) * 4).cast(), p1);
        d.add(bytes(&expected));
        d.add(&p0.0);
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vst1q_lane_f32", "0,1,2,3");
}

#[test]
fn a419_vst1q_lane_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(mut p0: Buffer, p1: real::float64x2_t, case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected =
            real::vst1q_lane_f64::<LANE>(p0.0.as_mut_ptr().add((case % 4) * 8).cast(), p1);
        d.add(bytes(&expected));
        d.add(&p0.0);
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: Buffer = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vst1q_lane_f64", "0,1");
}

#[test]
fn a420_vst1q_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vst1q_s32(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            d.add(bytes(&expected));
            d.add(&p0.0);
        }
    }
    d.line("vst1q_s32", "none");
}

#[test]
fn a421_vst1q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vst1q_u8(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            d.add(bytes(&expected));
            d.add(&p0.0);
        }
    }
    d.line("vst1q_u8", "none");
}

#[test]
fn a422_vst3q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let mut p0: Buffer = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4x3_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vst3q_f32(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            d.add(bytes(&expected));
            d.add(&p0.0);
        }
    }
    d.line("vst3q_f32", "none");
}

#[test]
fn a423_vsubq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vsubq_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_f32", "none");
}

#[test]
fn a424_vsubq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vsubq_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_f64", "none");
}

#[test]
fn a425_vsubq_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vsubq_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_s64", "none");
}

#[test]
fn a426_vsubq_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vsubq_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_s8", "none");
}

#[test]
fn a427_vsubq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vsubq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_u16", "none");
}

#[test]
fn a428_vsubq_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x4_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vsubq_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_u32", "none");
}

#[test]
fn a429_vsubq_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vsubq_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_u64", "none");
}

#[test]
fn a430_vsubq_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vsubq_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vsubq_u8", "none");
}

#[test]
fn a431_vtrn1q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vtrn1q_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_f32", "none");
}

#[test]
fn a432_vtrn2q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vtrn2q_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_f32", "none");
}

#[test]
fn a433_vtrnq_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vtrnq_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrnq_s16", "none");
}

#[test]
fn a434_vuzp1q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vuzp1q_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vuzp1q_f32", "none");
}

#[test]
fn a435_vuzp2q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vuzp2q_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vuzp2q_f32", "none");
}

#[test]
fn a436_vuzpq_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vuzpq_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vuzpq_u16", "none");
}

#[test]
fn a437_vzip1q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vzip1q_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vzip1q_f32", "none");
}

#[test]
fn a438_vzip2q_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            let expected = real::vzip2q_f32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vzip2q_f32", "none");
}

#[test]
fn a439_vzip2q_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vzip2q_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vzip2q_u64", "none");
}

#[test]
fn a440_vzip_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint8x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x8_t = neon_sample(&mut rng, case, 1, 0);
            let expected = real::vzip_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vzip_u8", "none");
}

#[test]
fn a441_vext_s8() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int8x8_t, p1: real::int8x8_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_s8::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::int8x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x8_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_s8", "0,1,2,3,4,5,6,7");
}

#[test]
fn a442_vextq_s8() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int8x16_t, p1: real::int8x16_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_s8::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::int8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int8x16_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
            compare::<8>(p0, p1, case, &mut d);
            compare::<9>(p0, p1, case, &mut d);
            compare::<10>(p0, p1, case, &mut d);
            compare::<11>(p0, p1, case, &mut d);
            compare::<12>(p0, p1, case, &mut d);
            compare::<13>(p0, p1, case, &mut d);
            compare::<14>(p0, p1, case, &mut d);
            compare::<15>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_s8", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15");
}

#[test]
fn a443_vext_u8() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint8x8_t, p1: real::uint8x8_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_u8::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::uint8x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x8_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_u8", "0,1,2,3,4,5,6,7");
}

#[test]
fn a444_vextq_u8() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint8x16_t, p1: real::uint8x16_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_u8::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::uint8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint8x16_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
            compare::<8>(p0, p1, case, &mut d);
            compare::<9>(p0, p1, case, &mut d);
            compare::<10>(p0, p1, case, &mut d);
            compare::<11>(p0, p1, case, &mut d);
            compare::<12>(p0, p1, case, &mut d);
            compare::<13>(p0, p1, case, &mut d);
            compare::<14>(p0, p1, case, &mut d);
            compare::<15>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_u8", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15");
}

#[test]
fn a445_vext_p8() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::poly8x8_t, p1: real::poly8x8_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_p8::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::poly8x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::poly8x8_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_p8", "0,1,2,3,4,5,6,7");
}

#[test]
fn a446_vextq_p8() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::poly8x16_t, p1: real::poly8x16_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_p8::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::poly8x16_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::poly8x16_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
            compare::<8>(p0, p1, case, &mut d);
            compare::<9>(p0, p1, case, &mut d);
            compare::<10>(p0, p1, case, &mut d);
            compare::<11>(p0, p1, case, &mut d);
            compare::<12>(p0, p1, case, &mut d);
            compare::<13>(p0, p1, case, &mut d);
            compare::<14>(p0, p1, case, &mut d);
            compare::<15>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_p8", "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15");
}

#[test]
fn a447_vext_s16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int16x4_t, p1: real::int16x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_s16::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::int16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x4_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_s16", "0,1,2,3");
}

#[test]
fn a448_vextq_s16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int16x8_t, p1: real::int16x8_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_s16::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::int16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int16x8_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_s16", "0,1,2,3,4,5,6,7");
}

#[test]
fn a449_vext_u16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint16x4_t, p1: real::uint16x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_u16::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::uint16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x4_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_u16", "0,1,2,3");
}

#[test]
fn a450_vextq_u16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint16x8_t, p1: real::uint16x8_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_u16::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::uint16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint16x8_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_u16", "0,1,2,3,4,5,6,7");
}

#[test]
fn a451_vext_p16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::poly16x4_t, p1: real::poly16x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_p16::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::poly16x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::poly16x4_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_p16", "0,1,2,3");
}

#[test]
fn a452_vextq_p16() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::poly16x8_t, p1: real::poly16x8_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_p16::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::poly16x8_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::poly16x8_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
            compare::<4>(p0, p1, case, &mut d);
            compare::<5>(p0, p1, case, &mut d);
            compare::<6>(p0, p1, case, &mut d);
            compare::<7>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_p16", "0,1,2,3,4,5,6,7");
}

#[test]
fn a453_vext_s32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int32x2_t, p1: real::int32x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_s32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::int32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_s32", "0,1");
}

#[test]
fn a454_vextq_s32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int32x4_t, p1: real::int32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_s32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::int32x4_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int32x4_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_s32", "0,1,2,3");
}

#[test]
fn a455_vext_u32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint32x2_t, p1: real::uint32x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_u32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::uint32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint32x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_u32", "0,1");
}

#[test]
fn a456_vext_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::float32x2_t, p1: real::float32x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_f32::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::float32x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_f32", "0,1");
}

#[test]
fn a457_vext_s64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int64x1_t, p1: real::int64x1_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_s64::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::int64x1_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x1_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_s64", "0");
}

#[test]
fn a458_vextq_s64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::int64x2_t, p1: real::int64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_s64::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::int64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::int64x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_s64", "0,1");
}

#[test]
fn a459_vext_u64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint64x1_t, p1: real::uint64x1_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_u64::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::uint64x1_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x1_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_u64", "0");
}

#[test]
fn a460_vextq_u64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::uint64x2_t, p1: real::uint64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_u64::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::uint64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::uint64x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_u64", "0,1");
}

#[test]
fn a461_vext_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::float64x1_t, p1: real::float64x1_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_f64::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::float64x1_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::float64x1_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_f64", "0");
}

#[test]
fn a462_vextq_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::float64x2_t, p1: real::float64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_f64::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_f64", "0,1");
}

#[test]
fn a463_vext_p64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::poly64x1_t, p1: real::poly64x1_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vext_p64::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::poly64x1_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::poly64x1_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
        }
    }
    d.line("vext_p64", "0");
}

#[test]
fn a464_vextq_p64() {
    let mut d = Digest::new();
    unsafe fn compare<const N: i32>(p0: real::poly64x2_t, p1: real::poly64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&N));
        let expected = real::vextq_p64::<N>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 {
            let p0: real::poly64x2_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::poly64x2_t = neon_sample(&mut rng, case, 1, 0);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vextq_p64", "0,1");
}

#[test]
fn a465_sha256_scalar_reference() {
    let mut d = Digest::new();
    assert!(std::arch::is_aarch64_feature_detected!("sha2"), "SHA256 coverage requires sha2");
    let mut rng = Rng(0x735a2d97e6481bcf);
    unsafe {
        for case in 0..RANDOM_CASES + 17 {
            let a: [u32; 4] = sample(&mut rng, case, 0);
            let b: [u32; 4] = sample(&mut rng, case, 1);
            let c: [u32; 4] = sample(&mut rng, case, 2);
            let got: [u32; 4] = cast(real::vsha256su0q_u32(cast(a), cast(b)));
            d.add(bytes(&got));
            for (x, y, z) in [(a, b, c), (a, a, c), (a, b, a), (a, b, b), (a, a, a)] {
                let got: [u32; 4] = cast(real::vsha256hq_u32(cast(x), cast(y), cast(z)));
                d.add(bytes(&got));
                let got: [u32; 4] = cast(real::vsha256h2q_u32(cast(x), cast(y), cast(z)));
                d.add(bytes(&got));
                let got: [u32; 4] = cast(real::vsha256su1q_u32(cast(x), cast(y), cast(z)));
                d.add(bytes(&got));
            }
        }
    }
    d.line("sha256_scalar_reference", "none");
}

#[test]
fn a466_vtrn1q_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vtrn1q_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_f64", "none");
}

#[test]
fn a467_vtrn2q_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            let expected = real::vtrn2q_f64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_f64", "none");
}

#[test]
fn a468_vtrn1q_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint64x2_t = sample(&mut rng, case, 0);
            let p1: real::uint64x2_t = sample(&mut rng, case, 1);
            let expected = real::vtrn1q_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_u64", "none");
}

#[test]
fn a469_vtrn2q_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint64x2_t = sample(&mut rng, case, 0);
            let p1: real::uint64x2_t = sample(&mut rng, case, 1);
            let expected = real::vtrn2q_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_u64", "none");
}

#[test]
fn a470_vuzp1q_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int64x2_t = sample(&mut rng, case, 0);
            let p1: real::int64x2_t = sample(&mut rng, case, 1);
            let expected = real::vuzp1q_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vuzp1q_s64", "none");
}

#[test]
fn a471_vuzp2q_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int64x2_t = sample(&mut rng, case, 0);
            let p1: real::int64x2_t = sample(&mut rng, case, 1);
            let expected = real::vuzp2q_s64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vuzp2q_s64", "none");
}

#[test]
fn a472_vuzp1q_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint64x2_t = sample(&mut rng, case, 0);
            let p1: real::uint64x2_t = sample(&mut rng, case, 1);
            let expected = real::vuzp1q_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vuzp1q_u64", "none");
}

#[test]
fn a473_vuzp2q_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint64x2_t = sample(&mut rng, case, 0);
            let p1: real::uint64x2_t = sample(&mut rng, case, 1);
            let expected = real::vuzp2q_u64(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vuzp2q_u64", "none");
}

#[test]
fn a474_vtrn1q_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int8x16_t = sample(&mut rng, case, 0);
            let p1: real::int8x16_t = sample(&mut rng, case, 1);
            let expected = real::vtrn1q_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_s8", "none");
}

#[test]
fn a475_vtrn2q_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int8x16_t = sample(&mut rng, case, 0);
            let p1: real::int8x16_t = sample(&mut rng, case, 1);
            let expected = real::vtrn2q_s8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_s8", "none");
}

#[test]
fn a476_vtrn1q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x16_t = sample(&mut rng, case, 0);
            let p1: real::uint8x16_t = sample(&mut rng, case, 1);
            let expected = real::vtrn1q_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_u8", "none");
}

#[test]
fn a477_vtrn2q_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint8x16_t = sample(&mut rng, case, 0);
            let p1: real::uint8x16_t = sample(&mut rng, case, 1);
            let expected = real::vtrn2q_u8(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_u8", "none");
}

#[test]
fn a478_vtrn1q_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vtrn1q_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_s16", "none");
}

#[test]
fn a479_vtrn2q_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int16x8_t = sample(&mut rng, case, 1);
            let expected = real::vtrn2q_s16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_s16", "none");
}

#[test]
fn a480_vtrn1q_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let p1: real::uint16x8_t = sample(&mut rng, case, 1);
            let expected = real::vtrn1q_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_u16", "none");
}

#[test]
fn a481_vtrn2q_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let p1: real::uint16x8_t = sample(&mut rng, case, 1);
            let expected = real::vtrn2q_u16(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_u16", "none");
}

#[test]
fn a482_vtrn1q_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int32x4_t = sample(&mut rng, case, 1);
            let expected = real::vtrn1q_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_s32", "none");
}

#[test]
fn a483_vtrn2q_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int32x4_t = sample(&mut rng, case, 1);
            let expected = real::vtrn2q_s32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_s32", "none");
}

#[test]
fn a484_vtrn1q_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint32x4_t = sample(&mut rng, case, 1);
            let expected = real::vtrn1q_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn1q_u32", "none");
}

#[test]
fn a485_vtrn2q_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint32x4_t = sample(&mut rng, case, 1);
            let expected = real::vtrn2q_u32(p0, p1);
            d.add(bytes(&expected));
        }
    }
    d.line("vtrn2q_u32", "none");
}

#[test]
fn a486_vmulq_laneq_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: real::float64x2_t, p1: real::float64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmulq_laneq_f64::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vmulq_laneq_f64", "0,1");
}

#[test]
fn a487_vmul_lane_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: real::float32x2_t, p1: real::float32x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmul_lane_f32::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x2_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vmul_lane_f32", "0,1");
}

#[test]
fn a488_vmul_laneq_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: real::float32x2_t, p1: real::float32x4_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmul_laneq_f32::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x4_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
            compare::<2>(p0, p1, case, &mut d);
            compare::<3>(p0, p1, case, &mut d);
        }
    }
    d.line("vmul_laneq_f32", "0,1,2,3");
}

#[test]
fn a489_vmulq_lane_f32() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: real::float32x4_t, p1: real::float32x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmulq_lane_f32::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x2_t = neon_sample(&mut rng, case, 1, 4);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vmulq_lane_f32", "0,1");
}

#[test]
fn a490_vmul_lane_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: real::float64x1_t, p1: real::float64x1_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmul_lane_f64::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x1_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x1_t = neon_sample(&mut rng, case, 1, 8);
            compare::<0>(p0, p1, case, &mut d);
        }
    }
    d.line("vmul_lane_f64", "0");
}

#[test]
fn a491_vmul_laneq_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: real::float64x1_t, p1: real::float64x2_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmul_laneq_f64::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x1_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x2_t = neon_sample(&mut rng, case, 1, 8);
            compare::<0>(p0, p1, case, &mut d);
            compare::<1>(p0, p1, case, &mut d);
        }
    }
    d.line("vmul_laneq_f64", "0,1");
}

#[test]
fn a492_vmulq_lane_f64() {
    let mut d = Digest::new();
    unsafe fn compare<const LANE: i32>(p0: real::float64x2_t, p1: real::float64x1_t, _case: usize, d: &mut Digest) {
        d.add(bytes(&LANE));
        let expected = real::vmulq_lane_f64::<LANE>(p0, p1);
        d.add(bytes(&expected));
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let p1: real::float64x1_t = neon_sample(&mut rng, case, 1, 8);
            compare::<0>(p0, p1, case, &mut d);
        }
    }
    d.line("vmulq_lane_f64", "0");
}

#[test]
fn a493_vmlal_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int8x8_t = sample(&mut rng, case, 1);
            let p2: real::int8x8_t = sample(&mut rng, case, 2);
            let expected = real::vmlal_s8(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlal_s8", "none");
}

#[test]
fn a494_vmlsl_s8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int16x8_t = sample(&mut rng, case, 0);
            let p1: real::int8x8_t = sample(&mut rng, case, 1);
            let p2: real::int8x8_t = sample(&mut rng, case, 2);
            let expected = real::vmlsl_s8(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlsl_s8", "none");
}

#[test]
fn a495_vmlsl_s16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int16x4_t = sample(&mut rng, case, 1);
            let p2: real::int16x4_t = sample(&mut rng, case, 2);
            let expected = real::vmlsl_s16(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlsl_s16", "none");
}

#[test]
fn a496_vmlal_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int64x2_t = sample(&mut rng, case, 0);
            let p1: real::int32x2_t = sample(&mut rng, case, 1);
            let p2: real::int32x2_t = sample(&mut rng, case, 2);
            let expected = real::vmlal_s32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlal_s32", "none");
}

#[test]
fn a497_vmlsl_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int64x2_t = sample(&mut rng, case, 0);
            let p1: real::int32x2_t = sample(&mut rng, case, 1);
            let p2: real::int32x2_t = sample(&mut rng, case, 2);
            let expected = real::vmlsl_s32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlsl_s32", "none");
}

#[test]
fn a498_vmlal_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let p1: real::uint8x8_t = sample(&mut rng, case, 1);
            let p2: real::uint8x8_t = sample(&mut rng, case, 2);
            let expected = real::vmlal_u8(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlal_u8", "none");
}

#[test]
fn a499_vmlsl_u8() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint16x8_t = sample(&mut rng, case, 0);
            let p1: real::uint8x8_t = sample(&mut rng, case, 1);
            let p2: real::uint8x8_t = sample(&mut rng, case, 2);
            let expected = real::vmlsl_u8(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlsl_u8", "none");
}

#[test]
fn a500_vmlsl_u16() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x4_t = sample(&mut rng, case, 0);
            let p1: real::uint16x4_t = sample(&mut rng, case, 1);
            let p2: real::uint16x4_t = sample(&mut rng, case, 2);
            let expected = real::vmlsl_u16(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlsl_u16", "none");
}

#[test]
fn a501_vmlsl_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint64x2_t = sample(&mut rng, case, 0);
            let p1: real::uint32x2_t = sample(&mut rng, case, 1);
            let p2: real::uint32x2_t = sample(&mut rng, case, 2);
            let expected = real::vmlsl_u32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vmlsl_u32", "none");
}

#[test]
fn a502_vqmovun_s64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int64x2_t = sample(&mut rng, case, 0);
            let expected = real::vqmovun_s64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vqmovun_s64", "none");
}

#[test]
fn a503_vcvta_s32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vcvta_s32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvta_s32_f32", "none");
}

#[test]
fn a504_vcvtaq_s32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vcvtaq_s32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtaq_s32_f32", "none");
}

#[test]
fn a505_vcvta_u32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vcvta_u32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvta_u32_f32", "none");
}

#[test]
fn a506_vcvta_s64_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x1_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vcvta_s64_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvta_s64_f64", "none");
}

#[test]
fn a507_vcvtaq_s64_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vcvtaq_s64_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtaq_s64_f64", "none");
}

#[test]
fn a508_vcvta_u64_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x1_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vcvta_u64_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvta_u64_f64", "none");
}

#[test]
fn a509_vcvtaq_u64_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vcvtaq_u64_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtaq_u64_f64", "none");
}

#[test]
fn a510_vqrdmlah_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("rdm"),
            "required native rdm coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x2_t = sample(&mut rng, case, 0);
            let p1: real::int32x2_t = sample(&mut rng, case, 1);
            let p2: real::int32x2_t = sample(&mut rng, case, 2);
            let expected = real::vqrdmlah_s32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vqrdmlah_s32", "none");
}

#[test]
fn a511_vqrdmlsh_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("rdm"),
            "required native rdm coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x2_t = sample(&mut rng, case, 0);
            let p1: real::int32x2_t = sample(&mut rng, case, 1);
            let p2: real::int32x2_t = sample(&mut rng, case, 2);
            let expected = real::vqrdmlsh_s32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vqrdmlsh_s32", "none");
}

#[test]
fn a512_vqrdmlshq_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("rdm"),
            "required native rdm coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x4_t = sample(&mut rng, case, 0);
            let p1: real::int32x4_t = sample(&mut rng, case, 1);
            let p2: real::int32x4_t = sample(&mut rng, case, 2);
            let expected = real::vqrdmlshq_s32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vqrdmlshq_s32", "none");
}

#[test]
fn a513_vbsl_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::uint64x1_t = neon_sample(&mut rng, case, 0, 0);
            let p1: real::float64x1_t = neon_sample(&mut rng, case, 1, 8);
            let p2: real::float64x1_t = neon_sample(&mut rng, case, 2, 8);
            let expected = real::vbsl_f64(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vbsl_f64", "none");
}

#[test]
fn a514_vcvt_f32_s32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::int32x2_t = sample(&mut rng, case, 0);
            let expected = real::vcvt_f32_s32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvt_f32_s32", "none");
}

#[test]
fn a515_vcvt_f32_u32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint32x2_t = sample(&mut rng, case, 0);
            let expected = real::vcvt_f32_u32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvt_f32_u32", "none");
}

#[test]
fn a516_vcvtq_f64_u64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 17 {
            let p0: real::uint64x2_t = sample(&mut rng, case, 0);
            let expected = real::vcvtq_f64_u64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtq_f64_u64", "none");
}

#[test]
fn a517_vcvt_s32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vcvt_s32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvt_s32_f32", "none");
}

#[test]
fn a518_vcvt_u32_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vcvt_u32_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvt_u32_f32", "none");
}

#[test]
fn a519_vcvtq_u64_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vcvtq_u64_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vcvtq_u64_f64", "none");
}

#[test]
fn a520_vfma_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x2_t = neon_sample(&mut rng, case, 1, 4);
            let p2: real::float32x2_t = neon_sample(&mut rng, case, 2, 4);
            let expected = real::vfma_f32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vfma_f32", "none");
}

#[test]
fn a521_vfms_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x2_t = neon_sample(&mut rng, case, 0, 4);
            let p1: real::float32x2_t = neon_sample(&mut rng, case, 1, 4);
            let p2: real::float32x2_t = neon_sample(&mut rng, case, 2, 4);
            let expected = real::vfms_f32(p0, p1, p2);
            d.add(bytes(&expected));
        }
    }
    d.line("vfms_f32", "none");
}

#[test]
fn a522_vrndiq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vrndiq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndiq_f32", "none");
}

#[test]
fn a523_vrndxq_f32() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float32x4_t = neon_sample(&mut rng, case, 0, 4);
            let expected = real::vrndxq_f32(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndxq_f32", "none");
}

#[test]
fn a524_vrndiq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vrndiq_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndiq_f64", "none");
}

#[test]
fn a525_vrndxq_f64() {
    let mut d = Digest::new();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        assert!(
            std::arch::is_aarch64_feature_detected!("neon"),
            "required native neon coverage unavailable"
        );
        for case in 0..RANDOM_CASES + 64 + 24 * 24 * 24 {
            let p0: real::float64x2_t = neon_sample(&mut rng, case, 0, 8);
            let expected = real::vrndxq_f64(p0);
            d.add(bytes(&expected));
        }
    }
    d.line("vrndxq_f64", "none");
}
