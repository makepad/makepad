#[cfg(target_arch = "x86_64")]
mod x86 {
    use core::arch::x86_64::*;
    use core::mem::transmute;

    // FIPS-197 (2001), Appendices C.1 and B. Each tuple is (state, round
    // key, result); hexadecimal strings are in AES column-major byte order.
    const ENC: [(u128, u128, u128); 2] = [
        (
            0x00102030405060708090a0b0c0d0e0f0,
            0xd6aa74fdd2af72fadaa678f1d6ab76fe,
            0x89d810e8855ace682d1843d8cb128fe4,
        ),
        (
            0x193de3bea0f4e22b9ac68d2ae9f84808,
            0xa0fafe1788542cb123a339392a6c7605,
            0xa49c7ff2689f352b6b5bea43026a5049,
        ),
    ];
    const ENCLAST: [(u128, u128, u128); 2] = [
        (
            0xbd6e7c3df2b5779e0b61216e8b10b689,
            0x13111d7fe3944a17f307a78b4d2b30c5,
            0x69c4e0d86a7b0430d8cdb78070b4c55a,
        ),
        (
            0xeb40f21e592e38848ba113e71bc342d2,
            0xd014f9a8c9ee2589e13f0cc8b6630ca6,
            0x3925841d02dc09fbdc118597196a0b32,
        ),
    ];
    // The equivalent inverse cipher uses InvMixColumns(key[9]). Results
    // are C.1's round[2].istart and B's round-9 state after ShiftRows.
    const DEC: [(u128, u128, u128); 2] = [
        (
            0x7ad5fda789ef4e272bca100b3d9ff59f,
            0x13aa29be9c8faff6f770f58000f7bf03,
            0x54d990a16ba09ab596bbf40ea111702f,
        ),
        (
            0xe9317db5cb322c723d2e895faf090794,
            0x0c7b5a631319eafeb0398890664cfbb4,
            0x876e46a6f24ce78c4d904ad897ecc395,
        ),
    ];
    const DECLAST: [(u128, u128, u128); 2] = [
        (
            0x6353e08c0960e104cd70b751bacad0e7,
            0x000102030405060708090a0b0c0d0e0f,
            0x00112233445566778899aabbccddeeff,
        ),
        (
            0xd4bf5d30e0b452aeb84111f11e2798e5,
            0x2b7e151628aed2a6abf7158809cf4f3c,
            0x3243f6a8885a308d313198a2e0370734,
        ),
    ];

    // Polynomial multiplication followed by long division by the AES
    // modulus. This reference does not use an intrinsic or a lookup table.
    fn gf_product(a: u8, b: u8) -> u8 {
        let mut product = 0u16;
        for bit in 0..8 {
            if (b >> bit) & 1 != 0 {
                product ^= (a as u16) << bit;
            }
        }
        for bit in (8..15).rev() {
            if (product >> bit) & 1 != 0 {
                product ^= 0x11b << (bit - 8);
            }
        }
        product as u8
    }

    fn sub_byte(x: u8) -> u8 {
        let mut inverse = 0u8;
        for candidate in 1..256u16 {
            if gf_product(x, candidate as u8) == 1 {
                inverse = candidate as u8;
                break;
            }
        }
        let mut result = 0u8;
        for bit in 0..8 {
            let b = ((inverse >> bit)
                ^ (inverse >> ((bit + 4) % 8))
                ^ (inverse >> ((bit + 5) % 8))
                ^ (inverse >> ((bit + 6) % 8))
                ^ (inverse >> ((bit + 7) % 8))
                ^ (0x63 >> bit)) & 1;
            result |= b << bit;
        }
        result
    }

