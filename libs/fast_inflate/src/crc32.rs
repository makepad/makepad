// Port of libdeflate's crc32.c with platform-specific intrinsics
// Original: Copyright 2016 Eric Biggers, MIT license
//
// Implementations:
// - aarch64: hardware CRC32 instructions (__crc32b, __crc32d)
// - x86/x86_64: PCLMULQDQ carryless multiplication
// - fallback: slice-by-8 table-based

/// One-shot CRC32 (gzip polynomial).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.sum()
}

/// Rolling CRC32 state.
pub struct Crc32 {
    val: u32,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    pub const fn new() -> Self {
        Crc32 { val: 0 }
    }
    pub const fn with_initial(val: u32) -> Self {
        Crc32 { val }
    }
    pub fn update(&mut self, data: &[u8]) {
        self.val = crc32_update(self.val, data);
    }
    pub const fn sum(&self) -> u32 {
        self.val
    }
}

fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    #[cfg(target_arch = "aarch64")]
    {
        if is_aarch64_crc_available() {
            return unsafe { crc32_aarch64(!crc, data) };
        }
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_pclmulqdq_available() {
            return unsafe { crc32_pclmulqdq(!crc, data) };
        }
    }
    !crc32_slice8(!crc, data)
}

// ============================================================================
// aarch64 hardware CRC32 implementation
// ============================================================================

#[cfg(target_arch = "aarch64")]
fn is_aarch64_crc_available() -> bool {
    std::arch::is_aarch64_feature_detected!("crc")
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "crc")]
unsafe fn crc32_aarch64(crc: u32, data: &[u8]) -> u32 {
    use std::arch::aarch64::*;

    let mut c = crc;
    let (pre, quads, post) = data.align_to::<u64>();

    // Handle pre-alignment bytes one at a time
    for &b in pre {
        c = __crc32b(c, b);
    }

    // Process 8 bytes at a time, unrolled 8x for ILP
    let mut quad_iter = quads.chunks_exact(8);
    for chunk in &mut quad_iter {
        c = __crc32d(c, chunk[0]);
        c = __crc32d(c, chunk[1]);
        c = __crc32d(c, chunk[2]);
        c = __crc32d(c, chunk[3]);
        c = __crc32d(c, chunk[4]);
        c = __crc32d(c, chunk[5]);
        c = __crc32d(c, chunk[6]);
        c = __crc32d(c, chunk[7]);
    }
    for &q in quad_iter.remainder() {
        c = __crc32d(c, q);
    }

    // Handle post-alignment bytes
    for &b in post {
        c = __crc32b(c, b);
    }

    !c
}

