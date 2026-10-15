// Literal Intel-pseudocode fixtures: zero, signed boundaries, and asymmetric bytes.
// Each function has three independent expected results; no call to the emulation
// is used to calculate an expectation.
use super::*;

pub fn _MM_SHUFFLE() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: u32 = sample(&mut rng, 0, 0);
            let p1: u32 = sample(&mut rng, 0, 1);
            let p2: u32 = sample(&mut rng, 0, 2);
            let p3: u32 = sample(&mut rng, 0, 3);
            let got = hr::_MM_SHUFFLE(p0, p1, p2, p3);
            assert_eq!(bytes(&got), &[0, 0, 0, 0], "Intel fixture 0");
        }
        {
            let p0: u32 = sample(&mut rng, 1, 0);
            let p1: u32 = sample(&mut rng, 1, 1);
            let p2: u32 = sample(&mut rng, 1, 2);
            let p3: u32 = sample(&mut rng, 1, 3);
            let got = hr::_MM_SHUFFLE(p0, p1, p2, p3);
            assert_eq!(bytes(&got), &[192, 255, 255, 7], "Intel fixture 1");
        }
        {
            let p0: u32 = sample(&mut rng, 2, 0);
            let p1: u32 = sample(&mut rng, 2, 1);
            let p2: u32 = sample(&mut rng, 2, 2);
            let p3: u32 = sample(&mut rng, 2, 3);
            let got = hr::_MM_SHUFFLE(p0, p1, p2, p3);
            assert_eq!(bytes(&got), &[236, 255, 255, 223], "Intel fixture 2");
        }
    }
}