    #[target_feature(enable = "aes")]
    unsafe fn check_keygen<const IMM8: i32>() {
        for input in [
            0x000102030405060708090a0b0c0d0e0fu128,
            0x2b7e151628aed2a6abf7158809cf4f3c,
            0x0000000000000000ffffffffffffffff,
        ] {
            let bytes = input.to_be_bytes();
            let mut expected = [0u8; 16];
            // Intel AESKEYGENASSIST: SubWord(X1), RotWord(SubWord(X1))
            // XOR ZeroExtend(IMM8), SubWord(X3), RotWord(SubWord(X3)) XOR IMM8.
            for word in 0..2 {
                let mut subword = [0u8; 4];
                for byte in 0..4 {
                    subword[byte] = sub_byte(bytes[8 * word + 4 + byte]);
                }
                for byte in 0..4 {
                    expected[8 * word + byte] = subword[byte];
                    expected[8 * word + 4 + byte] = subword[(byte + 1) % 4];
                }
                expected[8 * word + 4] ^= IMM8 as u8;
            }
            let a = transmute::<[u8; 16], __m128i>(bytes);
            let actual: [u8; 16] = transmute(_mm_aeskeygenassist_si128::<IMM8>(a));
            assert_eq!(actual, expected);
        }
    }

    #[target_feature(enable = "aes")]
    unsafe fn check_aes() {
        for (operation, cases) in [ENC, ENCLAST, DEC, DECLAST].iter().enumerate() {
            for &(state, key, expected) in cases {
                let a = transmute::<[u8; 16], __m128i>(state.to_be_bytes());
                let k = transmute::<[u8; 16], __m128i>(key.to_be_bytes());
                let actual: [u8; 16] = transmute(match operation {
                    0 => _mm_aesenc_si128(a, k),
                    1 => _mm_aesenclast_si128(a, k),
                    2 => _mm_aesdec_si128(a, k),
                    _ => _mm_aesdeclast_si128(a, k),
                });
                assert_eq!(actual, expected.to_be_bytes());
            }
        }
        // Undo round 1's MixColumns (C.1, then B), and check the equivalent
        // inverse round keys used above against their encryption round keys.
        for (input, expected) in [
            (0x5f72641557f5bc92f7be3b291db9f91au128, 0x6353e08c0960e104cd70b751bacad0e7u128),
            (0x046681e5e0cb199a48f8d37a2806264c, 0xd4bf5d30e0b452aeb84111f11e2798e5),
            (0x549932d1f08557681093ed9cbe2c974e, DEC[0].1),
            (0xac7766f319fadc2128d12941575c006e, DEC[1].1),
        ] {
            let a = transmute::<[u8; 16], __m128i>(input.to_be_bytes());
            let actual: [u8; 16] = transmute(_mm_aesimc_si128(a));
            assert_eq!(actual, expected.to_be_bytes());
        }
        check_keygen::<0>();
        check_keygen::<1>();
        check_keygen::<0x1b>();
        check_keygen::<255>();
        let a = transmute::<[u8; 16], __m128i>([0; 16]);
        let legacy: [u8; 16] = transmute(_mm_aeskeygenassist_si128(a, 1));
        let mut expected = [0x63; 16];
        expected[4] ^= 1;
        expected[12] ^= 1;
        assert_eq!(legacy, expected);
    }

    #[test]
    fn aes_fips197_rounds_and_keygen() {
        if is_x86_feature_detected!("aes") {
            unsafe { check_aes() };
        }
    }

    #[target_feature(enable = "vaes")]
    unsafe fn check_vaes256() {
        for (operation, cases) in [ENC, ENCLAST, DEC, DECLAST].iter().enumerate() {
            for offset in 0..2 {
                let mut states = [[0u8; 16]; 2];
                let mut keys = [[0u8; 16]; 2];
                let mut expected = [[0u8; 16]; 2];
                for lane in 0..2 {
                    let case = cases[(lane + offset) % 2];
                    states[lane] = case.0.to_be_bytes();
                    keys[lane] = case.1.to_be_bytes();
                    expected[lane] = case.2.to_be_bytes();
                }
                let a = transmute::<[[u8; 16]; 2], __m256i>(states);
                let k = transmute::<[[u8; 16]; 2], __m256i>(keys);
                let actual: [[u8; 16]; 2] = transmute(match operation {
                    0 => _mm256_aesenc_epi128(a, k),
                    1 => _mm256_aesenclast_epi128(a, k),
                    2 => _mm256_aesdec_epi128(a, k),
                    _ => _mm256_aesdeclast_epi128(a, k),
                });
                assert_eq!(actual, expected);
            }
        }
    }

