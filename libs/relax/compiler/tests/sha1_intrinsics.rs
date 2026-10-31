// SHA-1 end to end, using the SHA-NI compression sequence from makepad_git
// (RustCrypto/hashes, MIT/Apache-2.0), and native Arm SHA1 rounds/schedule.
// `makepad test libs/relax/compiler/tests/sha1_intrinsics.rs`
// Also runnable with rustc -O --test. No portable fallback: absent hardware fails coverage.

#[cfg(any(target_arch = "x86_64", relax))]
// the Intel sequence also runs on Arm under Relax (intel_sha1_on_arm): the x86 feature names only on x86_64
#[cfg_attr(target_arch = "x86_64", target_feature(enable = "sha,sse2,ssse3,sse4.1"))]
unsafe fn compress_x86(state: &mut [u32; 5], blocks: &[[u8; 64]]) {
    use std::arch::x86_64::*;

    #[allow(non_snake_case)]
    let MASK: __m128i = _mm_set_epi64x(0x0001_0203_0405_0607, 0x0809_0A0B_0C0D_0E0F);

    let mut state_abcd = _mm_set_epi32(
        state[0] as i32,
        state[1] as i32,
        state[2] as i32,
        state[3] as i32,
    );
    let mut state_e = _mm_set_epi32(state[4] as i32, 0, 0, 0);

    for block in blocks {
        #[allow(clippy::cast_ptr_alignment)]
        let block_ptr = block.as_ptr() as *const __m128i;

        let mut w0 = _mm_shuffle_epi8(_mm_loadu_si128(block_ptr.offset(0)), MASK);
        let mut w1 = _mm_shuffle_epi8(_mm_loadu_si128(block_ptr.offset(1)), MASK);
        let mut w2 = _mm_shuffle_epi8(_mm_loadu_si128(block_ptr.offset(2)), MASK);
        let mut w3 = _mm_shuffle_epi8(_mm_loadu_si128(block_ptr.offset(3)), MASK);
        #[allow(clippy::needless_late_init)]
        let mut w4;

        let mut h0 = state_abcd;
        let mut h1 = _mm_add_epi32(state_e, w0);

        // Rounds 0..20
        h1 = _mm_sha1rnds4_epu32::<0>(h0, h1);
        h0 = _mm_sha1rnds4_epu32::<0>(h1, _mm_sha1nexte_epu32(h0, w1));
        h1 = _mm_sha1rnds4_epu32::<0>(h0, _mm_sha1nexte_epu32(h1, w2));
        h0 = _mm_sha1rnds4_epu32::<0>(h1, _mm_sha1nexte_epu32(h0, w3));
        w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w0, w1), w2), w3);
        h1 = _mm_sha1rnds4_epu32::<0>(h0, _mm_sha1nexte_epu32(h1, w4));

        // Rounds 20..40
        w0 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w1, w2), w3), w4);
        h0 = _mm_sha1rnds4_epu32::<1>(h1, _mm_sha1nexte_epu32(h0, w0));
        w1 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w2, w3), w4), w0);
        h1 = _mm_sha1rnds4_epu32::<1>(h0, _mm_sha1nexte_epu32(h1, w1));
        w2 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w3, w4), w0), w1);
        h0 = _mm_sha1rnds4_epu32::<1>(h1, _mm_sha1nexte_epu32(h0, w2));
        w3 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w4, w0), w1), w2);
        h1 = _mm_sha1rnds4_epu32::<1>(h0, _mm_sha1nexte_epu32(h1, w3));
        w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w0, w1), w2), w3);
        h0 = _mm_sha1rnds4_epu32::<1>(h1, _mm_sha1nexte_epu32(h0, w4));

        // Rounds 40..60
        w0 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w1, w2), w3), w4);
        h1 = _mm_sha1rnds4_epu32::<2>(h0, _mm_sha1nexte_epu32(h1, w0));
        w1 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w2, w3), w4), w0);
        h0 = _mm_sha1rnds4_epu32::<2>(h1, _mm_sha1nexte_epu32(h0, w1));
        w2 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w3, w4), w0), w1);
        h1 = _mm_sha1rnds4_epu32::<2>(h0, _mm_sha1nexte_epu32(h1, w2));
        w3 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w4, w0), w1), w2);
        h0 = _mm_sha1rnds4_epu32::<2>(h1, _mm_sha1nexte_epu32(h0, w3));
        w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w0, w1), w2), w3);
        h1 = _mm_sha1rnds4_epu32::<2>(h0, _mm_sha1nexte_epu32(h1, w4));

        // Rounds 60..80
        w0 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w1, w2), w3), w4);
        h0 = _mm_sha1rnds4_epu32::<3>(h1, _mm_sha1nexte_epu32(h0, w0));
        w1 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w2, w3), w4), w0);
        h1 = _mm_sha1rnds4_epu32::<3>(h0, _mm_sha1nexte_epu32(h1, w1));
        w2 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w3, w4), w0), w1);
        h0 = _mm_sha1rnds4_epu32::<3>(h1, _mm_sha1nexte_epu32(h0, w2));
        w3 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w4, w0), w1), w2);
        h1 = _mm_sha1rnds4_epu32::<3>(h0, _mm_sha1nexte_epu32(h1, w3));
        w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w0, w1), w2), w3);
        h0 = _mm_sha1rnds4_epu32::<3>(h1, _mm_sha1nexte_epu32(h0, w4));

        state_abcd = _mm_add_epi32(state_abcd, h0);
        state_e = _mm_sha1nexte_epu32(h1, state_e);
    }

    state[0] = _mm_extract_epi32::<3>(state_abcd) as u32;
    state[1] = _mm_extract_epi32::<2>(state_abcd) as u32;
    state[2] = _mm_extract_epi32::<1>(state_abcd) as u32;
    state[3] = _mm_extract_epi32::<0>(state_abcd) as u32;
    state[4] = _mm_extract_epi32::<3>(state_e) as u32;
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "sha2")]
unsafe fn compress_arm(state: &mut [u32; 5], blocks: &[[u8; 64]]) {
    use core::arch::aarch64::*;
    let mut abcd = vld1q_u32(state.as_ptr());
    let mut e = state[4];
    for block in blocks {
        let saved_abcd = abcd;
        let saved_e = e;
        let mut words = [0u32; 16];
        for i in 0..16 {
            let j = i * 4;
            words[i] = u32::from_be_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
        }
        let mut m0 = vld1q_u32(words.as_ptr());
        let mut m1 = vld1q_u32(words.as_ptr().add(4));
        let mut m2 = vld1q_u32(words.as_ptr().add(8));
        let mut m3 = vld1q_u32(words.as_ptr().add(12));
        for group in 0..20 {
            let k = [0x5a827999, 0x6ed9eba1, 0x8f1bbcdc, 0xca62c1d6][group / 5];
            let wk = vaddq_u32(m0, vdupq_n_u32(k));
            let next_e = vsha1h_u32(vgetq_lane_u32::<0>(abcd));
            abcd = match group / 5 {
                0 => vsha1cq_u32(abcd, e, wk),
                2 => vsha1mq_u32(abcd, e, wk),
                _ => vsha1pq_u32(abcd, e, wk),
            };
            e = next_e;
            let next_m = vsha1su1q_u32(vsha1su0q_u32(m0, m1, m2), m3);
            m0 = m1;
            m1 = m2;
            m2 = m3;
            m3 = next_m;
        }
        abcd = vaddq_u32(abcd, saved_abcd);
        e = e.wrapping_add(saved_e);
    }
    vst1q_u32(state.as_mut_ptr(), abcd);
    state[4] = e;
}