pub fn _mm256_add_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_add_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_add_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    127, 255, 127, 128, 0, 0, 0, 0, 128, 1, 127, 255, 128, 127, 253, 128, 127, 255,
                    127, 128, 0, 0, 0, 0, 128, 1, 127, 255, 128, 127, 253, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_add_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    109, 183, 1, 76, 149, 224, 41, 116, 189, 7, 81, 156, 229, 47, 121, 195, 13, 88,
                    161, 236, 53, 128, 201, 19, 93, 168, 241, 59, 133, 207, 25, 100
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_add_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    127, 255, 128, 128, 0, 0, 1, 0, 128, 1, 127, 255, 128, 127, 254, 128, 127, 255,
                    128, 128, 0, 0, 1, 0, 128, 1, 127, 255, 128, 127, 254, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    109, 183, 1, 76, 149, 224, 41, 116, 189, 7, 82, 156, 229, 47, 122, 195, 13, 88,
                    162, 236, 53, 128, 201, 19, 93, 168, 242, 59, 133, 207, 25, 100
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_blend_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_blend_epi32::<0>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_blend_epi32::<128>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255,
                    0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 127, 255, 255, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_blend_epi32::<255>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    90, 127, 164, 201, 238, 19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133, 170,
                    207, 244, 25, 62, 99, 136, 173, 210, 247, 28, 65, 102, 139, 176, 213
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_blendv_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let p2: hr::__m256i = sample(&mut rng, 0, 2);
            let got = hr::_mm256_blendv_epi8(p0, p1, p2);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let p2: hr::__m256i = sample(&mut rng, 1, 2);
            let got = hr::_mm256_blendv_epi8(p0, p1, p2);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 127, 128, 1, 127, 0, 255, 0, 0, 128, 254, 1, 255, 255, 127, 255, 255,
                    127, 128, 1, 127, 0, 255, 0, 0, 128, 254, 1, 255, 255, 127
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let p2: hr::__m256i = sample(&mut rng, 2, 2);
            let got = hr::_mm256_blendv_epi8(p0, p1, p2);
            assert_eq!(
                bytes(&got),
                &[
                    90, 127, 164, 130, 167, 204, 241, 93, 130, 167, 133, 170, 207, 59, 96, 133,
                    170, 136, 173, 210, 62, 99, 136, 173, 139, 176, 213, 65, 102, 139, 176, 142
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_castsi256_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let got = hr::_mm256_castsi256_si128(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let got = hr::_mm256_castsi256_si128(p0);
            assert_eq!(
                bytes(&got),
                &[255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let got = hr::_mm256_castsi256_si128(p0);
            assert_eq!(
                bytes(&got),
                &[19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133, 170, 207, 244, 25, 62],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_extractf128_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let got = hr::_mm256_extractf128_si256::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let got = hr::_mm256_extractf128_si256::<1>(p0);
            assert_eq!(
                bytes(&got),
                &[255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let got = hr::_mm256_extractf128_si256::<1>(p0);
            assert_eq!(
                bytes(&got),
                &[99, 136, 173, 210, 247, 28, 65, 102, 139, 176, 213, 250, 31, 68, 105, 142],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_extracti128_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let got = hr::_mm256_extracti128_si256::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let got = hr::_mm256_extracti128_si256::<1>(p0);
            assert_eq!(
                bytes(&got),
                &[255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let got = hr::_mm256_extracti128_si256::<1>(p0);
            assert_eq!(
                bytes(&got),
                &[99, 136, 173, 210, 247, 28, 65, 102, 139, 176, 213, 250, 31, 68, 105, 142],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_loadu_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: Buffer = sample(&mut rng, 0, 0);
            let got = hr::_mm256_loadu_si256(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: Buffer = sample(&mut rng, 1, 0);
            let got = hr::_mm256_loadu_si256(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255,
                    0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: Buffer = sample(&mut rng, 2, 0);
            let got = hr::_mm256_loadu_si256(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133, 170, 207, 244, 25, 62, 99,
                    136, 173, 210, 247, 28, 65, 102, 139, 176, 213, 250, 31, 68, 105, 142
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_madd_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 128, 192, 255, 255, 255, 255, 191, 128, 1, 255, 255, 129, 253, 191, 0,
                    128, 128, 192, 255, 255, 255, 255, 191, 128, 1, 255, 255, 129, 253, 191, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    66, 172, 146, 54, 250, 57, 91, 4, 242, 113, 122, 227, 42, 5, 172, 223, 162, 80,
                    253, 17, 90, 200, 73, 234, 82, 115, 57, 1, 138, 62, 191, 243
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_maddubs_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 128, 0, 0, 127, 193, 255, 255, 0, 0, 128, 128, 255, 255, 2, 255, 0, 128, 0,
                    0, 127, 193, 255, 255, 0, 0, 128, 128, 255, 255, 2, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    118, 34, 166, 194, 102, 3, 182, 60, 150, 193, 6, 219, 6, 74, 150, 235, 182,
                    196, 102, 12, 166, 70, 118, 192, 214, 224, 198, 86, 70, 237, 86, 199
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_max_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_max_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_max_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 127, 0, 255, 127, 0, 1, 0, 1, 255, 0, 127, 255, 254, 127, 255, 255,
                    127, 0, 255, 127, 0, 1, 0, 1, 255, 0, 127, 255, 254, 127
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_max_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    90, 127, 164, 201, 238, 19, 56, 93, 59, 96, 204, 241, 22, 59, 25, 62, 170, 207,
                    244, 25, 62, 99, 65, 102, 210, 247, 28, 65, 31, 68, 176, 213
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_min_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_min_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_min_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 255, 0, 128, 1, 128, 0, 255, 128, 0, 128, 254, 1, 128, 255, 0, 128, 255,
                    0, 128, 1, 128, 0, 255, 128, 0, 128, 254, 1, 128, 255, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_min_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 93, 130, 167, 204, 241, 22, 130, 167, 133, 170, 207, 244, 96, 133, 99,
                    136, 173, 210, 247, 28, 136, 173, 139, 176, 213, 250, 102, 139, 105, 142
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_mullo_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_mullo_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_mullo_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 0, 0, 128, 255, 255, 0, 0, 0, 128, 128, 129, 127, 127, 2, 126, 128, 0, 0,
                    128, 255, 255, 0, 0, 0, 128, 128, 129, 127, 127, 2, 126
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_mullo_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    174, 35, 148, 136, 66, 168, 184, 145, 246, 90, 252, 22, 202, 190, 96, 70, 190,
                    158, 228, 177, 210, 136, 136, 63, 6, 239, 76, 132, 90, 249, 48, 69
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_mullo_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_mullo_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_mullo_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 0, 0, 255, 255, 255, 255, 65, 0, 128, 0, 63, 127, 127, 193, 129, 128, 0,
                    0, 255, 255, 255, 255, 65, 0, 128, 0, 63, 127, 127, 193, 129
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_mullo_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    174, 35, 195, 234, 66, 168, 132, 182, 246, 90, 133, 211, 202, 190, 70, 170,
                    190, 158, 224, 225, 210, 136, 48, 64, 6, 239, 212, 138, 90, 249, 61, 134
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_or_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_or_si256(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_or_si256(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 127, 128, 255, 255, 0, 255, 128, 1, 255, 254, 127, 255, 255, 127,
                    255, 255, 127, 128, 255, 255, 0, 255, 128, 1, 255, 254, 127, 255, 255, 127
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_or_si256(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    91, 127, 253, 203, 239, 223, 249, 95, 187, 231, 205, 251, 223, 255, 121, 191,
                    235, 207, 253, 219, 255, 127, 201, 239, 219, 247, 221, 251, 127, 207, 249, 223
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_packs_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_packs_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_packs_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 128, 255, 127, 255, 127, 255, 127, 255, 127, 0, 128, 0, 128, 255, 127, 0,
                    128, 255, 127, 255, 127, 255, 127, 255, 127, 0, 128, 0, 128, 255, 127
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_packs_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 128, 255, 127, 0, 128, 255, 127, 0, 128, 255, 127, 0, 128, 0, 128, 0, 128,
                    255, 127, 0, 128, 0, 128, 255, 127, 0, 128, 255, 127, 0, 128
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_packus_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_packus_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_packus_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 255, 255, 128, 255, 0, 255, 0, 127, 0, 0, 255, 0, 0, 255, 0, 0, 255, 255,
                    128, 255, 0, 255, 0, 127, 0, 0, 255, 0, 0, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_packus_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 255, 255, 0, 0, 255, 0, 0, 0, 255, 255,
                    0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_packus_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_packus_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_packus_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 0, 0, 255,
                    255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 255, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_packus_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 255, 255, 0,
                    0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_permute2x128_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_permute2x128_si256::<0>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_permute2x128_si256::<128>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_permute2x128_si256::<255>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_permute4x64_epi64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let got = hr::_mm256_permute4x64_epi64::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let got = hr::_mm256_permute4x64_epi64::<128>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 0, 128, 255, 127, 0, 1, 255, 255, 0, 128, 255, 127, 0, 1, 255, 255,
                    0, 128, 255, 127, 0, 1, 255, 255, 0, 128, 255, 127, 0, 1
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let got = hr::_mm256_permute4x64_epi64::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    139, 176, 213, 250, 31, 68, 105, 142, 139, 176, 213, 250, 31, 68, 105, 142,
                    139, 176, 213, 250, 31, 68, 105, 142, 139, 176, 213, 250, 31, 68, 105, 142
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_sad_epu8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    123, 3, 0, 0, 0, 0, 0, 0, 123, 3, 0, 0, 0, 0, 0, 0, 123, 3, 0, 0, 0, 0, 0, 0,
                    123, 3, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    28, 3, 0, 0, 0, 0, 0, 0, 28, 3, 0, 0, 0, 0, 0, 0, 28, 3, 0, 0, 0, 0, 0, 0, 28,
                    3, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_set1_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i16 = sample(&mut rng, 0, 0);
            let got = hr::_mm256_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: i16 = sample(&mut rng, 1, 0);
            let got = hr::_mm256_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: i16 = sample(&mut rng, 2, 0);
            let got = hr::_mm256_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56,
                    19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_set1_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i32 = sample(&mut rng, 0, 0);
            let got = hr::_mm256_set1_epi32(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 1, 0);
            let got = hr::_mm256_set1_epi32(p0);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 0, 128, 255, 255, 0, 128, 255, 255, 0, 128, 255, 255, 0, 128, 255,
                    255, 0, 128, 255, 255, 0, 128, 255, 255, 0, 128, 255, 255, 0, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 2, 0);
            let got = hr::_mm256_set1_epi32(p0);
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 93, 130, 19, 56, 93, 130, 19, 56, 93, 130, 19, 56, 93, 130, 19, 56, 93,
                    130, 19, 56, 93, 130, 19, 56, 93, 130, 19, 56, 93, 130
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_set_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i32 = sample(&mut rng, 0, 0);
            let p1: i32 = sample(&mut rng, 0, 1);
            let p2: i32 = sample(&mut rng, 0, 2);
            let p3: i32 = sample(&mut rng, 0, 3);
            let p4: i32 = sample(&mut rng, 0, 4);
            let p5: i32 = sample(&mut rng, 0, 5);
            let p6: i32 = sample(&mut rng, 0, 6);
            let p7: i32 = sample(&mut rng, 0, 7);
            let got = hr::_mm256_set_epi32(p0, p1, p2, p3, p4, p5, p6, p7);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 1, 0);
            let p1: i32 = sample(&mut rng, 1, 1);
            let p2: i32 = sample(&mut rng, 1, 2);
            let p3: i32 = sample(&mut rng, 1, 3);
            let p4: i32 = sample(&mut rng, 1, 4);
            let p5: i32 = sample(&mut rng, 1, 5);
            let p6: i32 = sample(&mut rng, 1, 6);
            let p7: i32 = sample(&mut rng, 1, 7);
            let got = hr::_mm256_set_epi32(p0, p1, p2, p3, p4, p5, p6, p7);
            assert_eq!(
                bytes(&got),
                &[
                    127, 0, 1, 128, 0, 128, 255, 127, 127, 255, 255, 0, 1, 128, 254, 127, 0, 255,
                    0, 1, 0, 1, 128, 0, 128, 255, 127, 0, 255, 255, 0, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 2, 0);
            let p1: i32 = sample(&mut rng, 2, 1);
            let p2: i32 = sample(&mut rng, 2, 2);
            let p3: i32 = sample(&mut rng, 2, 3);
            let p4: i32 = sample(&mut rng, 2, 4);
            let p5: i32 = sample(&mut rng, 2, 5);
            let p6: i32 = sample(&mut rng, 2, 6);
            let p7: i32 = sample(&mut rng, 2, 7);
            let got = hr::_mm256_set_epi32(p0, p1, p2, p3, p4, p5, p6, p7);
            assert_eq!(
                bytes(&got),
                &[
                    4, 41, 78, 115, 189, 226, 7, 44, 118, 155, 192, 229, 47, 84, 121, 158, 232, 13,
                    50, 87, 161, 198, 235, 16, 90, 127, 164, 201, 19, 56, 93, 130
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_set_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i8 = sample(&mut rng, 0, 0);
            let p1: i8 = sample(&mut rng, 0, 1);
            let p2: i8 = sample(&mut rng, 0, 2);
            let p3: i8 = sample(&mut rng, 0, 3);
            let p4: i8 = sample(&mut rng, 0, 4);
            let p5: i8 = sample(&mut rng, 0, 5);
            let p6: i8 = sample(&mut rng, 0, 6);
            let p7: i8 = sample(&mut rng, 0, 7);
            let p8: i8 = sample(&mut rng, 0, 8);
            let p9: i8 = sample(&mut rng, 0, 9);
            let p10: i8 = sample(&mut rng, 0, 10);
            let p11: i8 = sample(&mut rng, 0, 11);
            let p12: i8 = sample(&mut rng, 0, 12);
            let p13: i8 = sample(&mut rng, 0, 13);
            let p14: i8 = sample(&mut rng, 0, 14);
            let p15: i8 = sample(&mut rng, 0, 15);
            let p16: i8 = sample(&mut rng, 0, 16);
            let p17: i8 = sample(&mut rng, 0, 17);
            let p18: i8 = sample(&mut rng, 0, 18);
            let p19: i8 = sample(&mut rng, 0, 19);
            let p20: i8 = sample(&mut rng, 0, 20);
            let p21: i8 = sample(&mut rng, 0, 21);
            let p22: i8 = sample(&mut rng, 0, 22);
            let p23: i8 = sample(&mut rng, 0, 23);
            let p24: i8 = sample(&mut rng, 0, 24);
            let p25: i8 = sample(&mut rng, 0, 25);
            let p26: i8 = sample(&mut rng, 0, 26);
            let p27: i8 = sample(&mut rng, 0, 27);
            let p28: i8 = sample(&mut rng, 0, 28);
            let p29: i8 = sample(&mut rng, 0, 29);
            let p30: i8 = sample(&mut rng, 0, 30);
            let p31: i8 = sample(&mut rng, 0, 31);
            let got = hr::_mm256_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 1, 0);
            let p1: i8 = sample(&mut rng, 1, 1);
            let p2: i8 = sample(&mut rng, 1, 2);
            let p3: i8 = sample(&mut rng, 1, 3);
            let p4: i8 = sample(&mut rng, 1, 4);
            let p5: i8 = sample(&mut rng, 1, 5);
            let p6: i8 = sample(&mut rng, 1, 6);
            let p7: i8 = sample(&mut rng, 1, 7);
            let p8: i8 = sample(&mut rng, 1, 8);
            let p9: i8 = sample(&mut rng, 1, 9);
            let p10: i8 = sample(&mut rng, 1, 10);
            let p11: i8 = sample(&mut rng, 1, 11);
            let p12: i8 = sample(&mut rng, 1, 12);
            let p13: i8 = sample(&mut rng, 1, 13);
            let p14: i8 = sample(&mut rng, 1, 14);
            let p15: i8 = sample(&mut rng, 1, 15);
            let p16: i8 = sample(&mut rng, 1, 16);
            let p17: i8 = sample(&mut rng, 1, 17);
            let p18: i8 = sample(&mut rng, 1, 18);
            let p19: i8 = sample(&mut rng, 1, 19);
            let p20: i8 = sample(&mut rng, 1, 20);
            let p21: i8 = sample(&mut rng, 1, 21);
            let p22: i8 = sample(&mut rng, 1, 22);
            let p23: i8 = sample(&mut rng, 1, 23);
            let p24: i8 = sample(&mut rng, 1, 24);
            let p25: i8 = sample(&mut rng, 1, 25);
            let p26: i8 = sample(&mut rng, 1, 26);
            let p27: i8 = sample(&mut rng, 1, 27);
            let p28: i8 = sample(&mut rng, 1, 28);
            let p29: i8 = sample(&mut rng, 1, 29);
            let p30: i8 = sample(&mut rng, 1, 30);
            let p31: i8 = sample(&mut rng, 1, 31);
            let got = hr::_mm256_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            assert_eq!(
                bytes(&got),
                &[
                    128, 255, 1, 255, 255, 254, 0, 128, 127, 0, 127, 1, 0, 0, 128, 255, 128, 255,
                    1, 255, 255, 254, 0, 128, 127, 0, 127, 1, 0, 0, 128, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 2, 0);
            let p1: i8 = sample(&mut rng, 2, 1);
            let p2: i8 = sample(&mut rng, 2, 2);
            let p3: i8 = sample(&mut rng, 2, 3);
            let p4: i8 = sample(&mut rng, 2, 4);
            let p5: i8 = sample(&mut rng, 2, 5);
            let p6: i8 = sample(&mut rng, 2, 6);
            let p7: i8 = sample(&mut rng, 2, 7);
            let p8: i8 = sample(&mut rng, 2, 8);
            let p9: i8 = sample(&mut rng, 2, 9);
            let p10: i8 = sample(&mut rng, 2, 10);
            let p11: i8 = sample(&mut rng, 2, 11);
            let p12: i8 = sample(&mut rng, 2, 12);
            let p13: i8 = sample(&mut rng, 2, 13);
            let p14: i8 = sample(&mut rng, 2, 14);
            let p15: i8 = sample(&mut rng, 2, 15);
            let p16: i8 = sample(&mut rng, 2, 16);
            let p17: i8 = sample(&mut rng, 2, 17);
            let p18: i8 = sample(&mut rng, 2, 18);
            let p19: i8 = sample(&mut rng, 2, 19);
            let p20: i8 = sample(&mut rng, 2, 20);
            let p21: i8 = sample(&mut rng, 2, 21);
            let p22: i8 = sample(&mut rng, 2, 22);
            let p23: i8 = sample(&mut rng, 2, 23);
            let p24: i8 = sample(&mut rng, 2, 24);
            let p25: i8 = sample(&mut rng, 2, 25);
            let p26: i8 = sample(&mut rng, 2, 26);
            let p27: i8 = sample(&mut rng, 2, 27);
            let p28: i8 = sample(&mut rng, 2, 28);
            let p29: i8 = sample(&mut rng, 2, 29);
            let p30: i8 = sample(&mut rng, 2, 30);
            let p31: i8 = sample(&mut rng, 2, 31);
            let got = hr::_mm256_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            assert_eq!(
                bytes(&got),
                &[
                    172, 101, 30, 215, 144, 73, 2, 187, 116, 45, 230, 159, 88, 17, 202, 131, 60,
                    245, 174, 103, 32, 217, 146, 75, 4, 189, 118, 47, 232, 161, 90, 19
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_setr_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i8 = sample(&mut rng, 0, 0);
            let p1: i8 = sample(&mut rng, 0, 1);
            let p2: i8 = sample(&mut rng, 0, 2);
            let p3: i8 = sample(&mut rng, 0, 3);
            let p4: i8 = sample(&mut rng, 0, 4);
            let p5: i8 = sample(&mut rng, 0, 5);
            let p6: i8 = sample(&mut rng, 0, 6);
            let p7: i8 = sample(&mut rng, 0, 7);
            let p8: i8 = sample(&mut rng, 0, 8);
            let p9: i8 = sample(&mut rng, 0, 9);
            let p10: i8 = sample(&mut rng, 0, 10);
            let p11: i8 = sample(&mut rng, 0, 11);
            let p12: i8 = sample(&mut rng, 0, 12);
            let p13: i8 = sample(&mut rng, 0, 13);
            let p14: i8 = sample(&mut rng, 0, 14);
            let p15: i8 = sample(&mut rng, 0, 15);
            let p16: i8 = sample(&mut rng, 0, 16);
            let p17: i8 = sample(&mut rng, 0, 17);
            let p18: i8 = sample(&mut rng, 0, 18);
            let p19: i8 = sample(&mut rng, 0, 19);
            let p20: i8 = sample(&mut rng, 0, 20);
            let p21: i8 = sample(&mut rng, 0, 21);
            let p22: i8 = sample(&mut rng, 0, 22);
            let p23: i8 = sample(&mut rng, 0, 23);
            let p24: i8 = sample(&mut rng, 0, 24);
            let p25: i8 = sample(&mut rng, 0, 25);
            let p26: i8 = sample(&mut rng, 0, 26);
            let p27: i8 = sample(&mut rng, 0, 27);
            let p28: i8 = sample(&mut rng, 0, 28);
            let p29: i8 = sample(&mut rng, 0, 29);
            let p30: i8 = sample(&mut rng, 0, 30);
            let p31: i8 = sample(&mut rng, 0, 31);
            let got = hr::_mm256_setr_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 1, 0);
            let p1: i8 = sample(&mut rng, 1, 1);
            let p2: i8 = sample(&mut rng, 1, 2);
            let p3: i8 = sample(&mut rng, 1, 3);
            let p4: i8 = sample(&mut rng, 1, 4);
            let p5: i8 = sample(&mut rng, 1, 5);
            let p6: i8 = sample(&mut rng, 1, 6);
            let p7: i8 = sample(&mut rng, 1, 7);
            let p8: i8 = sample(&mut rng, 1, 8);
            let p9: i8 = sample(&mut rng, 1, 9);
            let p10: i8 = sample(&mut rng, 1, 10);
            let p11: i8 = sample(&mut rng, 1, 11);
            let p12: i8 = sample(&mut rng, 1, 12);
            let p13: i8 = sample(&mut rng, 1, 13);
            let p14: i8 = sample(&mut rng, 1, 14);
            let p15: i8 = sample(&mut rng, 1, 15);
            let p16: i8 = sample(&mut rng, 1, 16);
            let p17: i8 = sample(&mut rng, 1, 17);
            let p18: i8 = sample(&mut rng, 1, 18);
            let p19: i8 = sample(&mut rng, 1, 19);
            let p20: i8 = sample(&mut rng, 1, 20);
            let p21: i8 = sample(&mut rng, 1, 21);
            let p22: i8 = sample(&mut rng, 1, 22);
            let p23: i8 = sample(&mut rng, 1, 23);
            let p24: i8 = sample(&mut rng, 1, 24);
            let p25: i8 = sample(&mut rng, 1, 25);
            let p26: i8 = sample(&mut rng, 1, 26);
            let p27: i8 = sample(&mut rng, 1, 27);
            let p28: i8 = sample(&mut rng, 1, 28);
            let p29: i8 = sample(&mut rng, 1, 29);
            let p30: i8 = sample(&mut rng, 1, 30);
            let p31: i8 = sample(&mut rng, 1, 31);
            let got = hr::_mm256_setr_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            assert_eq!(
                bytes(&got),
                &[
                    255, 128, 0, 0, 1, 127, 0, 127, 128, 0, 254, 255, 255, 1, 255, 128, 255, 128,
                    0, 0, 1, 127, 0, 127, 128, 0, 254, 255, 255, 1, 255, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 2, 0);
            let p1: i8 = sample(&mut rng, 2, 1);
            let p2: i8 = sample(&mut rng, 2, 2);
            let p3: i8 = sample(&mut rng, 2, 3);
            let p4: i8 = sample(&mut rng, 2, 4);
            let p5: i8 = sample(&mut rng, 2, 5);
            let p6: i8 = sample(&mut rng, 2, 6);
            let p7: i8 = sample(&mut rng, 2, 7);
            let p8: i8 = sample(&mut rng, 2, 8);
            let p9: i8 = sample(&mut rng, 2, 9);
            let p10: i8 = sample(&mut rng, 2, 10);
            let p11: i8 = sample(&mut rng, 2, 11);
            let p12: i8 = sample(&mut rng, 2, 12);
            let p13: i8 = sample(&mut rng, 2, 13);
            let p14: i8 = sample(&mut rng, 2, 14);
            let p15: i8 = sample(&mut rng, 2, 15);
            let p16: i8 = sample(&mut rng, 2, 16);
            let p17: i8 = sample(&mut rng, 2, 17);
            let p18: i8 = sample(&mut rng, 2, 18);
            let p19: i8 = sample(&mut rng, 2, 19);
            let p20: i8 = sample(&mut rng, 2, 20);
            let p21: i8 = sample(&mut rng, 2, 21);
            let p22: i8 = sample(&mut rng, 2, 22);
            let p23: i8 = sample(&mut rng, 2, 23);
            let p24: i8 = sample(&mut rng, 2, 24);
            let p25: i8 = sample(&mut rng, 2, 25);
            let p26: i8 = sample(&mut rng, 2, 26);
            let p27: i8 = sample(&mut rng, 2, 27);
            let p28: i8 = sample(&mut rng, 2, 28);
            let p29: i8 = sample(&mut rng, 2, 29);
            let p30: i8 = sample(&mut rng, 2, 30);
            let p31: i8 = sample(&mut rng, 2, 31);
            let got = hr::_mm256_setr_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31,
            );
            assert_eq!(
                bytes(&got),
                &[
                    19, 90, 161, 232, 47, 118, 189, 4, 75, 146, 217, 32, 103, 174, 245, 60, 131,
                    202, 17, 88, 159, 230, 45, 116, 187, 2, 73, 144, 215, 30, 101, 172
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_setzero_si256() {
    unsafe {
        {
            let got = hr::_mm256_setzero_si256();
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let got = hr::_mm256_setzero_si256();
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let got = hr::_mm256_setzero_si256();
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_shuffle_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_shuffle_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_shuffle_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 127, 255, 255, 0, 255, 0, 255, 255, 0, 0, 127, 0, 0, 255, 0, 0, 127, 255,
                    255, 0, 255, 0, 255, 255, 0, 0, 127, 0, 0, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_shuffle_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    133, 62, 0, 0, 0, 130, 59, 244, 0, 0, 0, 0, 241, 170, 19, 0, 0, 0, 0, 176, 105,
                    210, 0, 0, 0, 0, 31, 136, 65, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_slli_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let got = hr::_mm256_slli_epi16::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let got = hr::_mm256_slli_epi16::<31>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let got = hr::_mm256_slli_epi16::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_slli_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let got = hr::_mm256_slli_epi32::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let got = hr::_mm256_slli_epi32::<31>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 128, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 128, 0, 0, 0,
                    128, 0, 0, 0, 0, 0, 0, 0, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let got = hr::_mm256_slli_epi32::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_srai_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let got = hr::_mm256_srai_epi16::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let got = hr::_mm256_srai_epi16::<31>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 255, 255,
                    0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 0, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let got = hr::_mm256_srai_epi16::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 255, 255, 255,
                    255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 255, 255
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_srai_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let got = hr::_mm256_srai_epi32::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let got = hr::_mm256_srai_epi32::<31>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let got = hr::_mm256_srai_epi32::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255,
                    255, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_storeu_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let mut p0: Buffer = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            hr::_mm256_storeu_si256(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..32],
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
            assert_eq!(
                &p0.0[32..],
                &(0..192).map(|i| fixture_byte(0, 0, i)).collect::<Vec<_>>()[32..]
            );
        }
        {
            let mut p0: Buffer = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            hr::_mm256_storeu_si256(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..32],
                &[
                    128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255, 0, 128, 255,
                    127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255, 0
                ],
                "Intel fixture 1"
            );
            assert_eq!(
                &p0.0[32..],
                &(0..192).map(|i| fixture_byte(1, 0, i)).collect::<Vec<_>>()[32..]
            );
        }
        {
            let mut p0: Buffer = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            hr::_mm256_storeu_si256(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..32],
                &[
                    90, 127, 164, 201, 238, 19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133, 170,
                    207, 244, 25, 62, 99, 136, 173, 210, 247, 28, 65, 102, 139, 176, 213
                ],
                "Intel fixture 2"
            );
            assert_eq!(
                &p0.0[32..],
                &(0..192).map(|i| fixture_byte(2, 0, i)).collect::<Vec<_>>()[32..]
            );
        }
    }
}

pub fn _mm256_sub_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_sub_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_sub_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    127, 0, 129, 127, 254, 255, 0, 2, 128, 255, 127, 2, 130, 128, 255, 126, 127, 0,
                    129, 127, 254, 255, 0, 2, 128, 255, 127, 2, 130, 128, 255, 126
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_sub_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    185, 184, 185, 184, 185, 184, 185, 185, 185, 184, 185, 184, 185, 185, 185, 184,
                    185, 184, 185, 184, 185, 185, 185, 184, 185, 184, 185, 185, 185, 184, 185, 184
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_sub_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_sub_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_sub_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    127, 0, 129, 127, 254, 255, 255, 1, 128, 255, 126, 2, 130, 128, 254, 126, 127,
                    0, 129, 127, 254, 255, 255, 1, 128, 255, 126, 2, 130, 128, 254, 126
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_sub_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    185, 184, 184, 184, 185, 184, 185, 185, 185, 184, 184, 184, 185, 185, 185, 184,
                    185, 184, 184, 184, 185, 185, 184, 184, 185, 184, 184, 185, 185, 184, 184, 184
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_testz_si256() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_testz_si256(p0, p1);
            assert_eq!(bytes(&got), &[1, 0, 0, 0], "Intel fixture 0");
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_testz_si256(p0, p1);
            assert_eq!(bytes(&got), &[0, 0, 0, 0], "Intel fixture 1");
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_testz_si256(p0, p1);
            assert_eq!(bytes(&got), &[0, 0, 0, 0], "Intel fixture 2");
        }
    }
}

pub fn _mm256_unpackhi_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_unpackhi_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_unpackhi_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 0, 0, 1, 255, 0, 128, 254, 1, 128, 127, 255, 254, 127, 255, 0, 128, 0, 0,
                    1, 255, 0, 128, 254, 1, 128, 127, 255, 254, 127, 255, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_unpackhi_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    59, 96, 130, 167, 133, 170, 204, 241, 207, 244, 22, 59, 25, 62, 96, 133, 139,
                    176, 210, 247, 213, 250, 28, 65, 31, 68, 102, 139, 105, 142, 176, 213
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_unpackhi_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_unpackhi_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_unpackhi_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 0, 255, 0, 0, 1, 128, 254, 1, 128, 254, 127, 127, 255, 255, 0, 128, 0,
                    255, 0, 0, 1, 128, 254, 1, 128, 254, 127, 127, 255, 255, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_unpackhi_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    59, 96, 133, 170, 130, 167, 204, 241, 207, 244, 25, 62, 22, 59, 96, 133, 139,
                    176, 213, 250, 210, 247, 28, 65, 31, 68, 105, 142, 102, 139, 176, 213
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_unpackhi_epi64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_unpackhi_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_unpackhi_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 0, 255, 0, 1, 128, 254, 127, 0, 1, 128, 254, 127, 255, 255, 0, 128, 0,
                    255, 0, 1, 128, 254, 127, 0, 1, 128, 254, 127, 255, 255, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_unpackhi_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    59, 96, 133, 170, 207, 244, 25, 62, 130, 167, 204, 241, 22, 59, 96, 133, 139,
                    176, 213, 250, 31, 68, 105, 142, 210, 247, 28, 65, 102, 139, 176, 213
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_unpackhi_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_unpackhi_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_unpackhi_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 0, 0, 1, 255, 128, 0, 254, 1, 127, 128, 255, 254, 255, 127, 0, 128, 0, 0,
                    1, 255, 128, 0, 254, 1, 127, 128, 255, 254, 255, 127, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_unpackhi_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    59, 130, 96, 167, 133, 204, 170, 241, 207, 22, 244, 59, 25, 96, 62, 133, 139,
                    210, 176, 247, 213, 28, 250, 65, 31, 102, 68, 139, 105, 176, 142, 213
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_unpacklo_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_unpacklo_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_unpacklo_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 128, 255, 0, 128, 127, 0, 255, 127, 1, 128, 0, 1, 0, 255, 255, 255,
                    128, 255, 0, 128, 127, 0, 255, 127, 1, 128, 0, 1, 0, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_unpacklo_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 90, 127, 93, 130, 164, 201, 167, 204, 238, 19, 241, 22, 56, 93, 99,
                    136, 170, 207, 173, 210, 244, 25, 247, 28, 62, 99, 65, 102, 136, 173
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_unpacklo_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_unpacklo_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_unpacklo_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 0, 128, 128, 255, 127, 0, 255, 127, 0, 1, 1, 128, 0, 255, 255, 255,
                    0, 128, 128, 255, 127, 0, 255, 127, 0, 1, 1, 128, 0, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_unpacklo_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 93, 130, 90, 127, 164, 201, 167, 204, 241, 22, 238, 19, 56, 93, 99,
                    136, 173, 210, 170, 207, 244, 25, 247, 28, 65, 102, 62, 99, 136, 173
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_unpacklo_epi64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_unpacklo_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_unpacklo_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 0, 128, 255, 127, 0, 1, 128, 255, 127, 0, 1, 128, 0, 255, 255, 255,
                    0, 128, 255, 127, 0, 1, 128, 255, 127, 0, 1, 128, 0, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_unpacklo_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 93, 130, 167, 204, 241, 22, 90, 127, 164, 201, 238, 19, 56, 93, 99,
                    136, 173, 210, 247, 28, 65, 102, 170, 207, 244, 25, 62, 99, 136, 173
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm256_unpacklo_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m256i = sample(&mut rng, 0, 0);
            let p1: hr::__m256i = sample(&mut rng, 0, 1);
            let got = hr::_mm256_unpacklo_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 1, 0);
            let p1: hr::__m256i = sample(&mut rng, 1, 1);
            let got = hr::_mm256_unpacklo_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    255, 128, 255, 255, 0, 127, 128, 0, 255, 1, 127, 128, 0, 0, 1, 255, 255, 128,
                    255, 255, 0, 127, 128, 0, 255, 1, 127, 128, 0, 0, 1, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m256i = sample(&mut rng, 2, 0);
            let p1: hr::__m256i = sample(&mut rng, 2, 1);
            let got = hr::_mm256_unpacklo_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    19, 90, 56, 127, 93, 164, 130, 201, 167, 238, 204, 19, 241, 56, 22, 93, 99,
                    170, 136, 207, 173, 244, 210, 25, 247, 62, 28, 99, 65, 136, 102, 173
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_add_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m512i = sample(&mut rng, 0, 0);
            let p1: hr::__m512i = sample(&mut rng, 0, 1);
            let got = hr::_mm512_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 1, 0);
            let p1: hr::__m512i = sample(&mut rng, 1, 1);
            let got = hr::_mm512_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    127, 255, 128, 128, 0, 0, 1, 0, 128, 1, 127, 255, 128, 127, 254, 128, 127, 255,
                    128, 128, 0, 0, 1, 0, 128, 1, 127, 255, 128, 127, 254, 128, 127, 255, 128, 128,
                    0, 0, 1, 0, 128, 1, 127, 255, 128, 127, 254, 128, 127, 255, 128, 128, 0, 0, 1,
                    0, 128, 1, 127, 255, 128, 127, 254, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 2, 0);
            let p1: hr::__m512i = sample(&mut rng, 2, 1);
            let got = hr::_mm512_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    109, 183, 1, 76, 149, 224, 41, 116, 189, 7, 82, 156, 229, 47, 122, 195, 13, 88,
                    162, 236, 53, 128, 201, 19, 93, 168, 242, 59, 133, 207, 25, 100, 173, 248, 65,
                    140, 213, 31, 106, 180, 253, 71, 145, 219, 37, 112, 186, 3, 77, 151, 225, 43,
                    117, 192, 9, 84, 157, 231, 49, 124, 197, 15, 90, 163
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_loadu_si512() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: Buffer = sample(&mut rng, 0, 0);
            let got = hr::_mm512_loadu_si512(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: Buffer = sample(&mut rng, 1, 0);
            let got = hr::_mm512_loadu_si512(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255,
                    0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255, 0, 128,
                    255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255, 0, 128, 255, 127,
                    0, 1, 128, 0, 255, 0, 1, 128, 254, 127
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: Buffer = sample(&mut rng, 2, 0);
            let got = hr::_mm512_loadu_si512(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133, 170, 207, 244, 25, 62, 99,
                    136, 173, 210, 247, 28, 65, 102, 139, 176, 213, 250, 31, 68, 105, 142, 179,
                    216, 253, 34, 71, 108, 145, 182, 219, 0, 37, 74, 111, 148, 185, 222, 3, 40, 77,
                    114, 151, 188, 225, 6, 43, 80, 117, 154, 191, 228, 9, 46
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_madd_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m512i = sample(&mut rng, 0, 0);
            let p1: hr::__m512i = sample(&mut rng, 0, 1);
            let got = hr::_mm512_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 1, 0);
            let p1: hr::__m512i = sample(&mut rng, 1, 1);
            let got = hr::_mm512_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    128, 128, 192, 255, 255, 255, 255, 191, 128, 1, 255, 255, 129, 253, 191, 0,
                    128, 128, 192, 255, 255, 255, 255, 191, 128, 1, 255, 255, 129, 253, 191, 0,
                    128, 128, 192, 255, 255, 255, 255, 191, 128, 1, 255, 255, 129, 253, 191, 0,
                    128, 128, 192, 255, 255, 255, 255, 191, 128, 1, 255, 255, 129, 253, 191, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 2, 0);
            let p1: hr::__m512i = sample(&mut rng, 2, 1);
            let got = hr::_mm512_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    66, 172, 146, 54, 250, 57, 91, 4, 242, 113, 122, 227, 42, 5, 172, 223, 162, 80,
                    253, 17, 90, 200, 73, 234, 82, 115, 57, 1, 138, 62, 191, 243, 2, 92, 122, 9,
                    186, 11, 73, 224, 178, 22, 54, 224, 234, 56, 112, 10, 98, 151, 243, 241, 26,
                    16, 14, 1, 18, 83, 67, 235, 74, 241, 131, 16
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_maddubs_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m512i = sample(&mut rng, 0, 0);
            let p1: hr::__m512i = sample(&mut rng, 0, 1);
            let got = hr::_mm512_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 1, 0);
            let p1: hr::__m512i = sample(&mut rng, 1, 1);
            let got = hr::_mm512_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 128, 0, 0, 127, 193, 255, 255, 0, 0, 128, 128, 255, 255, 2, 255, 0, 128, 0,
                    0, 127, 193, 255, 255, 0, 0, 128, 128, 255, 255, 2, 255, 0, 128, 0, 0, 127,
                    193, 255, 255, 0, 0, 128, 128, 255, 255, 2, 255, 0, 128, 0, 0, 127, 193, 255,
                    255, 0, 0, 128, 128, 255, 255, 2, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 2, 0);
            let p1: hr::__m512i = sample(&mut rng, 2, 1);
            let got = hr::_mm512_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    118, 34, 166, 194, 102, 3, 182, 60, 150, 193, 6, 219, 6, 74, 150, 235, 182,
                    196, 102, 12, 166, 70, 118, 192, 214, 224, 198, 86, 70, 237, 86, 199, 246, 21,
                    38, 81, 230, 191, 54, 231, 22, 29, 134, 239, 134, 202, 22, 32, 54, 18, 230,
                    191, 38, 238, 246, 36, 86, 242, 70, 206, 198, 42, 214, 23
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_sad_epu8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m512i = sample(&mut rng, 0, 0);
            let p1: hr::__m512i = sample(&mut rng, 0, 1);
            let got = hr::_mm512_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 1, 0);
            let p1: hr::__m512i = sample(&mut rng, 1, 1);
            let got = hr::_mm512_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    123, 3, 0, 0, 0, 0, 0, 0, 123, 3, 0, 0, 0, 0, 0, 0, 123, 3, 0, 0, 0, 0, 0, 0,
                    123, 3, 0, 0, 0, 0, 0, 0, 123, 3, 0, 0, 0, 0, 0, 0, 123, 3, 0, 0, 0, 0, 0, 0,
                    123, 3, 0, 0, 0, 0, 0, 0, 123, 3, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 2, 0);
            let p1: hr::__m512i = sample(&mut rng, 2, 1);
            let got = hr::_mm512_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[
                    28, 3, 0, 0, 0, 0, 0, 0, 28, 3, 0, 0, 0, 0, 0, 0, 28, 3, 0, 0, 0, 0, 0, 0, 28,
                    3, 0, 0, 0, 0, 0, 0, 28, 3, 0, 0, 0, 0, 0, 0, 142, 3, 0, 0, 0, 0, 0, 0, 28, 3,
                    0, 0, 0, 0, 0, 0, 28, 3, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_set1_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i16 = sample(&mut rng, 0, 0);
            let got = hr::_mm512_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: i16 = sample(&mut rng, 1, 0);
            let got = hr::_mm512_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
                    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: i16 = sample(&mut rng, 2, 0);
            let got = hr::_mm512_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[
                    19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56,
                    19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56,
                    19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56,
                    19, 56, 19, 56
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_set_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i32 = sample(&mut rng, 0, 0);
            let p1: i32 = sample(&mut rng, 0, 1);
            let p2: i32 = sample(&mut rng, 0, 2);
            let p3: i32 = sample(&mut rng, 0, 3);
            let p4: i32 = sample(&mut rng, 0, 4);
            let p5: i32 = sample(&mut rng, 0, 5);
            let p6: i32 = sample(&mut rng, 0, 6);
            let p7: i32 = sample(&mut rng, 0, 7);
            let p8: i32 = sample(&mut rng, 0, 8);
            let p9: i32 = sample(&mut rng, 0, 9);
            let p10: i32 = sample(&mut rng, 0, 10);
            let p11: i32 = sample(&mut rng, 0, 11);
            let p12: i32 = sample(&mut rng, 0, 12);
            let p13: i32 = sample(&mut rng, 0, 13);
            let p14: i32 = sample(&mut rng, 0, 14);
            let p15: i32 = sample(&mut rng, 0, 15);
            let got = hr::_mm512_set_epi32(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 1, 0);
            let p1: i32 = sample(&mut rng, 1, 1);
            let p2: i32 = sample(&mut rng, 1, 2);
            let p3: i32 = sample(&mut rng, 1, 3);
            let p4: i32 = sample(&mut rng, 1, 4);
            let p5: i32 = sample(&mut rng, 1, 5);
            let p6: i32 = sample(&mut rng, 1, 6);
            let p7: i32 = sample(&mut rng, 1, 7);
            let p8: i32 = sample(&mut rng, 1, 8);
            let p9: i32 = sample(&mut rng, 1, 9);
            let p10: i32 = sample(&mut rng, 1, 10);
            let p11: i32 = sample(&mut rng, 1, 11);
            let p12: i32 = sample(&mut rng, 1, 12);
            let p13: i32 = sample(&mut rng, 1, 13);
            let p14: i32 = sample(&mut rng, 1, 14);
            let p15: i32 = sample(&mut rng, 1, 15);
            let got = hr::_mm512_set_epi32(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            assert_eq!(
                bytes(&got),
                &[
                    128, 254, 127, 255, 255, 0, 1, 128, 1, 128, 0, 255, 255, 127, 0, 1, 255, 0,
                    128, 255, 254, 127, 255, 255, 0, 1, 128, 254, 128, 0, 255, 0, 127, 0, 1, 128,
                    0, 128, 255, 127, 127, 255, 255, 0, 1, 128, 254, 127, 0, 255, 0, 1, 0, 1, 128,
                    0, 128, 255, 127, 0, 255, 255, 0, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 2, 0);
            let p1: i32 = sample(&mut rng, 2, 1);
            let p2: i32 = sample(&mut rng, 2, 2);
            let p3: i32 = sample(&mut rng, 2, 3);
            let p4: i32 = sample(&mut rng, 2, 4);
            let p5: i32 = sample(&mut rng, 2, 5);
            let p6: i32 = sample(&mut rng, 2, 6);
            let p7: i32 = sample(&mut rng, 2, 7);
            let p8: i32 = sample(&mut rng, 2, 8);
            let p9: i32 = sample(&mut rng, 2, 9);
            let p10: i32 = sample(&mut rng, 2, 10);
            let p11: i32 = sample(&mut rng, 2, 11);
            let p12: i32 = sample(&mut rng, 2, 12);
            let p13: i32 = sample(&mut rng, 2, 13);
            let p14: i32 = sample(&mut rng, 2, 14);
            let p15: i32 = sample(&mut rng, 2, 15);
            let got = hr::_mm512_set_epi32(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            assert_eq!(
                bytes(&got),
                &[
                    60, 97, 134, 171, 245, 26, 63, 100, 174, 211, 248, 29, 103, 140, 177, 214, 32,
                    69, 106, 143, 217, 254, 35, 72, 146, 183, 220, 1, 75, 112, 149, 186, 4, 41, 78,
                    115, 189, 226, 7, 44, 118, 155, 192, 229, 47, 84, 121, 158, 232, 13, 50, 87,
                    161, 198, 235, 16, 90, 127, 164, 201, 19, 56, 93, 130
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_set_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i8 = sample(&mut rng, 0, 0);
            let p1: i8 = sample(&mut rng, 0, 1);
            let p2: i8 = sample(&mut rng, 0, 2);
            let p3: i8 = sample(&mut rng, 0, 3);
            let p4: i8 = sample(&mut rng, 0, 4);
            let p5: i8 = sample(&mut rng, 0, 5);
            let p6: i8 = sample(&mut rng, 0, 6);
            let p7: i8 = sample(&mut rng, 0, 7);
            let p8: i8 = sample(&mut rng, 0, 8);
            let p9: i8 = sample(&mut rng, 0, 9);
            let p10: i8 = sample(&mut rng, 0, 10);
            let p11: i8 = sample(&mut rng, 0, 11);
            let p12: i8 = sample(&mut rng, 0, 12);
            let p13: i8 = sample(&mut rng, 0, 13);
            let p14: i8 = sample(&mut rng, 0, 14);
            let p15: i8 = sample(&mut rng, 0, 15);
            let p16: i8 = sample(&mut rng, 0, 16);
            let p17: i8 = sample(&mut rng, 0, 17);
            let p18: i8 = sample(&mut rng, 0, 18);
            let p19: i8 = sample(&mut rng, 0, 19);
            let p20: i8 = sample(&mut rng, 0, 20);
            let p21: i8 = sample(&mut rng, 0, 21);
            let p22: i8 = sample(&mut rng, 0, 22);
            let p23: i8 = sample(&mut rng, 0, 23);
            let p24: i8 = sample(&mut rng, 0, 24);
            let p25: i8 = sample(&mut rng, 0, 25);
            let p26: i8 = sample(&mut rng, 0, 26);
            let p27: i8 = sample(&mut rng, 0, 27);
            let p28: i8 = sample(&mut rng, 0, 28);
            let p29: i8 = sample(&mut rng, 0, 29);
            let p30: i8 = sample(&mut rng, 0, 30);
            let p31: i8 = sample(&mut rng, 0, 31);
            let p32: i8 = sample(&mut rng, 0, 32);
            let p33: i8 = sample(&mut rng, 0, 33);
            let p34: i8 = sample(&mut rng, 0, 34);
            let p35: i8 = sample(&mut rng, 0, 35);
            let p36: i8 = sample(&mut rng, 0, 36);
            let p37: i8 = sample(&mut rng, 0, 37);
            let p38: i8 = sample(&mut rng, 0, 38);
            let p39: i8 = sample(&mut rng, 0, 39);
            let p40: i8 = sample(&mut rng, 0, 40);
            let p41: i8 = sample(&mut rng, 0, 41);
            let p42: i8 = sample(&mut rng, 0, 42);
            let p43: i8 = sample(&mut rng, 0, 43);
            let p44: i8 = sample(&mut rng, 0, 44);
            let p45: i8 = sample(&mut rng, 0, 45);
            let p46: i8 = sample(&mut rng, 0, 46);
            let p47: i8 = sample(&mut rng, 0, 47);
            let p48: i8 = sample(&mut rng, 0, 48);
            let p49: i8 = sample(&mut rng, 0, 49);
            let p50: i8 = sample(&mut rng, 0, 50);
            let p51: i8 = sample(&mut rng, 0, 51);
            let p52: i8 = sample(&mut rng, 0, 52);
            let p53: i8 = sample(&mut rng, 0, 53);
            let p54: i8 = sample(&mut rng, 0, 54);
            let p55: i8 = sample(&mut rng, 0, 55);
            let p56: i8 = sample(&mut rng, 0, 56);
            let p57: i8 = sample(&mut rng, 0, 57);
            let p58: i8 = sample(&mut rng, 0, 58);
            let p59: i8 = sample(&mut rng, 0, 59);
            let p60: i8 = sample(&mut rng, 0, 60);
            let p61: i8 = sample(&mut rng, 0, 61);
            let p62: i8 = sample(&mut rng, 0, 62);
            let p63: i8 = sample(&mut rng, 0, 63);
            let got = hr::_mm512_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31, p32, p33,
                p34, p35, p36, p37, p38, p39, p40, p41, p42, p43, p44, p45, p46, p47, p48, p49,
                p50, p51, p52, p53, p54, p55, p56, p57, p58, p59, p60, p61, p62, p63,
            );
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 1, 0);
            let p1: i8 = sample(&mut rng, 1, 1);
            let p2: i8 = sample(&mut rng, 1, 2);
            let p3: i8 = sample(&mut rng, 1, 3);
            let p4: i8 = sample(&mut rng, 1, 4);
            let p5: i8 = sample(&mut rng, 1, 5);
            let p6: i8 = sample(&mut rng, 1, 6);
            let p7: i8 = sample(&mut rng, 1, 7);
            let p8: i8 = sample(&mut rng, 1, 8);
            let p9: i8 = sample(&mut rng, 1, 9);
            let p10: i8 = sample(&mut rng, 1, 10);
            let p11: i8 = sample(&mut rng, 1, 11);
            let p12: i8 = sample(&mut rng, 1, 12);
            let p13: i8 = sample(&mut rng, 1, 13);
            let p14: i8 = sample(&mut rng, 1, 14);
            let p15: i8 = sample(&mut rng, 1, 15);
            let p16: i8 = sample(&mut rng, 1, 16);
            let p17: i8 = sample(&mut rng, 1, 17);
            let p18: i8 = sample(&mut rng, 1, 18);
            let p19: i8 = sample(&mut rng, 1, 19);
            let p20: i8 = sample(&mut rng, 1, 20);
            let p21: i8 = sample(&mut rng, 1, 21);
            let p22: i8 = sample(&mut rng, 1, 22);
            let p23: i8 = sample(&mut rng, 1, 23);
            let p24: i8 = sample(&mut rng, 1, 24);
            let p25: i8 = sample(&mut rng, 1, 25);
            let p26: i8 = sample(&mut rng, 1, 26);
            let p27: i8 = sample(&mut rng, 1, 27);
            let p28: i8 = sample(&mut rng, 1, 28);
            let p29: i8 = sample(&mut rng, 1, 29);
            let p30: i8 = sample(&mut rng, 1, 30);
            let p31: i8 = sample(&mut rng, 1, 31);
            let p32: i8 = sample(&mut rng, 1, 32);
            let p33: i8 = sample(&mut rng, 1, 33);
            let p34: i8 = sample(&mut rng, 1, 34);
            let p35: i8 = sample(&mut rng, 1, 35);
            let p36: i8 = sample(&mut rng, 1, 36);
            let p37: i8 = sample(&mut rng, 1, 37);
            let p38: i8 = sample(&mut rng, 1, 38);
            let p39: i8 = sample(&mut rng, 1, 39);
            let p40: i8 = sample(&mut rng, 1, 40);
            let p41: i8 = sample(&mut rng, 1, 41);
            let p42: i8 = sample(&mut rng, 1, 42);
            let p43: i8 = sample(&mut rng, 1, 43);
            let p44: i8 = sample(&mut rng, 1, 44);
            let p45: i8 = sample(&mut rng, 1, 45);
            let p46: i8 = sample(&mut rng, 1, 46);
            let p47: i8 = sample(&mut rng, 1, 47);
            let p48: i8 = sample(&mut rng, 1, 48);
            let p49: i8 = sample(&mut rng, 1, 49);
            let p50: i8 = sample(&mut rng, 1, 50);
            let p51: i8 = sample(&mut rng, 1, 51);
            let p52: i8 = sample(&mut rng, 1, 52);
            let p53: i8 = sample(&mut rng, 1, 53);
            let p54: i8 = sample(&mut rng, 1, 54);
            let p55: i8 = sample(&mut rng, 1, 55);
            let p56: i8 = sample(&mut rng, 1, 56);
            let p57: i8 = sample(&mut rng, 1, 57);
            let p58: i8 = sample(&mut rng, 1, 58);
            let p59: i8 = sample(&mut rng, 1, 59);
            let p60: i8 = sample(&mut rng, 1, 60);
            let p61: i8 = sample(&mut rng, 1, 61);
            let p62: i8 = sample(&mut rng, 1, 62);
            let p63: i8 = sample(&mut rng, 1, 63);
            let got = hr::_mm512_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31, p32, p33,
                p34, p35, p36, p37, p38, p39, p40, p41, p42, p43, p44, p45, p46, p47, p48, p49,
                p50, p51, p52, p53, p54, p55, p56, p57, p58, p59, p60, p61, p62, p63,
            );
            assert_eq!(
                bytes(&got),
                &[
                    128, 255, 1, 255, 255, 254, 0, 128, 127, 0, 127, 1, 0, 0, 128, 255, 128, 255,
                    1, 255, 255, 254, 0, 128, 127, 0, 127, 1, 0, 0, 128, 255, 128, 255, 1, 255,
                    255, 254, 0, 128, 127, 0, 127, 1, 0, 0, 128, 255, 128, 255, 1, 255, 255, 254,
                    0, 128, 127, 0, 127, 1, 0, 0, 128, 255
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 2, 0);
            let p1: i8 = sample(&mut rng, 2, 1);
            let p2: i8 = sample(&mut rng, 2, 2);
            let p3: i8 = sample(&mut rng, 2, 3);
            let p4: i8 = sample(&mut rng, 2, 4);
            let p5: i8 = sample(&mut rng, 2, 5);
            let p6: i8 = sample(&mut rng, 2, 6);
            let p7: i8 = sample(&mut rng, 2, 7);
            let p8: i8 = sample(&mut rng, 2, 8);
            let p9: i8 = sample(&mut rng, 2, 9);
            let p10: i8 = sample(&mut rng, 2, 10);
            let p11: i8 = sample(&mut rng, 2, 11);
            let p12: i8 = sample(&mut rng, 2, 12);
            let p13: i8 = sample(&mut rng, 2, 13);
            let p14: i8 = sample(&mut rng, 2, 14);
            let p15: i8 = sample(&mut rng, 2, 15);
            let p16: i8 = sample(&mut rng, 2, 16);
            let p17: i8 = sample(&mut rng, 2, 17);
            let p18: i8 = sample(&mut rng, 2, 18);
            let p19: i8 = sample(&mut rng, 2, 19);
            let p20: i8 = sample(&mut rng, 2, 20);
            let p21: i8 = sample(&mut rng, 2, 21);
            let p22: i8 = sample(&mut rng, 2, 22);
            let p23: i8 = sample(&mut rng, 2, 23);
            let p24: i8 = sample(&mut rng, 2, 24);
            let p25: i8 = sample(&mut rng, 2, 25);
            let p26: i8 = sample(&mut rng, 2, 26);
            let p27: i8 = sample(&mut rng, 2, 27);
            let p28: i8 = sample(&mut rng, 2, 28);
            let p29: i8 = sample(&mut rng, 2, 29);
            let p30: i8 = sample(&mut rng, 2, 30);
            let p31: i8 = sample(&mut rng, 2, 31);
            let p32: i8 = sample(&mut rng, 2, 32);
            let p33: i8 = sample(&mut rng, 2, 33);
            let p34: i8 = sample(&mut rng, 2, 34);
            let p35: i8 = sample(&mut rng, 2, 35);
            let p36: i8 = sample(&mut rng, 2, 36);
            let p37: i8 = sample(&mut rng, 2, 37);
            let p38: i8 = sample(&mut rng, 2, 38);
            let p39: i8 = sample(&mut rng, 2, 39);
            let p40: i8 = sample(&mut rng, 2, 40);
            let p41: i8 = sample(&mut rng, 2, 41);
            let p42: i8 = sample(&mut rng, 2, 42);
            let p43: i8 = sample(&mut rng, 2, 43);
            let p44: i8 = sample(&mut rng, 2, 44);
            let p45: i8 = sample(&mut rng, 2, 45);
            let p46: i8 = sample(&mut rng, 2, 46);
            let p47: i8 = sample(&mut rng, 2, 47);
            let p48: i8 = sample(&mut rng, 2, 48);
            let p49: i8 = sample(&mut rng, 2, 49);
            let p50: i8 = sample(&mut rng, 2, 50);
            let p51: i8 = sample(&mut rng, 2, 51);
            let p52: i8 = sample(&mut rng, 2, 52);
            let p53: i8 = sample(&mut rng, 2, 53);
            let p54: i8 = sample(&mut rng, 2, 54);
            let p55: i8 = sample(&mut rng, 2, 55);
            let p56: i8 = sample(&mut rng, 2, 56);
            let p57: i8 = sample(&mut rng, 2, 57);
            let p58: i8 = sample(&mut rng, 2, 58);
            let p59: i8 = sample(&mut rng, 2, 59);
            let p60: i8 = sample(&mut rng, 2, 60);
            let p61: i8 = sample(&mut rng, 2, 61);
            let p62: i8 = sample(&mut rng, 2, 62);
            let p63: i8 = sample(&mut rng, 2, 63);
            let got = hr::_mm512_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15, p16, p17,
                p18, p19, p20, p21, p22, p23, p24, p25, p26, p27, p28, p29, p30, p31, p32, p33,
                p34, p35, p36, p37, p38, p39, p40, p41, p42, p43, p44, p45, p46, p47, p48, p49,
                p50, p51, p52, p53, p54, p55, p56, p57, p58, p59, p60, p61, p62, p63,
            );
            assert_eq!(
                bytes(&got),
                &[
                    140, 69, 254, 183, 112, 41, 226, 155, 84, 13, 198, 127, 56, 241, 170, 99, 28,
                    213, 142, 71, 0, 185, 114, 43, 228, 157, 86, 15, 200, 129, 58, 243, 172, 101,
                    30, 215, 144, 73, 2, 187, 116, 45, 230, 159, 88, 17, 202, 131, 60, 245, 174,
                    103, 32, 217, 146, 75, 4, 189, 118, 47, 232, 161, 90, 19
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_setzero_si512() {
    unsafe {
        {
            let got = hr::_mm512_setzero_si512();
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let got = hr::_mm512_setzero_si512();
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 1"
            );
        }
        {
            let got = hr::_mm512_setzero_si512();
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm512_slli_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m512i = sample(&mut rng, 0, 0);
            let got = hr::_mm512_slli_epi32::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 1, 0);
            let got = hr::_mm512_slli_epi32::<31>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 128, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 128, 0, 0, 0,
                    128, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 128, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0,
                    128, 0, 0, 0, 128, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 128
                ],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m512i = sample(&mut rng, 2, 0);
            let got = hr::_mm512_slli_epi32::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_abs_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let got = hr::_mm_abs_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let got = hr::_mm_abs_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[1, 0, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 255, 127, 254, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let got = hr::_mm_abs_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[19, 56, 163, 125, 89, 51, 241, 22, 59, 96, 123, 85, 49, 11, 25, 62],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_add_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_add_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_add_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[127, 255, 127, 128, 0, 0, 0, 0, 128, 1, 127, 255, 128, 127, 253, 128],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_add_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[109, 183, 1, 76, 149, 224, 41, 116, 189, 7, 81, 156, 229, 47, 121, 195],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_add_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[127, 255, 128, 128, 0, 0, 1, 0, 128, 1, 127, 255, 128, 127, 254, 128],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_add_epi32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[109, 183, 1, 76, 149, 224, 41, 116, 189, 7, 82, 156, 229, 47, 122, 195],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_add_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_add_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_add_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[127, 254, 127, 128, 0, 255, 0, 0, 128, 1, 127, 254, 128, 127, 253, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_add_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[109, 183, 1, 75, 149, 223, 41, 115, 189, 7, 81, 155, 229, 47, 121, 195],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_and_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_and_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_and_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[128, 255, 0, 0, 1, 0, 0, 1, 0, 0, 128, 0, 1, 128, 254, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_and_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[18, 56, 4, 128, 166, 0, 48, 20, 2, 32, 132, 160, 6, 48, 0, 4],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_andnot_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_andnot_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_andnot_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 127, 0, 0, 128, 0, 254, 0, 1, 0, 254, 126, 127, 1, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_andnot_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[72, 71, 160, 73, 72, 19, 8, 73, 128, 135, 72, 81, 16, 11, 96, 129],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_avg_epu8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_avg_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_avg_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[192, 255, 64, 64, 128, 128, 0, 128, 64, 1, 192, 127, 64, 192, 255, 64],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_avg_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[55, 92, 129, 166, 203, 112, 149, 58, 95, 132, 169, 206, 115, 152, 61, 98],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_blendv_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let p2: hr::__m128i = sample(&mut rng, 0, 2);
            let got = hr::_mm_blendv_epi8(p0, p1, p2);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let p2: hr::__m128i = sample(&mut rng, 1, 2);
            let got = hr::_mm_blendv_epi8(p0, p1, p2);
            assert_eq!(
                bytes(&got),
                &[255, 255, 127, 128, 1, 127, 0, 255, 0, 0, 128, 254, 1, 255, 255, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let p2: hr::__m128i = sample(&mut rng, 2, 2);
            let got = hr::_mm_blendv_epi8(p0, p1, p2);
            assert_eq!(
                bytes(&got),
                &[90, 127, 164, 130, 167, 204, 241, 93, 130, 167, 133, 170, 207, 59, 96, 133],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_clmulepi64_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_clmulepi64_si128::<0>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_clmulepi64_si128::<16>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 255, 127, 85, 213, 0, 192, 255, 254, 127, 65, 213, 213, 255, 0, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_clmulepi64_si128::<255>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[246, 156, 244, 168, 134, 76, 120, 112, 49, 32, 127, 187, 144, 196, 194, 31],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_cmpeq_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_cmpeq_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_cmpeq_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_cmpeq_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_cvtsi128_si32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let got = hr::_mm_cvtsi128_si32(p0);
            assert_eq!(bytes(&got), &[0, 0, 0, 0], "Intel fixture 0");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let got = hr::_mm_cvtsi128_si32(p0);
            assert_eq!(bytes(&got), &[255, 255, 0, 128], "Intel fixture 1");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let got = hr::_mm_cvtsi128_si32(p0);
            assert_eq!(bytes(&got), &[19, 56, 93, 130], "Intel fixture 2");
        }
    }
}

pub fn _mm_cvtsi32_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i32 = sample(&mut rng, 0, 0);
            let got = hr::_mm_cvtsi32_si128(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 1, 0);
            let got = hr::_mm_cvtsi32_si128(p0);
            assert_eq!(
                bytes(&got),
                &[255, 255, 0, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 2, 0);
            let got = hr::_mm_cvtsi32_si128(p0);
            assert_eq!(
                bytes(&got),
                &[19, 56, 93, 130, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_extract_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let got = hr::_mm_extract_epi32::<0>(p0);
            assert_eq!(bytes(&got), &[0, 0, 0, 0], "Intel fixture 0");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let got = hr::_mm_extract_epi32::<2>(p0);
            assert_eq!(bytes(&got), &[128, 0, 255, 0], "Intel fixture 1");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let got = hr::_mm_extract_epi32::<3>(p0);
            assert_eq!(bytes(&got), &[207, 244, 25, 62], "Intel fixture 2");
        }
    }
}

pub fn _mm_load_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: Buffer = sample(&mut rng, 0, 0);
            let got = hr::_mm_load_si128(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: Buffer = sample(&mut rng, 1, 0);
            let got = hr::_mm_load_si128(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: Buffer = sample(&mut rng, 2, 0);
            let got = hr::_mm_load_si128(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133, 170, 207, 244, 25, 62],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_loadu_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: Buffer = sample(&mut rng, 0, 0);
            let got = hr::_mm_loadu_si128(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: Buffer = sample(&mut rng, 1, 0);
            let got = hr::_mm_loadu_si128(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[255, 255, 0, 128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: Buffer = sample(&mut rng, 2, 0);
            let got = hr::_mm_loadu_si128(p0.0.as_ptr().cast());
            assert_eq!(
                bytes(&got),
                &[19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133, 170, 207, 244, 25, 62],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_madd_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[128, 128, 192, 255, 255, 255, 255, 191, 128, 1, 255, 255, 129, 253, 191, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_madd_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[66, 172, 146, 54, 250, 57, 91, 4, 242, 113, 122, 227, 42, 5, 172, 223],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_maddubs_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 128, 0, 0, 127, 193, 255, 255, 0, 0, 128, 128, 255, 255, 2, 255],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_maddubs_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[118, 34, 166, 194, 102, 3, 182, 60, 150, 193, 6, 219, 6, 74, 150, 235],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_min_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_min_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_min_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[128, 255, 0, 128, 1, 128, 0, 255, 128, 0, 128, 254, 1, 128, 255, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_min_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[19, 56, 93, 130, 167, 204, 241, 22, 130, 167, 133, 170, 207, 244, 96, 133],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_or_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_or_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_or_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[255, 255, 127, 128, 255, 255, 0, 255, 128, 1, 255, 254, 127, 255, 255, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_or_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[91, 127, 253, 203, 239, 223, 249, 95, 187, 231, 205, 251, 223, 255, 121, 191],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_packus_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_packus_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_packus_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 255, 255, 128, 255, 0, 255, 0, 127, 0, 0, 255, 0, 0, 255],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_packus_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 255, 255, 0, 0, 255, 0],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_sad_epu8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[123, 3, 0, 0, 0, 0, 0, 0, 123, 3, 0, 0, 0, 0, 0, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_sad_epu8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[28, 3, 0, 0, 0, 0, 0, 0, 28, 3, 0, 0, 0, 0, 0, 0],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_set1_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i16 = sample(&mut rng, 0, 0);
            let got = hr::_mm_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: i16 = sample(&mut rng, 1, 0);
            let got = hr::_mm_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
                "Intel fixture 1"
            );
        }
        {
            let p0: i16 = sample(&mut rng, 2, 0);
            let got = hr::_mm_set1_epi16(p0);
            assert_eq!(
                bytes(&got),
                &[19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56, 19, 56],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_set1_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i8 = sample(&mut rng, 0, 0);
            let got = hr::_mm_set1_epi8(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 1, 0);
            let got = hr::_mm_set1_epi8(p0);
            assert_eq!(
                bytes(&got),
                &[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
                "Intel fixture 1"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 2, 0);
            let got = hr::_mm_set1_epi8(p0);
            assert_eq!(
                bytes(&got),
                &[19, 19, 19, 19, 19, 19, 19, 19, 19, 19, 19, 19, 19, 19, 19, 19],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_set_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i32 = sample(&mut rng, 0, 0);
            let p1: i32 = sample(&mut rng, 0, 1);
            let p2: i32 = sample(&mut rng, 0, 2);
            let p3: i32 = sample(&mut rng, 0, 3);
            let got = hr::_mm_set_epi32(p0, p1, p2, p3);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 1, 0);
            let p1: i32 = sample(&mut rng, 1, 1);
            let p2: i32 = sample(&mut rng, 1, 2);
            let p3: i32 = sample(&mut rng, 1, 3);
            let got = hr::_mm_set_epi32(p0, p1, p2, p3);
            assert_eq!(
                bytes(&got),
                &[0, 255, 0, 1, 0, 1, 128, 0, 128, 255, 127, 0, 255, 255, 0, 128],
                "Intel fixture 1"
            );
        }
        {
            let p0: i32 = sample(&mut rng, 2, 0);
            let p1: i32 = sample(&mut rng, 2, 1);
            let p2: i32 = sample(&mut rng, 2, 2);
            let p3: i32 = sample(&mut rng, 2, 3);
            let got = hr::_mm_set_epi32(p0, p1, p2, p3);
            assert_eq!(
                bytes(&got),
                &[232, 13, 50, 87, 161, 198, 235, 16, 90, 127, 164, 201, 19, 56, 93, 130],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_set_epi64x() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i64 = sample(&mut rng, 0, 0);
            let p1: i64 = sample(&mut rng, 0, 1);
            let got = hr::_mm_set_epi64x(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: i64 = sample(&mut rng, 1, 0);
            let p1: i64 = sample(&mut rng, 1, 1);
            let got = hr::_mm_set_epi64x(p0, p1);
            assert_eq!(
                bytes(&got),
                &[128, 255, 127, 0, 1, 128, 0, 255, 255, 255, 0, 128, 255, 127, 0, 1],
                "Intel fixture 1"
            );
        }
        {
            let p0: i64 = sample(&mut rng, 2, 0);
            let p1: i64 = sample(&mut rng, 2, 1);
            let got = hr::_mm_set_epi64x(p0, p1);
            assert_eq!(
                bytes(&got),
                &[90, 127, 164, 201, 238, 19, 56, 93, 19, 56, 93, 130, 167, 204, 241, 22],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_set_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: i8 = sample(&mut rng, 0, 0);
            let p1: i8 = sample(&mut rng, 0, 1);
            let p2: i8 = sample(&mut rng, 0, 2);
            let p3: i8 = sample(&mut rng, 0, 3);
            let p4: i8 = sample(&mut rng, 0, 4);
            let p5: i8 = sample(&mut rng, 0, 5);
            let p6: i8 = sample(&mut rng, 0, 6);
            let p7: i8 = sample(&mut rng, 0, 7);
            let p8: i8 = sample(&mut rng, 0, 8);
            let p9: i8 = sample(&mut rng, 0, 9);
            let p10: i8 = sample(&mut rng, 0, 10);
            let p11: i8 = sample(&mut rng, 0, 11);
            let p12: i8 = sample(&mut rng, 0, 12);
            let p13: i8 = sample(&mut rng, 0, 13);
            let p14: i8 = sample(&mut rng, 0, 14);
            let p15: i8 = sample(&mut rng, 0, 15);
            let got = hr::_mm_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 1, 0);
            let p1: i8 = sample(&mut rng, 1, 1);
            let p2: i8 = sample(&mut rng, 1, 2);
            let p3: i8 = sample(&mut rng, 1, 3);
            let p4: i8 = sample(&mut rng, 1, 4);
            let p5: i8 = sample(&mut rng, 1, 5);
            let p6: i8 = sample(&mut rng, 1, 6);
            let p7: i8 = sample(&mut rng, 1, 7);
            let p8: i8 = sample(&mut rng, 1, 8);
            let p9: i8 = sample(&mut rng, 1, 9);
            let p10: i8 = sample(&mut rng, 1, 10);
            let p11: i8 = sample(&mut rng, 1, 11);
            let p12: i8 = sample(&mut rng, 1, 12);
            let p13: i8 = sample(&mut rng, 1, 13);
            let p14: i8 = sample(&mut rng, 1, 14);
            let p15: i8 = sample(&mut rng, 1, 15);
            let got = hr::_mm_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            assert_eq!(
                bytes(&got),
                &[128, 255, 1, 255, 255, 254, 0, 128, 127, 0, 127, 1, 0, 0, 128, 255],
                "Intel fixture 1"
            );
        }
        {
            let p0: i8 = sample(&mut rng, 2, 0);
            let p1: i8 = sample(&mut rng, 2, 1);
            let p2: i8 = sample(&mut rng, 2, 2);
            let p3: i8 = sample(&mut rng, 2, 3);
            let p4: i8 = sample(&mut rng, 2, 4);
            let p5: i8 = sample(&mut rng, 2, 5);
            let p6: i8 = sample(&mut rng, 2, 6);
            let p7: i8 = sample(&mut rng, 2, 7);
            let p8: i8 = sample(&mut rng, 2, 8);
            let p9: i8 = sample(&mut rng, 2, 9);
            let p10: i8 = sample(&mut rng, 2, 10);
            let p11: i8 = sample(&mut rng, 2, 11);
            let p12: i8 = sample(&mut rng, 2, 12);
            let p13: i8 = sample(&mut rng, 2, 13);
            let p14: i8 = sample(&mut rng, 2, 14);
            let p15: i8 = sample(&mut rng, 2, 15);
            let got = hr::_mm_set_epi8(
                p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11, p12, p13, p14, p15,
            );
            assert_eq!(
                bytes(&got),
                &[60, 245, 174, 103, 32, 217, 146, 75, 4, 189, 118, 47, 232, 161, 90, 19],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_setzero_si128() {
    unsafe {
        {
            let got = hr::_mm_setzero_si128();
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let got = hr::_mm_setzero_si128();
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 1"
            );
        }
        {
            let got = hr::_mm_setzero_si128();
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_sha1msg1_epu32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_sha1msg1_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_sha1msg1_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[255, 254, 128, 126, 128, 128, 255, 1, 127, 255, 255, 128, 254, 255, 254, 126],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_sha1msg1_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[145, 159, 145, 115, 177, 247, 145, 147, 40, 88, 216, 40, 104, 56, 232, 40],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_sha1msg2_epu32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_sha1msg2_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_sha1msg2_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[249, 251, 251, 5, 254, 0, 255, 2, 3, 1, 255, 255, 3, 2, 253, 2],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_sha1msg2_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[16, 61, 239, 59, 251, 103, 171, 190, 171, 231, 122, 239, 155, 166, 170, 159],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_sha1nexte_epu32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_sha1nexte_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_sha1nexte_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 159, 255, 96],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_sha1nexte_epu32(p0, p1);
            assert_eq!(
                bytes(&got),
                &[90, 127, 164, 201, 238, 19, 56, 93, 130, 167, 204, 241, 73, 184, 230, 84],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_sha1rnds4_epu32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_sha1rnds4_epu32::<0>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[102, 158, 160, 86, 49, 107, 180, 42, 46, 18, 184, 180, 179, 75, 5, 244],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_sha1rnds4_epu32::<2>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[34, 15, 251, 99, 203, 195, 24, 67, 125, 66, 126, 43, 65, 219, 158, 79],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_sha1rnds4_epu32::<3>(p0, p1);
            assert_eq!(
                bytes(&got),
                &[216, 202, 74, 148, 134, 219, 142, 201, 46, 116, 6, 122, 253, 248, 110, 61],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_shuffle_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let got = hr::_mm_shuffle_epi32::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let got = hr::_mm_shuffle_epi32::<128>(p0);
            assert_eq!(
                bytes(&got),
                &[255, 255, 0, 128, 255, 255, 0, 128, 255, 255, 0, 128, 128, 0, 255, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let got = hr::_mm_shuffle_epi32::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[207, 244, 25, 62, 207, 244, 25, 62, 207, 244, 25, 62, 207, 244, 25, 62],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_shuffle_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_shuffle_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_shuffle_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 127, 255, 255, 0, 255, 0, 255, 255, 0, 0, 127, 0, 0, 255],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_shuffle_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[133, 62, 0, 0, 0, 130, 59, 244, 0, 0, 0, 0, 241, 170, 19, 0],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_slli_epi32() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let got = hr::_mm_slli_epi32::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let got = hr::_mm_slli_epi32::<31>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 128, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 128],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let got = hr::_mm_slli_epi32::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_srli_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let got = hr::_mm_srli_si128::<0>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let got = hr::_mm_srli_si128::<31>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let got = hr::_mm_srli_si128::<255>(p0);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_storeu_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let mut p0: Buffer = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            hr::_mm_storeu_si128(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..16],
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
            assert_eq!(
                &p0.0[16..],
                &(0..192).map(|i| fixture_byte(0, 0, i)).collect::<Vec<_>>()[16..]
            );
        }
        {
            let mut p0: Buffer = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            hr::_mm_storeu_si128(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..16],
                &[128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255, 0],
                "Intel fixture 1"
            );
            assert_eq!(
                &p0.0[16..],
                &(0..192).map(|i| fixture_byte(1, 0, i)).collect::<Vec<_>>()[16..]
            );
        }
        {
            let mut p0: Buffer = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            hr::_mm_storeu_si128(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..16],
                &[90, 127, 164, 201, 238, 19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133],
                "Intel fixture 2"
            );
            assert_eq!(
                &p0.0[16..],
                &(0..192).map(|i| fixture_byte(2, 0, i)).collect::<Vec<_>>()[16..]
            );
        }
    }
}

pub fn _mm_sub_epi16() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_sub_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_sub_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[127, 0, 129, 127, 254, 255, 0, 2, 128, 255, 127, 2, 130, 128, 255, 126],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_sub_epi16(p0, p1);
            assert_eq!(
                bytes(&got),
                &[185, 184, 185, 184, 185, 184, 185, 185, 185, 184, 185, 184, 185, 185, 185, 184],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_sub_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_sub_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_sub_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[127, 0, 129, 128, 254, 255, 0, 2, 128, 255, 127, 2, 130, 129, 255, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_sub_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[185, 185, 185, 185, 185, 185, 185, 185, 185, 185, 185, 185, 185, 185, 185, 185],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_unpackhi_epi64() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_unpackhi_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_unpackhi_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[128, 0, 255, 0, 1, 128, 254, 127, 0, 1, 128, 254, 127, 255, 255, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_unpackhi_epi64(p0, p1);
            assert_eq!(
                bytes(&got),
                &[59, 96, 133, 170, 207, 244, 25, 62, 130, 167, 204, 241, 22, 59, 96, 133],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_unpackhi_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_unpackhi_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_unpackhi_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[128, 0, 0, 1, 255, 128, 0, 254, 1, 127, 128, 255, 254, 255, 127, 0],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_unpackhi_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[59, 130, 96, 167, 133, 204, 170, 241, 207, 22, 244, 59, 25, 96, 62, 133],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_unpacklo_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_unpacklo_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_unpacklo_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[255, 128, 255, 255, 0, 127, 128, 0, 255, 1, 127, 128, 0, 0, 1, 255],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_unpacklo_epi8(p0, p1);
            assert_eq!(
                bytes(&got),
                &[19, 90, 56, 127, 93, 164, 130, 201, 167, 238, 204, 19, 241, 56, 22, 93],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_xor_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_xor_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_xor_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[127, 0, 127, 128, 254, 255, 0, 254, 128, 1, 127, 254, 126, 127, 1, 127],
                "Intel fixture 1"
            );
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_xor_si128(p0, p1);
            assert_eq!(
                bytes(&got),
                &[73, 71, 249, 75, 73, 223, 201, 75, 185, 199, 73, 91, 217, 207, 121, 187],
                "Intel fixture 2"
            );
        }
    }
}

pub fn _mm_cmpeq_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_cmpeq_epi8(p0, p1);
            assert_eq!(bytes(&got), &[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255], "Intel fixture 0");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_cmpeq_epi8(p0, p1);
            assert_eq!(bytes(&got), &[0, 255, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0], "Intel fixture 1");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_cmpeq_epi8(p0, p1);
            assert_eq!(bytes(&got), &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], "Intel fixture 2");
        }
    }
}

pub fn _mm_cmpgt_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            let got = hr::_mm_cmpgt_epi8(p0, p1);
            assert_eq!(bytes(&got), &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], "Intel fixture 0");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            let got = hr::_mm_cmpgt_epi8(p0, p1);
            assert_eq!(bytes(&got), &[255, 0, 0, 0, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 255], "Intel fixture 1");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            let got = hr::_mm_cmpgt_epi8(p0, p1);
            assert_eq!(bytes(&got), &[0, 0, 255, 0, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 255], "Intel fixture 2");
        }
    }
}

pub fn _mm_movemask_epi8() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let p0: hr::__m128i = sample(&mut rng, 0, 0);
            let got = hr::_mm_movemask_epi8(p0);
            assert_eq!(bytes(&got), &[0, 0, 0, 0], "Intel fixture 0");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 1, 0);
            let got = hr::_mm_movemask_epi8(p0);
            assert_eq!(bytes(&got), &[27, 101, 0, 0], "Intel fixture 1");
        }
        {
            let p0: hr::__m128i = sample(&mut rng, 2, 0);
            let got = hr::_mm_movemask_epi8(p0);
            assert_eq!(bytes(&got), &[120, 60, 0, 0], "Intel fixture 2");
        }
    }
}

pub fn _mm_store_si128() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    unsafe {
        {
            let mut p0: Buffer = sample(&mut rng, 0, 0);
            let p1: hr::__m128i = sample(&mut rng, 0, 1);
            hr::_mm_store_si128(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..16],
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "Intel fixture 0"
            );
            assert_eq!(
                &p0.0[16..],
                &(0..192).map(|i| fixture_byte(0, 0, i)).collect::<Vec<_>>()[16..]
            );
        }
        {
            let mut p0: Buffer = sample(&mut rng, 1, 0);
            let p1: hr::__m128i = sample(&mut rng, 1, 1);
            hr::_mm_store_si128(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..16],
                &[128, 255, 127, 0, 1, 128, 0, 255, 0, 1, 128, 254, 127, 255, 255, 0],
                "Intel fixture 1"
            );
            assert_eq!(
                &p0.0[16..],
                &(0..192).map(|i| fixture_byte(1, 0, i)).collect::<Vec<_>>()[16..]
            );
        }
        {
            let mut p0: Buffer = sample(&mut rng, 2, 0);
            let p1: hr::__m128i = sample(&mut rng, 2, 1);
            hr::_mm_store_si128(p0.0.as_mut_ptr().cast(), p1);
            assert_eq!(
                &p0.0[..16],
                &[90, 127, 164, 201, 238, 19, 56, 93, 130, 167, 204, 241, 22, 59, 96, 133],
                "Intel fixture 2"
            );
            assert_eq!(
                &p0.0[16..],
                &(0..192).map(|i| fixture_byte(2, 0, i)).collect::<Vec<_>>()[16..]
            );
        }
    }
}