    #[target_feature(enable = "vaes,avx512f")]
    unsafe fn check_vaes512() {
        for (operation, cases) in [ENC, ENCLAST, DEC, DECLAST].iter().enumerate() {
            let mut states = [[0u8; 16]; 4];
            let mut keys = [[0u8; 16]; 4];
            let mut expected = [[0u8; 16]; 4];
            for lane in 0..4 {
                let case = cases[lane % 2];
                states[lane] = case.0.to_be_bytes();
                keys[lane] = case.1.to_be_bytes();
                expected[lane] = case.2.to_be_bytes();
                // AddRoundKey is the final XOR in all four instructions.
                // Distinguish the upper lanes without using another intrinsic.
                if lane >= 2 {
                    for byte in 0..16 {
                        let change = (lane * 17 + byte) as u8;
                        keys[lane][byte] ^= change;
                        expected[lane][byte] ^= change;
                    }
                }
            }
            let a = transmute::<[[u8; 16]; 4], __m512i>(states);
            let k = transmute::<[[u8; 16]; 4], __m512i>(keys);
            let actual: [[u8; 16]; 4] = transmute(match operation {
                0 => _mm512_aesenc_epi128(a, k),
                1 => _mm512_aesenclast_epi128(a, k),
                2 => _mm512_aesdec_epi128(a, k),
                _ => _mm512_aesdeclast_epi128(a, k),
            });
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn vaes_fips197_lanes() {
        if is_x86_feature_detected!("vaes") {
            unsafe { check_vaes256() };
            if is_x86_feature_detected!("avx512f") {
                unsafe { check_vaes512() };
            }
        }
    }

    // A known base alignment makes offsets 1 and 3 genuinely unaligned.
    #[repr(align(64))]
    struct Memory([u8; 80]);

    #[target_feature(enable = "sse2")]
    unsafe fn check_sse2() {
        let lanes = [i16::MIN, -123, -1, 0, 1, 123, 456, i16::MAX];
        let actual: [i16; 8] = transmute(_mm_set_epi16(
            lanes[7], lanes[6], lanes[5], lanes[4],
            lanes[3], lanes[2], lanes[1], lanes[0],
        ));
        assert_eq!(actual, lanes);

        let a = [0x0123456789abcdefu64, 0xfedcba9876543210];
        let mut memory = Memory([0xa5; 80]);
        _mm_storel_epi64(memory.0.as_mut_ptr().add(1) as *mut __m128i, transmute(a));
        let mut expected = [0xa5; 80];
        for byte in 0..8 {
            expected[1 + byte] = (a[0] >> (byte * 8)) as u8;
        }
        assert_eq!(memory.0, expected);
    }

    #[test]
    fn constant_and_sse2_lanes() {
        assert_eq!(_XCR_XFEATURE_ENABLED_MASK, 0u32);
        if is_x86_feature_detected!("sse2") {
            unsafe { check_sse2() };
        }
    }

    #[target_feature(enable = "avx2")]
    unsafe fn check_avx2() {
        let a = [i32::MIN, -123, -1, 0, 1, 17, 987654, i32::MAX];
        let v = transmute::<[i32; 8], __m256i>(a);
        let extracted = [
            _mm256_extract_epi32::<0>(v), _mm256_extract_epi32::<1>(v),
            _mm256_extract_epi32::<2>(v), _mm256_extract_epi32::<3>(v),
            _mm256_extract_epi32::<4>(v), _mm256_extract_epi32::<5>(v),
            _mm256_extract_epi32::<6>(v), _mm256_extract_epi32::<7>(v),
        ];
        assert_eq!(extracted, a);
        assert_eq!(_mm256_extract_epi32(v, 7), a[7]);

        let q = [i64::MIN, -1, 0x123456789abcdef, i64::MAX];
        let vq = transmute::<[i64; 4], __m256i>(q);
        let extracted = [
            _mm256_extract_epi64::<0>(vq), _mm256_extract_epi64::<1>(vq),
            _mm256_extract_epi64::<2>(vq), _mm256_extract_epi64::<3>(vq),
        ];
        assert_eq!(extracted, q);
        assert_eq!(_mm256_extract_epi64(vq, 2), q[2]);
        let set: [i64; 4] = transmute(_mm256_setr_epi64x(q[0], q[1], q[2], q[3]));
        assert_eq!(set, q);

        let lo = transmute::<[i32; 4], __m128i>([a[0], a[1], a[2], a[3]]);
        let hi = transmute::<[i32; 4], __m128i>([a[4], a[5], a[6], a[7]]);
        let joined: [i32; 8] = transmute(_mm256_set_m128i(hi, lo));
        assert_eq!(joined, a);
        let broadcast: [i32; 8] = transmute(_mm256_broadcastsi128_si256(lo));
        let dwords: [i32; 8] = transmute(_mm256_broadcastd_epi32(lo));
        for lane in 0..8 {
            assert_eq!(broadcast[lane], a[lane % 4]);
            assert_eq!(dwords[lane], a[0]);
        }
        let replacement = [31i32, -32, 33, -34];
        let vr = transmute::<[i32; 4], __m128i>(replacement);
        let inserted = [
            transmute::<__m256i, [i32; 8]>(_mm256_insertf128_si256::<0>(v, vr)),
            transmute::<__m256i, [i32; 8]>(_mm256_insertf128_si256::<1>(v, vr)),
            transmute::<__m256i, [i32; 8]>(_mm256_insertf128_si256(v, vr, 1)),
        ];
        for case in 0..3 {
            let half = if case == 0 { 0 } else { 1 };
            for lane in 0..8 {
                let expected = if lane / 4 == half { replacement[lane % 4] } else { a[lane] };
                assert_eq!(inserted[case][lane], expected);
            }
        }

        let mut x = [0u16; 16];
        let mut y = [0u16; 16];
        for lane in 0..16 {
            x[lane] = (lane * 4371) as u16;
            y[lane] = u16::MAX - (lane * 123) as u16;
        }
        let product: [u16; 16] = transmute(_mm256_mulhi_epu16(transmute(x), transmute(y)));
        for lane in 0..16 {
            assert_eq!(product[lane], ((x[lane] as u32 * y[lane] as u32) / 65536) as u16);
        }
        let counts = [0u32, 1, 31, 32, 33, u32::MAX, 16, 7];
        let left: [u32; 8] = transmute(_mm256_sllv_epi32(v, transmute(counts)));
        for lane in 0..8 {
            let expected = if counts[lane] < 32 {
                ((a[lane] as u32 as u64) * (1u64 << counts[lane])) as u32
            } else {
                0
            };
            assert_eq!(left[lane], expected);
        }
        let x = [u64::MAX, 0x8123456789abcdef, 0x8000000000000000, 0xfedcba9876543210];
        for counts in [[0u64, 1, 63, 64], [65, u64::MAX, 1u64 << 32, 32]] {
            let right: [u64; 4] = transmute(_mm256_srlv_epi64(transmute(x), transmute(counts)));
            for lane in 0..4 {
                let expected = if counts[lane] < 64 { x[lane] / (1u64 << counts[lane]) } else { 0 };
                assert_eq!(right[lane], expected);
            }
        }
    }

    #[test]
    fn avx2_broadcast_extract_insert_arithmetic() {
        if is_x86_feature_detected!("avx2") {
            unsafe { check_avx2() };
        }
    }

    fn rotate_reference(value: u32, count: i32) -> u32 {
        let mut expected = 0u32;
        for bit in 0..32 {
            expected |= ((value >> bit) & 1) << ((bit + count as u32) % 32);
        }
        expected
    }

    const ROTATE_INPUT: [u32; 16] = [
        0, 1, u32::MAX, 0x80000000, 0x01234567, 0x89abcdef, 0xaaaaaaaa, 0x55555555,
        0xdeadbeef, 0xfedcba98, 2, 3, 0x80000001, 0x76543210, 0x00010000, 0x7fffffff,
    ];

    #[target_feature(enable = "avx512f")]
    unsafe fn check_rotate512<const IMM8: i32>() {
        let a = transmute::<[u32; 16], __m512i>(ROTATE_INPUT);
        let actual: [u32; 16] = transmute(_mm512_rol_epi32::<IMM8>(a));
        for lane in 0..16 {
            assert_eq!(actual[lane], rotate_reference(ROTATE_INPUT[lane], IMM8));
        }
    }

    #[target_feature(enable = "avx512f,avx512vl")]
    unsafe fn check_rotate_vl<const IMM8: i32>() {
        let mut a = [0u32; 8];
        let mut b = [0u32; 4];
        for lane in 0..8 {
            a[lane] = ROTATE_INPUT[lane + 4];
        }
        for lane in 0..4 {
            b[lane] = ROTATE_INPUT[lane + 12];
        }
        let wide: [u32; 8] = transmute(_mm256_rol_epi32::<IMM8>(transmute(a)));
        let narrow: [u32; 4] = transmute(_mm_rol_epi32::<IMM8>(transmute(b)));
        for lane in 0..8 {
            assert_eq!(wide[lane], rotate_reference(a[lane], IMM8));
        }
        for lane in 0..4 {
            assert_eq!(narrow[lane], rotate_reference(b[lane], IMM8));
        }
    }

    #[target_feature(enable = "avx512f")]
    unsafe fn check_shuffles<const MASK: i32>() {
        let mut a = [0i32; 16];
        let mut b = [0i32; 16];
        for lane in 0..16 {
            a[lane] = lane as i32 * 31 - 500;
            b[lane] = lane as i32 * 47 + 1000;
        }
        let va = transmute::<[i32; 16], __m512i>(a);
        let vb = transmute::<[i32; 16], __m512i>(b);
        let dwords: [i32; 16] = transmute(_mm512_shuffle_epi32::<MASK>(va));
        let blocks: [i32; 16] = transmute(_mm512_shuffle_i32x4::<MASK>(va, vb));
        for lane in 0..16 {
            let selector = ((MASK >> (2 * (lane % 4))) & 3) as usize;
            assert_eq!(dwords[lane], a[(lane / 4) * 4 + selector]);
            let block = ((MASK >> (2 * (lane / 4))) & 3) as usize;
            let source = if lane < 8 { a } else { b };
            assert_eq!(blocks[lane], source[block * 4 + lane % 4]);
        }
    }

    #[target_feature(enable = "avx512f")]
    unsafe fn check_avx512() {
        let a = [i64::MIN, i64::MAX, -1, 0, 1, 7, -39, 987654321];
        let b = [-1i64, 1, 1, i64::MIN, i64::MAX, -13, 59, -17];
        let va = transmute::<[i64; 8], __m512i>(a);
        let vb = transmute::<[i64; 8], __m512i>(b);
        let sum: [i64; 8] = transmute(_mm512_add_epi64(va, vb));
        for lane in 0..8 {
            assert_eq!(sum[lane], (a[lane] as i128 + b[lane] as i128) as i64);
        }
        let set: [i64; 8] = transmute(_mm512_setr_epi64(
            a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7],
        ));
        assert_eq!(set, a);
        let mut table = [0i64; 16];
        for lane in 0..8 {
            table[lane] = a[lane];
            table[lane + 8] = b[lane];
        }
        for indices in [
            [0i64, 7, 8, 15, -1, i64::MIN, 0x123456789abcde19, -9],
            [1, 2, 3, 4, 5, 6, 10, 11],
            [12, 13, 14, 16, 23, 24, 31, i64::MAX],
        ] {
            let actual: [i64; 8] = transmute(_mm512_permutex2var_epi64(va, transmute(indices), vb));
            for lane in 0..8 {
                assert_eq!(actual[lane], table[(indices[lane] as u64 % 16) as usize]);
            }
        }
        let words = [i32::MIN, -1, 17, i32::MAX];
        let broadcast: [i32; 16] = transmute(_mm512_broadcast_i32x4(transmute(words)));
        for lane in 0..16 {
            assert_eq!(broadcast[lane], words[lane % 4]);
        }
        let mut memory = Memory([0xa5; 80]);
        _mm512_storeu_si512(memory.0.as_mut_ptr().add(3) as *mut __m512i, va);
        let mut expected = [0xa5; 80];
        for lane in 0..8 {
            for byte in 0..8 {
                expected[3 + 8 * lane + byte] = ((a[lane] as u64) >> (8 * byte)) as u8;
            }
        }
        assert_eq!(memory.0, expected);
        check_shuffles::<0>();
        check_shuffles::<0x1b>();
        check_shuffles::<0x39>();
        check_shuffles::<0xe4>();
        check_shuffles::<255>();
        check_rotate512::<0>();
        check_rotate512::<1>();
        check_rotate512::<31>();
        check_rotate512::<32>();
        check_rotate512::<33>();
        check_rotate512::<255>();
        let input = transmute::<[u32; 16], __m512i>(ROTATE_INPUT);
        let dwords: [u32; 16] = transmute(_mm512_shuffle_epi32(input, 0x1b));
        let blocks: [u32; 16] = transmute(_mm512_shuffle_i32x4(input, input, 0x1b));
        let rotated: [u32; 16] = transmute(_mm512_rol_epi32(input, 33));
        for lane in 0..16 {
            assert_eq!(dwords[lane], ROTATE_INPUT[(lane / 4) * 4 + 3 - lane % 4]);
            assert_eq!(blocks[lane], ROTATE_INPUT[(3 - lane / 4) * 4 + lane % 4]);
            assert_eq!(rotated[lane], rotate_reference(ROTATE_INPUT[lane], 33));
        }
    }

    #[test]
    fn avx512_lanes_shuffles_rotates_and_store() {
        if is_x86_feature_detected!("avx512f") {
            unsafe { check_avx512() };
        }
    }

    #[target_feature(enable = "avx512f,avx512vl")]
    unsafe fn check_avx512vl() {
        let a = [i64::MIN, -1, 17, i64::MAX];
        let b = [33i64, -71, 123456789, -123456789];
        let mut table = [0i64; 8];
        for lane in 0..4 {
            table[lane] = a[lane];
            table[lane + 4] = b[lane];
        }
        for indices in [[0i64, 3, 4, 7], [-1, i64::MIN, 0x123456789abcde19, -2], [1, 2, 5, 6]] {
            let actual: [i64; 4] = transmute(_mm256_permutex2var_epi64(
                transmute(a), transmute(indices), transmute(b),
            ));
            for lane in 0..4 {
                assert_eq!(actual[lane], table[(indices[lane] as u64 % 8) as usize]);
            }
        }
        let words = [i32::MIN, -1, 17, i32::MAX];
        let v = transmute::<[i32; 4], __m128i>(words);
        let broadcast: [i32; 8] = transmute(_mm256_broadcast_i32x4(v));
        for lane in 0..8 {
            assert_eq!(broadcast[lane], words[lane % 4]);
        }
        let mut source = Memory([0x5a; 80]);
        for lane in 0..4 {
            for byte in 0..4 {
                source.0[1 + 4 * lane + byte] = ((words[lane] as u32) >> (8 * byte)) as u8;
            }
        }
        let loaded: [i32; 4] = transmute(_mm_loadu_epi32(source.0.as_ptr().add(1) as *const i32));
        assert_eq!(loaded, words);
        let mut memory = Memory([0xa5; 80]);
        _mm_storeu_epi32(memory.0.as_mut_ptr().add(3) as *mut i32, v);
        let mut expected = [0xa5; 80];
        for byte in 0..16 {
            expected[3 + byte] = source.0[1 + byte];
        }
        assert_eq!(memory.0, expected);

        let wide = [i32::MIN, -7, 0, 1, 33, -456, 987654321, i32::MAX];
        let vwide = transmute::<[i32; 8], __m256i>(wide);
        let mut memory = Memory([0xa5; 80]);
        _mm256_storeu_epi32(memory.0.as_mut_ptr().add(1) as *mut i32, vwide);
        let mut expected = [0xa5; 80];
        for lane in 0..8 {
            for byte in 0..4 {
                expected[1 + 4 * lane + byte] = ((wide[lane] as u32) >> (8 * byte)) as u8;
            }
        }
        assert_eq!(memory.0, expected);
        check_rotate_vl::<0>();
        check_rotate_vl::<1>();
        check_rotate_vl::<31>();
        check_rotate_vl::<32>();
        check_rotate_vl::<33>();
        check_rotate_vl::<255>();
        let narrow: [u32; 4] = transmute(_mm_rol_epi32(v, 33));
        let wide_result: [u32; 8] = transmute(_mm256_rol_epi32(vwide, 255));
        for lane in 0..4 {
            assert_eq!(narrow[lane], rotate_reference(words[lane] as u32, 33));
        }
        for lane in 0..8 {
            assert_eq!(wide_result[lane], rotate_reference(wide[lane] as u32, 255));
        }
    }

    #[test]
    fn avx512vl_lanes_rotates_and_unaligned_memory() {
        if is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("avx512vl") {
            unsafe { check_avx512vl() };
        }
    }

    #[target_feature(enable = "avx512ifma,avx512vl")]
    unsafe fn check_ifma() {
        let mask = (1u64 << 52) - 1;
        let a = [u64::MAX, u64::MAX - 7, 0xfedcba9876543210, 0x8000000000000000];
        for (b, c) in [
            ([mask, u64::MAX, 1u64 << 52, 1u64 << 51], [mask, mask, u64::MAX, 3]),
            ([0, 1, 0xfed123456789abcd, 0xabcfffffffffffff], [u64::MAX, u64::MAX, 0xdef9876543210123, 1]),
        ] {
            let low: [u64; 4] = transmute(_mm256_madd52lo_epu64(transmute(a), transmute(b), transmute(c)));
            let high: [u64; 4] = transmute(_mm256_madd52hi_epu64(transmute(a), transmute(b), transmute(c)));
            for lane in 0..4 {
                // Scalar binary long multiplication ignores bits 52..63.
                let mut product = 0u128;
                for bit in 0..52 {
                    if (c[lane] >> bit) & 1 != 0 {
                        product += ((b[lane] & mask) as u128) << bit;
                    }
                }
                let expected_low = (a[lane] as u128 + product % (1u128 << 52)) as u64;
                let expected_high = (a[lane] as u128 + product / (1u128 << 52)) as u64;
                assert_eq!(low[lane], expected_low);
                assert_eq!(high[lane], expected_high);
            }
        }
    }

    #[test]
    fn ifma_52_bit_products_and_wrapping_accumulators() {
        if is_x86_feature_detected!("avx512ifma") && is_x86_feature_detected!("avx512vl") {
            unsafe { check_ifma() };
        }
    }

    #[target_feature(enable = "rdrand")]
    unsafe fn check_rdrand() {
        let mut word = u32::MAX;
        let mut qword = u64::MAX;
        let status32 = _rdrand32_step(&mut word);
        let status64 = _rdrand64_step(&mut qword);
        // Hardware is nondeterministic and may fail. Relax's software path
        // always fails. The common contract is a 0/1 status and zero on failure.
        assert_eq!(status32 == 0 || status32 == 1, true);
        assert_eq!(status64 == 0 || status64 == 1, true);
        if status32 == 0 {
            assert_eq!(word, 0);
        }
        if status64 == 0 {
            assert_eq!(qword, 0);
        }
    }

    #[test]
    fn rdrand_status_and_failure_outputs() {
        if is_x86_feature_detected!("rdrand") {
            unsafe { check_rdrand() };
        }
    }
}