// ============================================================================
// x86/x86_64 PCLMULQDQ implementation
// Based on Intel whitepaper: "Fast CRC computation for generic polynomials
// using PCLMULQDQ instruction" and the crc32fast crate implementation.
// ============================================================================

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn is_x86_pclmulqdq_available() -> bool {
    is_x86_feature_detected!("pclmulqdq")
        && is_x86_feature_detected!("sse2")
        && is_x86_feature_detected!("sse4.1")
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "pclmulqdq", enable = "sse2", enable = "sse4.1")]
unsafe fn crc32_pclmulqdq(crc: u32, data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86")]
    use std::arch::x86 as arch;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64 as arch;

    // For small data, fall back to slice-by-8
    if data.len() < 128 {
        return !crc32_slice8(crc, data);
    }

    let mut p = data;

    // Fold constants for CRC-32 (gzip polynomial 0xEDB88320, bit-reflected)
    const K1: i64 = 0x154442bd4;
    const K2: i64 = 0x1c6e41596;
    const K3: i64 = 0x1751997d0;
    const K4: i64 = 0x0ccaa009e;
    const K5: i64 = 0x163cd6124;
    const P_X: i64 = 0x1DB710641;
    const U_PRIME: i64 = 0x1F7011641;

    #[inline(always)]
    unsafe fn get(p: &mut &[u8]) -> arch::__m128i {
        debug_assert!(p.len() >= 16);
        let r = arch::_mm_loadu_si128(p.as_ptr() as *const arch::__m128i);
        *p = &p[16..];
        r
    }

    #[inline(always)]
    unsafe fn reduce128(a: arch::__m128i, b: arch::__m128i, keys: arch::__m128i) -> arch::__m128i {
        let t1 = arch::_mm_clmulepi64_si128(a, keys, 0x00);
        let t2 = arch::_mm_clmulepi64_si128(a, keys, 0x11);
        arch::_mm_xor_si128(arch::_mm_xor_si128(b, t1), t2)
    }

    // Step 1: fold by 4 loop
    let mut x3 = get(&mut p);
    let mut x2 = get(&mut p);
    let mut x1 = get(&mut p);
    let mut x0 = get(&mut p);

    // Fold in initial CRC value
    x3 = arch::_mm_xor_si128(x3, arch::_mm_cvtsi32_si128(!crc as i32));

    let k1k2 = arch::_mm_set_epi64x(K2, K1);
    while p.len() >= 64 {
        x3 = reduce128(x3, get(&mut p), k1k2);
        x2 = reduce128(x2, get(&mut p), k1k2);
        x1 = reduce128(x1, get(&mut p), k1k2);
        x0 = reduce128(x0, get(&mut p), k1k2);
    }

    let k3k4 = arch::_mm_set_epi64x(K4, K3);
    let mut x = reduce128(x3, x2, k3k4);
    x = reduce128(x, x1, k3k4);
    x = reduce128(x, x0, k3k4);

    // Step 2: fold by 1 loop
    while p.len() >= 16 {
        x = reduce128(x, get(&mut p), k3k4);
    }

    // Step 3: reduction from 128 bits to 64 bits
    x = arch::_mm_xor_si128(
        arch::_mm_clmulepi64_si128(x, k3k4, 0x10),
        arch::_mm_srli_si128(x, 8),
    );
    x = arch::_mm_xor_si128(
        arch::_mm_clmulepi64_si128(
            arch::_mm_and_si128(x, arch::_mm_set_epi32(0, 0, 0, !0)),
            arch::_mm_set_epi64x(0, K5),
            0x00,
        ),
        arch::_mm_srli_si128(x, 4),
    );

    // Step 4: Barrett reduction from 64 bits to 32 bits
    let pu = arch::_mm_set_epi64x(U_PRIME, P_X);
    let t1 = arch::_mm_clmulepi64_si128(
        arch::_mm_and_si128(x, arch::_mm_set_epi32(0, 0, 0, !0)),
        pu,
        0x10,
    );
    let t2 = arch::_mm_clmulepi64_si128(
        arch::_mm_and_si128(t1, arch::_mm_set_epi32(0, 0, 0, !0)),
        pu,
        0x00,
    );
    let c = arch::_mm_extract_epi32(arch::_mm_xor_si128(x, t2), 1) as u32;

    // Handle remaining bytes
    if !p.is_empty() {
        !crc32_slice8(!c, p)
    } else {
        !c
    }
}

// ============================================================================
// Portable slice-by-8 fallback
// ============================================================================

fn crc32_slice8(mut crc: u32, data: &[u8]) -> u32 {
    let mut i = 0;
    let len = data.len();

    // Align to 8 bytes
    while i < len && (data.as_ptr() as usize + i) & 7 != 0 {
        crc = (crc >> 8) ^ CRC32_TABLE[((crc as u8) ^ data[i]) as usize];
        i += 1;
    }

    // Process 8 bytes at a time
    while i + 8 <= len {
        let v1 = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let v2 = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
        let xv1 = crc ^ v1;

        crc = CRC32_SLICE8[0x700 + ((xv1 >> 0) & 0xFF) as usize]
            ^ CRC32_SLICE8[0x600 + ((xv1 >> 8) & 0xFF) as usize]
            ^ CRC32_SLICE8[0x500 + ((xv1 >> 16) & 0xFF) as usize]
            ^ CRC32_SLICE8[0x400 + ((xv1 >> 24) & 0xFF) as usize]
            ^ CRC32_SLICE8[0x300 + ((v2 >> 0) & 0xFF) as usize]
            ^ CRC32_SLICE8[0x200 + ((v2 >> 8) & 0xFF) as usize]
            ^ CRC32_SLICE8[0x100 + ((v2 >> 16) & 0xFF) as usize]
            ^ CRC32_SLICE8[0x000 + ((v2 >> 24) & 0xFF) as usize];

        i += 8;
    }

    // Remaining bytes
    while i < len {
        crc = (crc >> 8) ^ CRC32_TABLE[((crc as u8) ^ data[i]) as usize];
        i += 1;
    }

    crc
}

