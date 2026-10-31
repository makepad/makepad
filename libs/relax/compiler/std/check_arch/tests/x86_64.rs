#![cfg(target_arch = "x86_64")]
#![allow(non_snake_case)]
#![cfg_attr(feature = "check-unstable-shuffle", feature(stdarch_x86_mm_shuffle))]
include!("common/mod.rs");
include!("common/differential.rs");
use core::arch::x86_64 as real;
use relax_std_arch_check::hr_x86 as hr;

#[path = "common/intel.rs"]
mod intel;

#[test]
fn _MM_SHUFFLE() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_MM_SHUFFLE();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_MM_SHUFFLE: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: u32 = sample(&mut rng, case, 0);
            let p1: u32 = sample(&mut rng, case, 1);
            let p2: u32 = sample(&mut rng, case, 2);
            let p3: u32 = sample(&mut rng, case, 3);
            let got = hr::_MM_SHUFFLE(p0, p1, p2, p3);
            #[cfg(feature = "check-unstable-shuffle")]
            let expected = real::_MM_SHUFFLE(p0, p1, p2, p3);
            // The helper is unstable; this is its documented scalar definition on stable.
            #[cfg(not(feature = "check-unstable-shuffle"))]
            let expected = ((p0 << 6) | (p1 << 4) | (p2 << 2) | p3) as i32;
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_add_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_add_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_add_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_add_epi16(p0, p1);
            let expected = real::_mm256_add_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_add_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_add_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_add_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_add_epi32(p0, p1);
            let expected = real::_mm256_add_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_blend_epi32() {
    unsafe fn compare<const N: i32>((p0, p1, case): (hr::__m256i, hr::__m256i, usize)) {
        let got = hr::_mm256_blend_epi32::<N>(p0, p1);
        let expected = real::_mm256_blend_epi32::<N>(cast(p0), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_blend_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_blend_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            immediate_cases!(
                compare,
                (p0, p1, case),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
                    42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
                    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
                    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
                    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132,
                    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148,
                    149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164,
                    165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180,
                    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196,
                    197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212,
                    213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
                    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244,
                    245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
                ]
            );
        }
    }
}

#[test]
fn _mm256_blendv_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_blendv_epi8();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_blendv_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let p2: hr::__m256i = sample(&mut rng, case, 2);
            let got = hr::_mm256_blendv_epi8(p0, p1, p2);
            let expected = real::_mm256_blendv_epi8(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_castsi256_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_castsi256_si128();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_castsi256_si128: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let got = hr::_mm256_castsi256_si128(p0);
            let expected = real::_mm256_castsi256_si128(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_extractf128_si256() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m256i, usize)) {
        let got = hr::_mm256_extractf128_si256::<N>(p0);
        let expected = real::_mm256_extractf128_si256::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_extractf128_si256();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_extractf128_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case), [0, 1]);
        }
    }
}

#[test]
fn _mm256_extracti128_si256() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m256i, usize)) {
        let got = hr::_mm256_extracti128_si256::<N>(p0);
        let expected = real::_mm256_extracti128_si256::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_extracti128_si256();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_extracti128_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case), [0, 1]);
        }
    }
}

#[test]
fn _mm256_loadu_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_loadu_si256();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_loadu_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::_mm256_loadu_si256(p0.0.as_ptr().add(case % 16).cast());
            let expected = real::_mm256_loadu_si256(q0.0.as_ptr().add(case % 16).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_madd_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_madd_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_madd_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_madd_epi16(p0, p1);
            let expected = real::_mm256_madd_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_maddubs_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_maddubs_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_maddubs_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_maddubs_epi16(p0, p1);
            let expected = real::_mm256_maddubs_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_max_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_max_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_max_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_max_epi16(p0, p1);
            let expected = real::_mm256_max_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_min_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_min_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_min_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_min_epi16(p0, p1);
            let expected = real::_mm256_min_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_mullo_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_mullo_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_mullo_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_mullo_epi16(p0, p1);
            let expected = real::_mm256_mullo_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_mullo_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_mullo_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_mullo_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_mullo_epi32(p0, p1);
            let expected = real::_mm256_mullo_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_or_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_or_si256();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_or_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_or_si256(p0, p1);
            let expected = real::_mm256_or_si256(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_packs_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_packs_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_packs_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_packs_epi32(p0, p1);
            let expected = real::_mm256_packs_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_packus_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_packus_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_packus_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_packus_epi16(p0, p1);
            let expected = real::_mm256_packus_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_packus_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_packus_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_packus_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_packus_epi32(p0, p1);
            let expected = real::_mm256_packus_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_permute2x128_si256() {
    unsafe fn compare<const N: i32>((p0, p1, case): (hr::__m256i, hr::__m256i, usize)) {
        let got = hr::_mm256_permute2x128_si256::<N>(p0, p1);
        let expected = real::_mm256_permute2x128_si256::<N>(cast(p0), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_permute2x128_si256();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_permute2x128_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            immediate_cases!(
                compare,
                (p0, p1, case),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
                    42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
                    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
                    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
                    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132,
                    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148,
                    149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164,
                    165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180,
                    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196,
                    197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212,
                    213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
                    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244,
                    245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
                ]
            );
        }
    }
}

#[test]
fn _mm256_permute4x64_epi64() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m256i, usize)) {
        let got = hr::_mm256_permute4x64_epi64::<N>(p0);
        let expected = real::_mm256_permute4x64_epi64::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_permute4x64_epi64();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_permute4x64_epi64: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
                    42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
                    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
                    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
                    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132,
                    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148,
                    149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164,
                    165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180,
                    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196,
                    197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212,
                    213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
                    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244,
                    245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
                ]
            );
        }
    }
}