fn digest(data: &[u8]) -> [u32; 5] {
    #[cfg(target_arch = "x86_64")]
    let compress = compress_x86;
    #[cfg(target_arch = "aarch64")]
    let compress = compress_arm;
    digest_with(data, compress)
}

fn digest_with(data: &[u8], compress: unsafe fn(&mut [u32; 5], &[[u8; 64]])) -> [u32; 5] {
    let block_count = (data.len() + 9 + 63) / 64;
    let mut blocks = vec![[0u8; 64]; block_count];
    for (i, &byte) in data.iter().enumerate() {
        blocks[i / 64][i % 64] = byte;
    }
    blocks[data.len() / 64][data.len() % 64] = 0x80;
    blocks[block_count - 1][56..].copy_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    let mut state = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    unsafe { compress(&mut state, &blocks) };
    state
}

#[test]
fn sha1_known_digests() {
    #[cfg(target_arch = "x86_64")]
    {
        let detected = std::is_x86_feature_detected!("sha");
        let cpuid = unsafe { core::arch::x86_64::__cpuid_count(7, 0) };
        assert_eq!(detected, cpuid.ebx & (1 << 29) != 0);
        assert!(detected, "SHA-NI coverage requires a SHA-capable host");
    }
    #[cfg(target_arch = "aarch64")]
    assert!(std::arch::is_aarch64_feature_detected!("sha2"), "SHA1 coverage requires sha2");
    let cases: &[(&[u8], [u32; 5])] = &[
        (b"", [0xda39a3ee, 0x5e6b4b0d, 0x3255bfef, 0x95601890, 0xafd80709]),
        (b"abc", [0xa9993e36, 0x4706816a, 0xba3e2571, 0x7850c26c, 0x9cd0d89d]),
        (b"The quick brown fox jumps over the lazy dog", [0x2fd4e1c6, 0x7a2d28fc, 0xed849ee1, 0xbb76e739, 0x1b93eb12]),
        (b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq", [0x84983e44, 0x1c3bd26e, 0xbaae4aa1, 0xf95129e5, 0xe54670f1]),
    ];
    // Repeat in one process: the original state must never be mutated as a constant.
    for _ in 0..3 {
        for &(input, expected) in cases {
            assert_eq!(digest(input), expected);
        }
    }
    assert_eq!(digest(&vec![b'a'; 1000]), [0x291e9a6c, 0x66994949, 0xb57ba5e6, 0x50361e98, 0xfc36b1ba]);
}

// The existing check_arch Arm suite also checks Intel's word ordering on Arm.
#[cfg(all(target_arch = "aarch64", relax))]
#[test]
fn intel_sha1_on_arm() {
    for input in [b"".as_slice(), b"abc".as_slice(), &[b'a'; 1000]] {
        assert_eq!(digest_with(input, compress_x86), digest(input));
    }
}