// Slice-1 table (first 256 entries of slice-8 table)
static CRC32_TABLE: [u32; 256] = [
    0x00000000, 0x77073096, 0xee0e612c, 0x990951ba, 0x076dc419, 0x706af48f, 0xe963a535, 0x9e6495a3,
    0x0edb8832, 0x79dcb8a4, 0xe0d5e91e, 0x97d2d988, 0x09b64c2b, 0x7eb17cbd, 0xe7b82d07, 0x90bf1d91,
    0x1db71064, 0x6ab020f2, 0xf3b97148, 0x84be41de, 0x1adad47d, 0x6ddde4eb, 0xf4d4b551, 0x83d385c7,
    0x136c9856, 0x646ba8c0, 0xfd62f97a, 0x8a65c9ec, 0x14015c4f, 0x63066cd9, 0xfa0f3d63, 0x8d080df5,
    0x3b6e20c8, 0x4c69105e, 0xd56041e4, 0xa2677172, 0x3c03e4d1, 0x4b04d447, 0xd20d85fd, 0xa50ab56b,
    0x35b5a8fa, 0x42b2986c, 0xdbbbc9d6, 0xacbcf940, 0x32d86ce3, 0x45df5c75, 0xdcd60dcf, 0xabd13d59,
    0x26d930ac, 0x51de003a, 0xc8d75180, 0xbfd06116, 0x21b4f4b5, 0x56b3c423, 0xcfba9599, 0xb8bda50f,
    0x2802b89e, 0x5f058808, 0xc60cd9b2, 0xb10be924, 0x2f6f7c87, 0x58684c11, 0xc1611dab, 0xb6662d3d,
    0x76dc4190, 0x01db7106, 0x98d220bc, 0xefd5102a, 0x71b18589, 0x06b6b51f, 0x9fbfe4a5, 0xe8b8d433,
    0x7807c9a2, 0x0f00f934, 0x9609a88e, 0xe10e9818, 0x7f6a0dbb, 0x086d3d2d, 0x91646c97, 0xe6635c01,
    0x6b6b51f4, 0x1c6c6162, 0x856530d8, 0xf262004e, 0x6c0695ed, 0x1b01a57b, 0x8208f4c1, 0xf50fc457,
    0x65b0d9c6, 0x12b7e950, 0x8bbeb8ea, 0xfcb9887c, 0x62dd1ddf, 0x15da2d49, 0x8cd37cf3, 0xfbd44c65,
    0x4db26158, 0x3ab551ce, 0xa3bc0074, 0xd4bb30e2, 0x4adfa541, 0x3dd895d7, 0xa4d1c46d, 0xd3d6f4fb,
    0x4369e96a, 0x346ed9fc, 0xad678846, 0xda60b8d0, 0x44042d73, 0x33031de5, 0xaa0a4c5f, 0xdd0d7cc9,
    0x5005713c, 0x270241aa, 0xbe0b1010, 0xc90c2086, 0x5768b525, 0x206f85b3, 0xb966d409, 0xce61e49f,
    0x5edef90e, 0x29d9c998, 0xb0d09822, 0xc7d7a8b4, 0x59b33d17, 0x2eb40d81, 0xb7bd5c3b, 0xc0ba6cad,
    0xedb88320, 0x9abfb3b6, 0x03b6e20c, 0x74b1d29a, 0xead54739, 0x9dd277af, 0x04db2615, 0x73dc1683,
    0xe3630b12, 0x94643b84, 0x0d6d6a3e, 0x7a6a5aa8, 0xe40ecf0b, 0x9309ff9d, 0x0a00ae27, 0x7d079eb1,
    0xf00f9344, 0x8708a3d2, 0x1e01f268, 0x6906c2fe, 0xf762575d, 0x806567cb, 0x196c3671, 0x6e6b06e7,
    0xfed41b76, 0x89d32be0, 0x10da7a5a, 0x67dd4acc, 0xf9b9df6f, 0x8ebeeff9, 0x17b7be43, 0x60b08ed5,
    0xd6d6a3e8, 0xa1d1937e, 0x38d8c2c4, 0x4fdff252, 0xd1bb67f1, 0xa6bc5767, 0x3fb506dd, 0x48b2364b,
    0xd80d2bda, 0xaf0a1b4c, 0x36034af6, 0x41047a60, 0xdf60efc3, 0xa867df55, 0x316e8eef, 0x4669be79,
    0xcb61b38c, 0xbc66831a, 0x256fd2a0, 0x5268e236, 0xcc0c7795, 0xbb0b4703, 0x220216b9, 0x5505262f,
    0xc5ba3bbe, 0xb2bd0b28, 0x2bb45a92, 0x5cb36a04, 0xc2d7ffa7, 0xb5d0cf31, 0x2cd99e8b, 0x5bdeae1d,
    0x9b64c2b0, 0xec63f226, 0x756aa39c, 0x026d930a, 0x9c0906a9, 0xeb0e363f, 0x72076785, 0x05005713,
    0x95bf4a82, 0xe2b87a14, 0x7bb12bae, 0x0cb61b38, 0x92d28e9b, 0xe5d5be0d, 0x7cdcefb7, 0x0bdbdf21,
    0x86d3d2d4, 0xf1d4e242, 0x68ddb3f8, 0x1fda836e, 0x81be16cd, 0xf6b9265b, 0x6fb077e1, 0x18b74777,
    0x88085ae6, 0xff0f6a70, 0x66063bca, 0x11010b5c, 0x8f659eff, 0xf862ae69, 0x616bffd3, 0x166ccf45,
    0xa00ae278, 0xd70dd2ee, 0x4e048354, 0x3903b3c2, 0xa7672661, 0xd06016f7, 0x4969474d, 0x3e6e77db,
    0xaed16a4a, 0xd9d65adc, 0x40df0b66, 0x37d83bf0, 0xa9bcae53, 0xdebb9ec5, 0x47b2cf7f, 0x30b5ffe9,
    0xbdbdf21c, 0xcabac28a, 0x53b39330, 0x24b4a3a6, 0xbad03605, 0xcdd70693, 0x54de5729, 0x23d967bf,
    0xb3667a2e, 0xc4614ab8, 0x5d681b02, 0x2a6f2b94, 0xb40bbe37, 0xc30c8ea1, 0x5a05df1b, 0x2d02ef8d,
];

