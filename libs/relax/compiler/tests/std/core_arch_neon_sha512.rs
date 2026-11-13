// The four aarch64 SHA-512 intrinsics (vsha512hq_u64, vsha512h2q_u64, vsha512su0q_u64 and
// vsha512su1q_u64: FEAT_SHA512, the backend sha2 takes on Apple silicon). Whole digests are
// compressed with them the way sha2 does and compared with FIPS 180-4's answers. Each intrinsic is
// also folded over edge and pseudo-random operands into one hash, and the expected hashes are
// what the real instructions give (this file built with rustc on an Apple M-series CPU), so the
// emulation has to agree with the CPU bit for bit. The "| " lines are equal under rustc --test and
// makepad test.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
use std::mem::transmute;

const K: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// One block, two rounds at a time, as sha2's aarch64 backend chains the four instructions: the
/// roles of the hash pairs (a,b), (c,d), (e,f), (g,h) move one place along `h` every two rounds.
unsafe fn compress(state: &mut [u64; 8], block: &[u8]) {
    let mut h = [vld1q_u64(state.as_ptr()), vld1q_u64(state.as_ptr().add(2)), vld1q_u64(state.as_ptr().add(4)), vld1q_u64(state.as_ptr().add(6))];
    let before = h;
    let mut w = [vdupq_n_u64(0); 8];
    for i in 0..8 {
        w[i] = vreinterpretq_u64_u8(vrev64q_u8(vld1q_u8(block.as_ptr().add(16 * i))));
    }
    for pair in 0..40 {
        if pair >= 8 {
            let tail = vextq_u64::<1>(w[(pair + 4) % 8], w[(pair + 5) % 8]);
            w[pair % 8] = vsha512su1q_u64(vsha512su0q_u64(w[pair % 8], w[(pair + 1) % 8]), w[(pair + 7) % 8], tail);
        }
        let (ab, cd, ef, gh) = ((40 - pair) % 4, (41 - pair) % 4, (42 - pair) % 4, (43 - pair) % 4);
        let kw = vaddq_u64(w[pair % 8], vld1q_u64(K.as_ptr().add(2 * pair)));
        let sum = vaddq_u64(vextq_u64::<1>(kw, kw), h[gh]);
        let t = vsha512hq_u64(sum, vextq_u64::<1>(h[ef], h[gh]), vextq_u64::<1>(h[cd], h[ef]));
        h[gh] = vsha512h2q_u64(t, h[cd], h[ab]);
        h[cd] = vaddq_u64(h[cd], t);
    }
    for i in 0..4 {
        vst1q_u64(state.as_mut_ptr().add(2 * i), vaddq_u64(h[i], before[i]));
    }
}

fn sha512(message: &[u8]) -> String {
    let mut state = [
        0x6a09e667f3bcc908u64, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
        0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
    ];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 128 != 112 {
        padded.push(0);
    }
    padded.extend_from_slice(&(message.len() as u128 * 8).to_be_bytes());
    for block in padded.chunks_exact(128) {
        unsafe { compress(&mut state, block) };
    }
    state.iter().map(|x| format!("{x:016x}")).collect()
}

#[test]
fn a01_digests() {
    if !std::arch::is_aarch64_feature_detected!("sha3") {
        return;
    }
    // FIPS 180-4's one-block and two-block examples, the empty message, the padding's 111 / 112
    // byte edge and a thousand bytes
    let two_blocks = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";
    let counting: Vec<u8> = (0..112u8).collect();
    let thousand: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
    let cases: [(&[u8], &str); 6] = [
        (b"abc", "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"),
        (b"", "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"),
        (two_blocks, "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"),
        (&counting[..111], "a1a111449b198d9b1f538bad7f3fc1022b3a5b1a5e90a0bc860de8512746cbc31599e6c834de3a3235327af0b51ff57bf7acf1974a73014d9c3953812edc7c8d"),
        (&counting, "c5fbd731d19d2ae1180f001be72c2c1aaba1d7b094b3748880e24593b8e117a750e11c1bd867cc2f96dace8c8b74abd2d5c4f236be444e77d30d1916174070b9"),
        (&thousand, "00e36fccf193e59697a92b5ab24666ce6326d7fa16bf10832d0991ddc591112e9dfa6a636950ed9c4d67344a760654c2ff7785e1d60094d651038735b5dccabd"),
    ];
    for (message, digest) in cases {
        let got = sha512(message);
        println!("| sha512 of {} bytes {}", message.len(), &got[..32]);
        assert_eq!(got, digest, "SHA-512 of {} bytes", message.len());
    }
}

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// Two 64-bit lanes: random, or an edge pattern in both (zero, all ones, one bit, all but one).
fn lanes(s: &mut u64) -> uint64x2_t {
    let mode = next(s) % 8;
    let mut w = [0u64; 2];
    for x in w.iter_mut() {
        let r = next(s);
        *x = match mode {
            0 => 0,
            1 => u64::MAX,
            2 => 1 << (r % 64),
            3 => !(1 << (r % 64)),
            _ => r,
        };
    }
    unsafe { transmute(w) }
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
fn a02_each_instruction() {
    if !std::arch::is_aarch64_feature_detected!("sha3") {
        return;
    }
    unsafe {
        // one bit of one operand at a time: which lanes and rotations each bit reaches
        let one_bit = |case: usize, arg: usize| -> uint64x2_t {
            let bit = case % 128;
            let on = case / 128 == arg;
            transmute([if on && bit < 64 { 1u64 << bit } else { 0 }, if on && bit >= 64 { 1u64 << (bit - 64) } else { 0 }])
        };
        expect("vsha512hq_u64 one bit", fold(384, |_, case| vsha512hq_u64(one_bit(case, 0), one_bit(case, 1), one_bit(case, 2))));
        expect("vsha512h2q_u64 one bit", fold(384, |_, case| vsha512h2q_u64(one_bit(case, 0), one_bit(case, 1), one_bit(case, 2))));
        expect("vsha512su0q_u64 one bit", fold(256, |_, case| vsha512su0q_u64(one_bit(case, 0), one_bit(case, 1))));
        expect("vsha512su1q_u64 one bit", fold(384, |_, case| vsha512su1q_u64(one_bit(case, 0), one_bit(case, 1), one_bit(case, 2))));
        expect("vsha512hq_u64", fold(4096, |s, _| vsha512hq_u64(lanes(s), lanes(s), lanes(s))));
        expect("vsha512h2q_u64", fold(4096, |s, _| vsha512h2q_u64(lanes(s), lanes(s), lanes(s))));
        expect("vsha512su0q_u64", fold(4096, |s, _| vsha512su0q_u64(lanes(s), lanes(s))));
        expect("vsha512su1q_u64", fold(4096, |s, _| vsha512su1q_u64(lanes(s), lanes(s), lanes(s))));
    }
}

/// What the real instructions give: every hash above, from this file built with rustc.
const REAL: &[(&str, u64)] = &[
    ("vsha512hq_u64 one bit", 0xcefa7d4eee4f4489),
    ("vsha512h2q_u64 one bit", 0x19a874492e258c75),
    ("vsha512su0q_u64 one bit", 0x48839a3f4a6ff335),
    ("vsha512su1q_u64 one bit", 0xa4ec24a5c5188405),
    ("vsha512hq_u64", 0xb4f79c1c3a469d67),
    ("vsha512h2q_u64", 0x96eca8cf37e98dfb),
    ("vsha512su0q_u64", 0xac33fb41c407c398),
    ("vsha512su1q_u64", 0x319f05d02697c8ba),
];