#[test]
fn _mm256_sad_epu8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_sad_epu8();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_sad_epu8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_sad_epu8(p0, p1);
            let expected = real::_mm256_sad_epu8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_set1_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_set1_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_set1_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i16 = sample(&mut rng, case, 0);
            let got = hr::_mm256_set1_epi16(p0);
            let expected = real::_mm256_set1_epi16(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_set1_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_set1_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_set1_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let got = hr::_mm256_set1_epi32(p0);
            let expected = real::_mm256_set1_epi32(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_set_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_set_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_set_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let p1: i32 = sample(&mut rng, case, 1);
            let p2: i32 = sample(&mut rng, case, 2);
            let p3: i32 = sample(&mut rng, case, 3);
            let p4: i32 = sample(&mut rng, case, 4);
            let p5: i32 = sample(&mut rng, case, 5);
            let p6: i32 = sample(&mut rng, case, 6);
            let p7: i32 = sample(&mut rng, case, 7);
            let got = hr::_mm256_set_epi32(p0, p1, p2, p3, p4, p5, p6, p7);
            let expected = real::_mm256_set_epi32(p0, p1, p2, p3, p4, p5, p6, p7);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_set_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_set_epi8();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_set_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let p1: i8 = sample(&mut rng, case, 1);
            let p2: i8 = sample(&mut rng, case, 2);
            let p3: i8 = sample(&mut rng, case, 3);
            let p4: i8 = sample(&mut rng, case, 4);
            let p5: i8 = sample(&mut rng, case, 5);
            let p6: i8 = sample(&mut rng, case, 6);
            let p7: i8 = sample(&mut rng, case, 7);
            let p8: i8 = sample(&mut rng, case, 8);
            let p9: i8 = sample(&mut rng, case, 9);
            let p10: i8 = sample(&mut rng, case, 10);
            let p11: i8 = sample(&mut rng, case, 11);
            let p12: i8 = sample(&mut rng, case, 12);
            let p13: i8 = sample(&mut rng, case, 13);
            let p14: i8 = sample(&mut rng, case, 14);
            let p15: i8 = sample(&mut rng, case, 15);
            let p16: i8 = sample(&mut rng, case, 16);
            let p17: i8 = sample(&mut rng, case, 17);
            let p18: i8 = sample(&mut rng, case, 18);
            let p19: i8 = sample(&mut rng, case, 19);
            let p20: i8 = sample(&mut rng, case, 20);
            let p21: i8 = sample(&mut rng, case, 21);
            let p22: i8 = sample(&mut rng, case, 22);
            let p23: i8 = sample(&mut rng, case, 23);
            let p24: i8 = sample(&mut rng, case, 24);
            let p25: i8 = sample(&mut rng, case, 25);
            let p26: i8 = sample(&mut rng, case, 26);
            let p27: i8 = sample(&mut rng, case, 27);
            let p28: i8 = sample(&mut rng, case, 28);
            let p29: i8 = sample(&mut rng, case, 29);
            let p30: i8 = sample(&mut rng, case, 30);
            let p31: i8 = sample(&mut rng, case, 31);
            let got = hr::_mm256_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            let expected = real::_mm256_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_setr_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_setr_epi8();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_setr_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let p1: i8 = sample(&mut rng, case, 1);
            let p2: i8 = sample(&mut rng, case, 2);
            let p3: i8 = sample(&mut rng, case, 3);
            let p4: i8 = sample(&mut rng, case, 4);
            let p5: i8 = sample(&mut rng, case, 5);
            let p6: i8 = sample(&mut rng, case, 6);
            let p7: i8 = sample(&mut rng, case, 7);
            let p8: i8 = sample(&mut rng, case, 8);
            let p9: i8 = sample(&mut rng, case, 9);
            let p10: i8 = sample(&mut rng, case, 10);
            let p11: i8 = sample(&mut rng, case, 11);
            let p12: i8 = sample(&mut rng, case, 12);
            let p13: i8 = sample(&mut rng, case, 13);
            let p14: i8 = sample(&mut rng, case, 14);
            let p15: i8 = sample(&mut rng, case, 15);
            let p16: i8 = sample(&mut rng, case, 16);
            let p17: i8 = sample(&mut rng, case, 17);
            let p18: i8 = sample(&mut rng, case, 18);
            let p19: i8 = sample(&mut rng, case, 19);
            let p20: i8 = sample(&mut rng, case, 20);
            let p21: i8 = sample(&mut rng, case, 21);
            let p22: i8 = sample(&mut rng, case, 22);
            let p23: i8 = sample(&mut rng, case, 23);
            let p24: i8 = sample(&mut rng, case, 24);
            let p25: i8 = sample(&mut rng, case, 25);
            let p26: i8 = sample(&mut rng, case, 26);
            let p27: i8 = sample(&mut rng, case, 27);
            let p28: i8 = sample(&mut rng, case, 28);
            let p29: i8 = sample(&mut rng, case, 29);
            let p30: i8 = sample(&mut rng, case, 30);
            let p31: i8 = sample(&mut rng, case, 31);
            let got = hr::_mm256_setr_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            let expected = real::_mm256_setr_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_setzero_si256() {
    unsafe {
        intel::_mm256_setzero_si256();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_setzero_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let got = hr::_mm256_setzero_si256();
            let expected = real::_mm256_setzero_si256();
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_shuffle_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_shuffle_epi8();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_shuffle_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_shuffle_epi8(p0, p1);
            let expected = real::_mm256_shuffle_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_slli_epi16() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m256i, usize)) {
        let got = hr::_mm256_slli_epi16::<N>(p0);
        let expected = real::_mm256_slli_epi16::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_slli_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_slli_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
}

#[test]
fn _mm256_slli_epi32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m256i, usize)) {
        let got = hr::_mm256_slli_epi32::<N>(p0);
        let expected = real::_mm256_slli_epi32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_slli_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_slli_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
}

