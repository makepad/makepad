// The four aarch64 AES intrinsics (vaeseq_u8, vaesdq_u8, vaesmcq_u8 and vaesimcq_u8: FEAT_AES, the
// backend the aes crate takes on aarch64). FIPS-197's example vectors are encrypted and decrypted
// with them the way that backend does, key schedule included, and its round-one states are checked
// one instruction at a time. Each intrinsic is also folded over every byte in every position and
// over pseudo-random states into one hash, and the expected hashes are what the real instructions
// give (this file built with rustc on an Apple M-series CPU), so the emulation has to agree with
// the CPU bit for bit. The "| " lines are equal under rustc --test and makepad test.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
use std::mem::transmute;

const ZERO: [u8; 16] = [0; 16];

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn state(hex: &str) -> uint8x16_t {
    let mut b = [0u8; 16];
    b.copy_from_slice(&bytes(hex));
    unsafe { transmute(b) }
}

fn hex(v: uint8x16_t) -> String {
    let b: [u8; 16] = unsafe { transmute(v) };
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn xor(a: uint8x16_t, b: uint8x16_t) -> uint8x16_t {
    let (a, b): (u128, u128) = unsafe { (transmute(a), transmute(b)) };
    unsafe { transmute(a ^ b) }
}

/// SubWord: ShiftRows does not move a state of four equal columns, so AESE with a zero key is the
/// S-box on each byte.
fn sub_word(w: u32) -> u32 {
    let s: [u32; 4] = unsafe { transmute(vaeseq_u8(transmute([w; 4]), transmute(ZERO))) };
    s[0]
}

/// FIPS-197's key expansion for a 16, 24 or 32 byte key; the words are in memory order.
fn expand(key: &[u8]) -> Vec<uint8x16_t> {
    let nk = key.len() / 4;
    let mut w: Vec<u32> = key.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let mut rcon = 1u32;
    for i in nk..4 * (nk + 7) {
        let mut t = w[i - 1];
        if i % nk == 0 {
            t = sub_word(t).rotate_right(8) ^ rcon;
            rcon = (rcon << 1) ^ if rcon & 0x80 != 0 { 0x11b } else { 0 };
        } else if nk > 6 && i % nk == 4 {
            t = sub_word(t);
        }
        w.push(w[i - nk] ^ t);
    }
    w.chunks_exact(4).map(|c| unsafe { transmute([c[0], c[1], c[2], c[3]]) }).collect()
}

fn encrypt(keys: &[uint8x16_t], block: uint8x16_t) -> uint8x16_t {
    let last = keys.len() - 1;
    let mut b = block;
    unsafe {
        for key in &keys[..last - 1] {
            b = vaesmcq_u8(vaeseq_u8(b, *key));
        }
        xor(vaeseq_u8(b, keys[last - 1]), keys[last])
    }
}

/// The equivalent inverse cipher: the middle round keys go through InvMixColumns.
fn decrypt(keys: &[uint8x16_t], block: uint8x16_t) -> uint8x16_t {
    let last = keys.len() - 1;
    unsafe {
        let mut b = vaesimcq_u8(vaesdq_u8(block, keys[last]));
        for i in (2..last).rev() {
            b = vaesimcq_u8(vaesdq_u8(b, vaesimcq_u8(keys[i])));
        }
        xor(vaesdq_u8(b, vaesimcq_u8(keys[1])), keys[0])
    }
}

#[test]
fn a01_fips_197_vectors() {
    if !std::arch::is_aarch64_feature_detected!("aes") {
        return;
    }
    // (key, plaintext, ciphertext, last round key): Appendix B, then C.1, C.2 and C.3
    let plain = "00112233445566778899aabbccddeeff";
    let cases = [
        ("2b7e151628aed2a6abf7158809cf4f3c", "3243f6a8885a308d313198a2e0370734", "3925841d02dc09fbdc118597196a0b32", "d014f9a8c9ee2589e13f0cc8b6630ca6"),
        ("000102030405060708090a0b0c0d0e0f", plain, "69c4e0d86a7b0430d8cdb78070b4c55a", "13111d7fe3944a17f307a78b4d2b30c5"),
        (
            "000102030405060708090a0b0c0d0e0f1011121314151617",
            plain,
            "dda97ca4864cdfe06eaf70a0ec0d7191",
            "a4970a331a78dc09c418c271e3a41d5d",
        ),
        (
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            plain,
            "8ea2b7ca516745bfeafc49904b496089",
            "24fc79ccbf0979e9371ac23c6d68de36",
        ),
    ];
    for (key, plain, cipher, last_key) in cases {
        let keys = expand(&bytes(key));
        assert_eq!(keys.len(), key.len() / 8 + 7);
        assert_eq!(hex(keys[keys.len() - 1]), last_key, "the key schedule of {key}");
        let c = encrypt(&keys, state(plain));
        println!("| aes-{} {} -> {}", key.len() * 4, plain, hex(c));
        assert_eq!(hex(c), cipher, "encrypting with {key}");
        assert_eq!(hex(decrypt(&keys, c)), plain, "decrypting with {key}");
    }
}

#[test]
fn a02_one_round_one_instruction_at_a_time() {
    if !std::arch::is_aarch64_feature_detected!("aes") {
        return;
    }
    unsafe {
        let zero: uint8x16_t = transmute(ZERO);
        // Appendix B, round 1: AESE adds the round key, then SubBytes and ShiftRows; AESMC mixes
        let s_row = vaeseq_u8(state("3243f6a8885a308d313198a2e0370734"), state("2b7e151628aed2a6abf7158809cf4f3c"));
        assert_eq!(hex(s_row), "d4bf5d30e0b452aeb84111f11e2798e5");
        let m_col = vaesmcq_u8(s_row);
        assert_eq!(hex(m_col), "046681e5e0cb199a48f8d37a2806264c");
        assert_eq!(hex(vaesimcq_u8(m_col)), "d4bf5d30e0b452aeb84111f11e2798e5");
        // AESD with a zero key takes the s_row state back to the round's start
        assert_eq!(hex(vaesdq_u8(s_row, zero)), "193de3bea0f4e22b9ac68d2ae9f84808");
        // Appendix C.1, round 1
        let s_row = vaeseq_u8(state("00112233445566778899aabbccddeeff"), state("000102030405060708090a0b0c0d0e0f"));
        assert_eq!(hex(s_row), "6353e08c0960e104cd70b751bacad0e7");
        assert_eq!(hex(vaesmcq_u8(s_row)), "5f72641557f5bc92f7be3b291db9f91a");
        // one byte's way: byte 5 is row 1 of column 1, which ShiftRows takes to column 0 and
        // InvShiftRows to column 2; S(53) = ed, InvS(53) = 50, S(00) = 63, InvS(00) = 52
        let one = state("00000000005300000000000000000000");
        assert_eq!(hex(vaeseq_u8(one, zero)), "63ed6363636363636363636363636363");
        assert_eq!(hex(vaesdq_u8(one, zero)), "52525252525252525250525252525252");
        // the key is added before the substitution, so data and key are interchangeable
        assert_eq!(hex(vaeseq_u8(zero, one)), "63ed6363636363636363636363636363");
        assert_eq!(hex(vaesdq_u8(zero, one)), "52525252525252525250525252525252");
        // MixColumns of one column (FIPS-197 5.1.3: 02 03 01 01 down the first column of the matrix)
        assert_eq!(hex(vaesmcq_u8(state("01000000000100000000010000000001"))), "02010103030201010103020101010302");
        assert_eq!(hex(vaesimcq_u8(state("01000000000100000000010000000001"))), "0e090d0b0b0e090d0d0b0e09090d0b0e");
        println!("| round one ok");
    }
}

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn random(s: &mut u64) -> uint8x16_t {
    unsafe { transmute([next(s), next(s)]) }
}

/// Byte `case % 256` at position `case / 256`, zero elsewhere.
fn one_byte(case: usize) -> uint8x16_t {
    let mut b = ZERO;
    b[case / 256] = case as u8;
    unsafe { transmute(b) }
}

/// FNV-1a over the bytes of what `f` returns for each of `n` cases.
fn fold<T: Copy>(n: usize, mut f: impl FnMut(&mut u64, usize) -> T) -> u64 {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for case in 0..n {
        let v = f(&mut s, case);
        let b = unsafe { std::slice::from_raw_parts(&v as *const T as *const u8, std::mem::size_of::<T>()) };
        for &x in b {
            h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

fn expect(name: &str, hash: u64) {
    println!("| {name} {hash:016x}");
    let real = REAL.iter().find(|(n, _)| *n == name).expect(name).1;
    assert_eq!(hash, real, "{name}: the real instruction gives {real:016x}");
}

#[test]
fn a03_each_instruction() {
    if !std::arch::is_aarch64_feature_detected!("aes") {
        return;
    }
    unsafe {
        let zero: uint8x16_t = transmute(ZERO);
        // both S-boxes and every row's shift: each byte value through each of the 16 positions
        expect("vaeseq_u8 every byte", fold(4096, |_, case| vaeseq_u8(one_byte(case), zero)));
        expect("vaesdq_u8 every byte", fold(4096, |_, case| vaesdq_u8(one_byte(case), zero)));
        expect("vaeseq_u8 every key byte", fold(4096, |s, case| vaeseq_u8(random(s), one_byte(case))));
        expect("vaesdq_u8 every key byte", fold(4096, |s, case| vaesdq_u8(random(s), one_byte(case))));
        // both matrices: each byte value through each row of each column
        expect("vaesmcq_u8 every byte", fold(4096, |_, case| vaesmcq_u8(one_byte(case))));
        expect("vaesimcq_u8 every byte", fold(4096, |_, case| vaesimcq_u8(one_byte(case))));
        expect("vaeseq_u8", fold(4096, |s, _| vaeseq_u8(random(s), random(s))));
        expect("vaesdq_u8", fold(4096, |s, _| vaesdq_u8(random(s), random(s))));
        expect("vaesmcq_u8", fold(4096, |s, _| vaesmcq_u8(random(s))));
        expect("vaesimcq_u8", fold(4096, |s, _| vaesimcq_u8(random(s))));
        // rounds feeding rounds, both directions at once
        let mut chain = [random(&mut 7), random(&mut 11)];
        expect(
            "round chain",
            fold(2048, |s, _| {
                let key = random(s);
                chain = [vaesmcq_u8(vaeseq_u8(chain[0], key)), vaesimcq_u8(vaesdq_u8(chain[1], chain[0]))];
                chain
            }),
        );
    }
}

#[test]
fn a04_inverses() {
    if !std::arch::is_aarch64_feature_detected!("aes") {
        return;
    }
    unsafe {
        let zero: uint8x16_t = transmute(ZERO);
        let mut s = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..2048 {
            let (x, k) = (random(&mut s), random(&mut s));
            assert_eq!(hex(vaesimcq_u8(vaesmcq_u8(x))), hex(x));
            assert_eq!(hex(vaesmcq_u8(vaesimcq_u8(x))), hex(x));
            assert_eq!(hex(vaesdq_u8(vaeseq_u8(x, k), zero)), hex(xor(x, k)));
            assert_eq!(hex(vaeseq_u8(vaesdq_u8(x, k), zero)), hex(xor(x, k)));
        }
        println!("| inverses ok");
    }
}

/// What the real instructions give: every hash above, from this file built with rustc.
const REAL: &[(&str, u64)] = &[
    ("vaeseq_u8 every byte", 0x94d1aee91faed7a5),
    ("vaesdq_u8 every byte", 0x22cb937eecc41b25),
    ("vaeseq_u8 every key byte", 0x6fa4625bfc327fa0),
    ("vaesdq_u8 every key byte", 0x41b19b69e35ba9f1),
    ("vaesmcq_u8 every byte", 0xfcffa98c4fedda25),
    ("vaesimcq_u8 every byte", 0x2db314cecfcb8925),
    ("vaeseq_u8", 0xb8584b84b6fd128b),
    ("vaesdq_u8", 0x5ad4295c42ecd0b6),
    ("vaesmcq_u8", 0x1ae7231200d80579),
    ("vaesimcq_u8", 0x89122ad9a0379d1d),
    ("round chain", 0x41fa71e79a27ca5d),
];