// Full slice-by-8 table (2048 entries = 8 * 256)
// Generated from the gzip CRC-32 polynomial 0xEDB88320
static CRC32_SLICE8: [u32; 2048] = include!("crc32_table.inc");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32_empty() {
        assert_eq!(crc32(&[]), 0);
    }

    #[test]
    fn test_crc32_hello() {
        // Known CRC32 of "Hello" = 0xF7D18982
        let val = crc32(b"Hello");
        assert_eq!(val, 0xF7D18982);
    }

    #[test]
    fn test_crc32_rolling() {
        let data = b"Hello, World!";
        let one_shot = crc32(data);

        let mut rolling = Crc32::new();
        rolling.update(&data[..5]);
        rolling.update(&data[5..]);
        assert_eq!(rolling.sum(), one_shot);
    }

    #[test]
    fn test_crc32_various_sizes() {
        // Test various sizes to exercise alignment and tail handling
        let data: Vec<u8> = (0..=255).cycle().take(4096).collect();
        for size in [
            1, 2, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 512, 1024, 4096,
        ] {
            let slice = &data[..size];
            // Compare accelerated path against slice-by-8 fallback
            let expected = !crc32_slice8(!0, slice);
            let got = crc32(slice);
            assert_eq!(got, expected, "mismatch at size {}", size);
        }
    }

    #[test]
    fn test_crc32_unaligned() {
        // Test with data at various alignments
        let data: Vec<u8> = (0..=255).cycle().take(1024).collect();
        for offset in 0..8 {
            let slice = &data[offset..offset + 512];
            let expected = !crc32_slice8(!0, slice);
            let got = crc32(slice);
            assert_eq!(got, expected, "mismatch at offset {}", offset);
        }
    }
}