#[test]
fn _mm256_srai_epi16() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m256i, usize)) {
        let got = hr::_mm256_srai_epi16::<N>(p0);
        let expected = real::_mm256_srai_epi16::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_srai_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_srai_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
}

#[test]
fn _mm256_srai_epi32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m256i, usize)) {
        let got = hr::_mm256_srai_epi32::<N>(p0);
        let expected = real::_mm256_srai_epi32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_srai_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_srai_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
}

#[test]
fn _mm256_storeu_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_storeu_si256();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_storeu_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            hr::_mm256_storeu_si256(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            real::_mm256_storeu_si256(q0.0.as_mut_ptr().add(case % 16).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn _mm256_sub_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_sub_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_sub_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_sub_epi16(p0, p1);
            let expected = real::_mm256_sub_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_sub_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_sub_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_sub_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_sub_epi32(p0, p1);
            let expected = real::_mm256_sub_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_testz_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_testz_si256();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_testz_si256: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_testz_si256(p0, p1);
            let expected = real::_mm256_testz_si256(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_unpackhi_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpackhi_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpackhi_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_unpackhi_epi16(p0, p1);
            let expected = real::_mm256_unpackhi_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_unpackhi_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpackhi_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpackhi_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_unpackhi_epi32(p0, p1);
            let expected = real::_mm256_unpackhi_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_unpackhi_epi64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpackhi_epi64();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpackhi_epi64: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_unpackhi_epi64(p0, p1);
            let expected = real::_mm256_unpackhi_epi64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_unpackhi_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpackhi_epi8();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpackhi_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_unpackhi_epi8(p0, p1);
            let expected = real::_mm256_unpackhi_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_unpacklo_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpacklo_epi16();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpacklo_epi16: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_unpacklo_epi16(p0, p1);
            let expected = real::_mm256_unpacklo_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_unpacklo_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpacklo_epi32();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpacklo_epi32: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_unpacklo_epi32(p0, p1);
            let expected = real::_mm256_unpacklo_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_unpacklo_epi64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpacklo_epi64();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpacklo_epi64: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_unpacklo_epi64(p0, p1);
            let expected = real::_mm256_unpacklo_epi64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm256_unpacklo_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm256_unpacklo_epi8();
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("_mm256_unpacklo_epi8: avx2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m256i = sample(&mut rng, case, 0);
            let p1: hr::__m256i = sample(&mut rng, case, 1);
            let got = hr::_mm256_unpacklo_epi8(p0, p1);
            let expected = real::_mm256_unpacklo_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_add_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_add_epi32();
        if !std::is_x86_feature_detected!("avx512f") {
            eprintln!("_mm512_add_epi32: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m512i = sample(&mut rng, case, 0);
            let p1: hr::__m512i = sample(&mut rng, case, 1);
            let got = hr::_mm512_add_epi32(p0, p1);
            let expected = real::_mm512_add_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_loadu_si512() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_loadu_si512();
        if !std::is_x86_feature_detected!("avx512f") {
            eprintln!("_mm512_loadu_si512: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::_mm512_loadu_si512(p0.0.as_ptr().add(case % 16).cast());
            let expected = real::_mm512_loadu_si512(q0.0.as_ptr().add(case % 16).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_madd_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_madd_epi16();
        if !std::is_x86_feature_detected!("avx512bw") {
            eprintln!("_mm512_madd_epi16: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m512i = sample(&mut rng, case, 0);
            let p1: hr::__m512i = sample(&mut rng, case, 1);
            let got = hr::_mm512_madd_epi16(p0, p1);
            let expected = real::_mm512_madd_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_maddubs_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_maddubs_epi16();
        if !std::is_x86_feature_detected!("avx512bw") {
            eprintln!("_mm512_maddubs_epi16: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m512i = sample(&mut rng, case, 0);
            let p1: hr::__m512i = sample(&mut rng, case, 1);
            let got = hr::_mm512_maddubs_epi16(p0, p1);
            let expected = real::_mm512_maddubs_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_sad_epu8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_sad_epu8();
        if !std::is_x86_feature_detected!("avx512bw") {
            eprintln!("_mm512_sad_epu8: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m512i = sample(&mut rng, case, 0);
            let p1: hr::__m512i = sample(&mut rng, case, 1);
            let got = hr::_mm512_sad_epu8(p0, p1);
            let expected = real::_mm512_sad_epu8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_set1_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_set1_epi16();
        if !std::is_x86_feature_detected!("avx512bw") {
            eprintln!("_mm512_set1_epi16: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i16 = sample(&mut rng, case, 0);
            let got = hr::_mm512_set1_epi16(p0);
            let expected = real::_mm512_set1_epi16(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_set_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_set_epi32();
        if !std::is_x86_feature_detected!("avx512f") {
            eprintln!("_mm512_set_epi32: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let p1: i32 = sample(&mut rng, case, 1);
            let p2: i32 = sample(&mut rng, case, 2);
            let p3: i32 = sample(&mut rng, case, 3);
            let p4: i32 = sample(&mut rng, case, 4);
            let p5: i32 = sample(&mut rng, case, 5);
            let p6: i32 = sample(&mut rng, case, 6);
            let p7: i32 = sample(&mut rng, case, 7);
            let p8: i32 = sample(&mut rng, case, 8);
            let p9: i32 = sample(&mut rng, case, 9);
            let p10: i32 = sample(&mut rng, case, 10);
            let p11: i32 = sample(&mut rng, case, 11);
            let p12: i32 = sample(&mut rng, case, 12);
            let p13: i32 = sample(&mut rng, case, 13);
            let p14: i32 = sample(&mut rng, case, 14);
            let p15: i32 = sample(&mut rng, case, 15);
            let got = hr::_mm512_set_epi32(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            let expected = real::_mm512_set_epi32(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_set_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_set_epi8();
        if !std::is_x86_feature_detected!("avx512bw") {
            eprintln!("_mm512_set_epi8: avx512bw unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let p1: i8 = sample(&mut rng, case, 1);
            let p2: i8 = sample(&mut rng, case, 2);
            let p3: i8 = sample(&mut rng, case, 3);
            let p4: i8 = sample(&mut rng, case, 4);
            let p5: i8 = sample(&mut rng, case, 5);
            let p6: i8 = sample(&mut rng, case, 6);
            let p7: i8 = sample(&mut rng, case, 7);
            let p8: i8 = sample(&mut rng, case, 8);
            let p9: i8 = sample(&mut rng, case, 9);
            let p10: i8 = sample(&mut rng, case, 10);
            let p11: i8 = sample(&mut rng, case, 11);
            let p12: i8 = sample(&mut rng, case, 12);
            let p13: i8 = sample(&mut rng, case, 13);
            let p14: i8 = sample(&mut rng, case, 14);
            let p15: i8 = sample(&mut rng, case, 15);
            let p16: i8 = sample(&mut rng, case, 16);
            let p17: i8 = sample(&mut rng, case, 17);
            let p18: i8 = sample(&mut rng, case, 18);
            let p19: i8 = sample(&mut rng, case, 19);
            let p20: i8 = sample(&mut rng, case, 20);
            let p21: i8 = sample(&mut rng, case, 21);
            let p22: i8 = sample(&mut rng, case, 22);
            let p23: i8 = sample(&mut rng, case, 23);
            let p24: i8 = sample(&mut rng, case, 24);
            let p25: i8 = sample(&mut rng, case, 25);
            let p26: i8 = sample(&mut rng, case, 26);
            let p27: i8 = sample(&mut rng, case, 27);
            let p28: i8 = sample(&mut rng, case, 28);
            let p29: i8 = sample(&mut rng, case, 29);
            let p30: i8 = sample(&mut rng, case, 30);
            let p31: i8 = sample(&mut rng, case, 31);
            let p32: i8 = sample(&mut rng, case, 32);
            let p33: i8 = sample(&mut rng, case, 33);
            let p34: i8 = sample(&mut rng, case, 34);
            let p35: i8 = sample(&mut rng, case, 35);
            let p36: i8 = sample(&mut rng, case, 36);
            let p37: i8 = sample(&mut rng, case, 37);
            let p38: i8 = sample(&mut rng, case, 38);
            let p39: i8 = sample(&mut rng, case, 39);
            let p40: i8 = sample(&mut rng, case, 40);
            let p41: i8 = sample(&mut rng, case, 41);
            let p42: i8 = sample(&mut rng, case, 42);
            let p43: i8 = sample(&mut rng, case, 43);
            let p44: i8 = sample(&mut rng, case, 44);
            let p45: i8 = sample(&mut rng, case, 45);
            let p46: i8 = sample(&mut rng, case, 46);
            let p47: i8 = sample(&mut rng, case, 47);
            let p48: i8 = sample(&mut rng, case, 48);
            let p49: i8 = sample(&mut rng, case, 49);
            let p50: i8 = sample(&mut rng, case, 50);
            let p51: i8 = sample(&mut rng, case, 51);
            let p52: i8 = sample(&mut rng, case, 52);
            let p53: i8 = sample(&mut rng, case, 53);
            let p54: i8 = sample(&mut rng, case, 54);
            let p55: i8 = sample(&mut rng, case, 55);
            let p56: i8 = sample(&mut rng, case, 56);
            let p57: i8 = sample(&mut rng, case, 57);
            let p58: i8 = sample(&mut rng, case, 58);
            let p59: i8 = sample(&mut rng, case, 59);
            let p60: i8 = sample(&mut rng, case, 60);
            let p61: i8 = sample(&mut rng, case, 61);
            let p62: i8 = sample(&mut rng, case, 62);
            let p63: i8 = sample(&mut rng, case, 63);
            let got = hr::_mm512_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31, p32, p33,
                p34, p35, p36, p37, p38, p39, p40, p41, p42, p43, p44, p45, p46, p47, p48, p49,
                p50, p51, p52, p53, p54, p55, p56, p57, p58, p59, p60, p61, p62, p63,
            );
            let expected = real::_mm512_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31, p32, p33,
                p34, p35, p36, p37, p38, p39, p40, p41, p42, p43, p44, p45, p46, p47, p48, p49,
                p50, p51, p52, p53, p54, p55, p56, p57, p58, p59, p60, p61, p62, p63,
            );
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_setzero_si512() {
    unsafe {
        intel::_mm512_setzero_si512();
        if !std::is_x86_feature_detected!("avx512f") {
            eprintln!("_mm512_setzero_si512: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let got = hr::_mm512_setzero_si512();
            let expected = real::_mm512_setzero_si512();
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm512_slli_epi32() {
    unsafe fn compare<const N: u32>((p0, case): (hr::__m512i, usize)) {
        let got = hr::_mm512_slli_epi32::<N>(p0);
        let expected = real::_mm512_slli_epi32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm512_slli_epi32();
        if !std::is_x86_feature_detected!("avx512f") {
            eprintln!("_mm512_slli_epi32: avx512f unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m512i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
}

#[test]
fn _mm_abs_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_abs_epi16();
        if !std::is_x86_feature_detected!("ssse3") {
            eprintln!("_mm_abs_epi16: ssse3 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let got = hr::_mm_abs_epi16(p0);
            let expected = real::_mm_abs_epi16(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_add_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_add_epi16();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_add_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_add_epi16(p0, p1);
            let expected = real::_mm_add_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_add_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_add_epi32();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_add_epi32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_add_epi32(p0, p1);
            let expected = real::_mm_add_epi32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_add_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_add_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_add_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_add_epi8(p0, p1);
            let expected = real::_mm_add_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_and_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_and_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_and_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_and_si128(p0, p1);
            let expected = real::_mm_and_si128(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_andnot_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_andnot_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_andnot_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_andnot_si128(p0, p1);
            let expected = real::_mm_andnot_si128(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_avg_epu8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_avg_epu8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_avg_epu8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_avg_epu8(p0, p1);
            let expected = real::_mm_avg_epu8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_blendv_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_blendv_epi8();
        if !std::is_x86_feature_detected!("sse4.1") {
            eprintln!("_mm_blendv_epi8: sse4.1 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let p2: hr::__m128i = sample(&mut rng, case, 2);
            let got = hr::_mm_blendv_epi8(p0, p1, p2);
            let expected = real::_mm_blendv_epi8(cast(p0), cast(p1), cast(p2));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_clmulepi64_si128() {
    unsafe fn compare<const N: i32>((p0, p1, case): (hr::__m128i, hr::__m128i, usize)) {
        let got = hr::_mm_clmulepi64_si128::<N>(p0, p1);
        let expected = real::_mm_clmulepi64_si128::<N>(cast(p0), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_clmulepi64_si128();
        if !std::is_x86_feature_detected!("pclmulqdq") {
            eprintln!("_mm_clmulepi64_si128: pclmulqdq unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            immediate_cases!(compare, (p0, p1, case), [0, 1, 16, 17, 255]);
        }
    }
}

#[test]
fn _mm_cmpeq_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cmpeq_epi16();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cmpeq_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_cmpeq_epi16(p0, p1);
            let expected = real::_mm_cmpeq_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_cvtsi128_si32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cvtsi128_si32();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cvtsi128_si32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let got = hr::_mm_cvtsi128_si32(p0);
            let expected = real::_mm_cvtsi128_si32(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_cvtsi32_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cvtsi32_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cvtsi32_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let got = hr::_mm_cvtsi32_si128(p0);
            let expected = real::_mm_cvtsi32_si128(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_extract_epi32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m128i, usize)) {
        let got = hr::_mm_extract_epi32::<N>(p0);
        let expected = real::_mm_extract_epi32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_extract_epi32();
        if !std::is_x86_feature_detected!("sse4.1") {
            eprintln!("_mm_extract_epi32: sse4.1 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            immediate_cases!(compare, (p0, case), [0, 1, 2, 3]);
        }
    }
}

#[test]
fn _mm_load_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_load_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_load_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::_mm_load_si128(p0.0.as_ptr().add(0).cast());
            let expected = real::_mm_load_si128(q0.0.as_ptr().add(0).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_loadu_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_loadu_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_loadu_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: Buffer = sample(&mut rng, case, 0);
            let q0 = p0;
            let got = hr::_mm_loadu_si128(p0.0.as_ptr().add(case % 16).cast());
            let expected = real::_mm_loadu_si128(q0.0.as_ptr().add(case % 16).cast());
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_madd_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_madd_epi16();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_madd_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_madd_epi16(p0, p1);
            let expected = real::_mm_madd_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_maddubs_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_maddubs_epi16();
        if !std::is_x86_feature_detected!("ssse3") {
            eprintln!("_mm_maddubs_epi16: ssse3 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_maddubs_epi16(p0, p1);
            let expected = real::_mm_maddubs_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_min_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_min_epi16();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_min_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_min_epi16(p0, p1);
            let expected = real::_mm_min_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_or_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_or_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_or_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_or_si128(p0, p1);
            let expected = real::_mm_or_si128(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_packus_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_packus_epi16();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_packus_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_packus_epi16(p0, p1);
            let expected = real::_mm_packus_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_sad_epu8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_sad_epu8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_sad_epu8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_sad_epu8(p0, p1);
            let expected = real::_mm_sad_epu8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_set1_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set1_epi16();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set1_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i16 = sample(&mut rng, case, 0);
            let got = hr::_mm_set1_epi16(p0);
            let expected = real::_mm_set1_epi16(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_set1_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set1_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set1_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let got = hr::_mm_set1_epi8(p0);
            let expected = real::_mm_set1_epi8(p0);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_set_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set_epi32();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set_epi32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i32 = sample(&mut rng, case, 0);
            let p1: i32 = sample(&mut rng, case, 1);
            let p2: i32 = sample(&mut rng, case, 2);
            let p3: i32 = sample(&mut rng, case, 3);
            let got = hr::_mm_set_epi32(p0, p1, p2, p3);
            let expected = real::_mm_set_epi32(p0, p1, p2, p3);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_set_epi64x() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set_epi64x();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set_epi64x: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i64 = sample(&mut rng, case, 0);
            let p1: i64 = sample(&mut rng, case, 1);
            let got = hr::_mm_set_epi64x(p0, p1);
            let expected = real::_mm_set_epi64x(p0, p1);
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_set_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_set_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_set_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: i8 = sample(&mut rng, case, 0);
            let p1: i8 = sample(&mut rng, case, 1);
            let p2: i8 = sample(&mut rng, case, 2);
            let p3: i8 = sample(&mut rng, case, 3);
            let p4: i8 = sample(&mut rng, case, 4);
            let p5: i8 = sample(&mut rng, case, 5);
            let p6: i8 = sample(&mut rng, case, 6);
            let p7: i8 = sample(&mut rng, case, 7);
            let p8: i8 = sample(&mut rng, case, 8);
            let p9: i8 = sample(&mut rng, case, 9);
            let p10: i8 = sample(&mut rng, case, 10);
            let p11: i8 = sample(&mut rng, case, 11);
            let p12: i8 = sample(&mut rng, case, 12);
            let p13: i8 = sample(&mut rng, case, 13);
            let p14: i8 = sample(&mut rng, case, 14);
            let p15: i8 = sample(&mut rng, case, 15);
            let got = hr::_mm_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            let expected = real::_mm_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_setzero_si128() {
    unsafe {
        intel::_mm_setzero_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_setzero_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let got = hr::_mm_setzero_si128();
            let expected = real::_mm_setzero_si128();
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_sha1msg1_epu32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        if !std::is_x86_feature_detected!("sha") {
            eprintln!("_mm_sha1msg1_epu32: sha unavailable; hardware coverage skipped");
            return;
        }
        intel::_mm_sha1msg1_epu32();
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_sha1msg1_epu32(p0, p1);
            let expected = real::_mm_sha1msg1_epu32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_sha1msg2_epu32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        if !std::is_x86_feature_detected!("sha") {
            eprintln!("_mm_sha1msg2_epu32: sha unavailable; hardware coverage skipped");
            return;
        }
        intel::_mm_sha1msg2_epu32();
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_sha1msg2_epu32(p0, p1);
            let expected = real::_mm_sha1msg2_epu32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_sha1nexte_epu32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        if !std::is_x86_feature_detected!("sha") {
            eprintln!("_mm_sha1nexte_epu32: sha unavailable; hardware coverage skipped");
            return;
        }
        intel::_mm_sha1nexte_epu32();
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_sha1nexte_epu32(p0, p1);
            let expected = real::_mm_sha1nexte_epu32(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_sha1rnds4_epu32() {
    unsafe fn compare<const N: i32>((p0, p1, case): (hr::__m128i, hr::__m128i, usize)) {
        let got = hr::_mm_sha1rnds4_epu32::<N>(p0, p1);
        let expected = real::_mm_sha1rnds4_epu32::<N>(cast(p0), cast(p1));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        if !std::is_x86_feature_detected!("sha") {
            eprintln!("_mm_sha1rnds4_epu32: sha unavailable; hardware coverage skipped");
            return;
        }
        intel::_mm_sha1rnds4_epu32();
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            immediate_cases!(compare, (p0, p1, case), [0, 1, 2, 3]);
        }
    }
}

#[test]
fn _mm_shuffle_epi32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m128i, usize)) {
        let got = hr::_mm_shuffle_epi32::<N>(p0);
        let expected = real::_mm_shuffle_epi32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_shuffle_epi32();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_shuffle_epi32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [
                    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21,
                    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41,
                    42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61,
                    62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
                    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
                    101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
                    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132,
                    133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148,
                    149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 164,
                    165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 180,
                    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196,
                    197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212,
                    213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228,
                    229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244,
                    245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
                ]
            );
        }
    }
}

#[test]
fn _mm_shuffle_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_shuffle_epi8();
        if !std::is_x86_feature_detected!("ssse3") {
            eprintln!("_mm_shuffle_epi8: ssse3 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_shuffle_epi8(p0, p1);
            let expected = real::_mm_shuffle_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_slli_epi32() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m128i, usize)) {
        let got = hr::_mm_slli_epi32::<N>(p0);
        let expected = real::_mm_slli_epi32::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_slli_epi32();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_slli_epi32: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
}

#[test]
fn _mm_srli_si128() {
    unsafe fn compare<const N: i32>((p0, case): (hr::__m128i, usize)) {
        let got = hr::_mm_srli_si128::<N>(p0);
        let expected = real::_mm_srli_si128::<N>(cast(p0));
        assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate {N}");
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_srli_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_srli_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            immediate_cases!(
                compare,
                (p0, case),
                [0, 1, 2, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 127, 128, 255]
            );
        }
    }
}

#[test]
fn _mm_storeu_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_storeu_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_storeu_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            hr::_mm_storeu_si128(p0.0.as_mut_ptr().add(case % 16).cast(), p1);
            real::_mm_storeu_si128(q0.0.as_mut_ptr().add(case % 16).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[test]
fn _mm_sub_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_sub_epi16();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_sub_epi16: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_sub_epi16(p0, p1);
            let expected = real::_mm_sub_epi16(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_sub_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_sub_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_sub_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_sub_epi8(p0, p1);
            let expected = real::_mm_sub_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_unpackhi_epi64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_unpackhi_epi64();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_unpackhi_epi64: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_unpackhi_epi64(p0, p1);
            let expected = real::_mm_unpackhi_epi64(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_unpackhi_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_unpackhi_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_unpackhi_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_unpackhi_epi8(p0, p1);
            let expected = real::_mm_unpackhi_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_unpacklo_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_unpacklo_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_unpacklo_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_unpacklo_epi8(p0, p1);
            let expected = real::_mm_unpacklo_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_xor_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_xor_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_xor_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_xor_si128(p0, p1);
            let expected = real::_mm_xor_si128(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn float_vector_types_layout() {
    use core::arch::x86_64 as real;
    use core::mem::{align_of, size_of};
    use relax_std_arch_check::hr_x86 as hrx;
    assert_eq!((size_of::<hrx::__m128>(), align_of::<hrx::__m128>()), (size_of::<real::__m128>(), align_of::<real::__m128>()));
    assert_eq!((size_of::<hrx::__m128d>(), align_of::<hrx::__m128d>()), (size_of::<real::__m128d>(), align_of::<real::__m128d>()));
    assert_eq!((size_of::<hrx::__m256>(), align_of::<hrx::__m256>()), (size_of::<real::__m256>(), align_of::<real::__m256>()));
    assert_eq!((size_of::<hrx::__m256d>(), align_of::<hrx::__m256d>()), (size_of::<real::__m256d>(), align_of::<real::__m256d>()));
    assert_eq!((size_of::<hrx::__m512>(), align_of::<hrx::__m512>()), (size_of::<real::__m512>(), align_of::<real::__m512>()));
    assert_eq!((size_of::<hrx::__m512d>(), align_of::<hrx::__m512d>()), (size_of::<real::__m512d>(), align_of::<real::__m512d>()));
    // bit patterns survive a transmute round trip through the real type
    let v = hrx::__m128([1.5, -2.0, f32::MIN_POSITIVE, 3.25]);
    let r: real::__m128 = unsafe { core::mem::transmute(v) };
    let back: [f32; 4] = unsafe { core::mem::transmute(r) };
    assert_eq!(back, [1.5, -2.0, f32::MIN_POSITIVE, 3.25]);
}

#[test]
fn _mm_cmpeq_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cmpeq_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cmpeq_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_cmpeq_epi8(p0, p1);
            let expected = real::_mm_cmpeq_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_cmpgt_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_cmpgt_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_cmpgt_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            let got = hr::_mm_cmpgt_epi8(p0, p1);
            let expected = real::_mm_cmpgt_epi8(cast(p0), cast(p1));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_movemask_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_movemask_epi8();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_movemask_epi8: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let p0: hr::__m128i = sample(&mut rng, case, 0);
            let got = hr::_mm_movemask_epi8(p0);
            let expected = real::_mm_movemask_epi8(cast(p0));
            assert_eq!(bytes(&got), bytes(&expected), "case {case}, immediate None");
        }
    }
}

#[test]
fn _mm_store_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        intel::_mm_store_si128();
        if !std::is_x86_feature_detected!("sse2") {
            eprintln!("_mm_store_si128: sse2 unavailable; 3 Intel fixtures passed");
            return;
        }
        for case in 0..RANDOM_CASES + 17 {
            let mut p0: Buffer = sample(&mut rng, case, 0);
            let mut q0 = p0;
            let p1: hr::__m128i = sample(&mut rng, case, 1);
            hr::_mm_store_si128(p0.0.as_mut_ptr().add((case % 12) * 16).cast(), p1);
            real::_mm_store_si128(q0.0.as_mut_ptr().add((case % 12) * 16).cast(), cast(p1));
            assert_eq!(p0.0, q0.0, "case {case}");
        }
    }
}

#[allow(dead_code)]
mod sha256_oracle {
    include!("common/sha256_oracle.rs");
}
include!("common/sha256_cases.rs");
