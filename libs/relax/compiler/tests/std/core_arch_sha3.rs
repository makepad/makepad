// core::arch::aarch64 SHA-512 and SHA-3 intrinsics (relax #108): vsha512hq/h2q/su0q/su1q_u64,
// vrax1q_u64, vxarq_u64 and veor3q/vbcaxq for every 128-bit integer type, on edge and random lanes,
// and SHA-512 itself compressed with them (the digests of "abc" and a three-block message). The
// `| ` lines must equal real std's, where the M-series CPU runs the instructions; the file is empty
// elsewhere.
#![cfg(target_arch = "aarch64")]
use core::arch::aarch64::*;

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn lane(s: &mut u64, i: u32) -> u64 {
    match i % 7 {
        0 => !0,
        1 => 0,
        2 => 1u64 << (next(s) % 64),
        _ => next(s),
    }
}

fn q(a: [u64; 2]) -> uint64x2_t {
    unsafe { vld1q_u64(a.as_ptr()) }
}

fn out(r: uint64x2_t) -> [u64; 2] {
    let mut a = [0u64; 2];
    unsafe { vst1q_u64(a.as_mut_ptr(), r) };
    a
}

// FNV-1a over the lanes of many results: one line per intrinsic instead of thousands.
fn fold(h: &mut u64, r: [u64; 2]) {
    for b in r.iter().flat_map(|x| x.to_le_bytes()) {
        *h = (*h ^ b as u64).wrapping_mul(0x100000001b3);
    }
}

#[test]
fn a01_sha512_intrinsics() {
    let mut s = 0x9e3779b97f4a7c15u64;
    let mut sums = [0xcbf29ce484222325u64; 6];
    for i in 0..3000u32 {
        let a = [lane(&mut s, i), lane(&mut s, i / 3)];
        let b = [lane(&mut s, i / 5), lane(&mut s, i)];
        let c = [lane(&mut s, i / 2), lane(&mut s, i / 7)];
        let r = unsafe {
            [
                out(vsha512hq_u64(q(a), q(b), q(c))),
                out(vsha512h2q_u64(q(a), q(b), q(c))),
                out(vsha512su0q_u64(q(a), q(b))),
                out(vsha512su1q_u64(q(a), q(b), q(c))),
                out(vrax1q_u64(q(a), q(b))),
                out(vxarq_u64::<29>(q(a), q(b))),
            ]
        };
        if i < 6 {
            println!("| {i} {:016x?}", r);
        }
        for (h, r) in sums.iter_mut().zip(r) {
            fold(h, r);
        }
    }
    println!("| h h2 su0 su1 rax1 xar {:016x?}", sums);
    let (a, b) = (q([0x0123456789abcdef, 0xfedcba9876543210]), q([0x8000000000000001, 0x7fffffffffffffff]));
    let mut xar = 0xcbf29ce484222325u64;
    unsafe {
        fold(&mut xar, out(vxarq_u64::<0>(a, b)));
        fold(&mut xar, out(vxarq_u64::<1>(a, b)));
        fold(&mut xar, out(vxarq_u64::<31>(a, b)));
        fold(&mut xar, out(vxarq_u64::<32>(a, b)));
        fold(&mut xar, out(vxarq_u64::<63>(a, b)));
        println!("| xar {:016x} {:016x?}", xar, out(vxarq_u64(a, b, 17)));
    }
}

#[test]
fn a02_eor3_bcax() {
    let mut s = 0x2545f4914f6cdd1du64;
    let mut v = || {
        let mut a = [0u8; 16];
        for c in a.chunks_mut(8) {
            c.copy_from_slice(&next(&mut s).to_le_bytes());
        }
        a
    };
    let (x, y, z) = (v(), v(), v());
    unsafe {
        let (a, b, c) = (vld1q_u8(x.as_ptr()), vld1q_u8(y.as_ptr()), vld1q_u8(z.as_ptr()));
        let bytes = |r: uint8x16_t| {
            let mut o = [0u8; 16];
            vst1q_u8(o.as_mut_ptr(), r);
            o
        };
        println!("| u8 {:02x?} {:02x?}", bytes(veor3q_u8(a, b, c)), bytes(vbcaxq_u8(a, b, c)));
        let s8 = |r: int8x16_t| bytes(vreinterpretq_u8_s8(r));
        let (a8, b8, c8) = (vreinterpretq_s8_u8(a), vreinterpretq_s8_u8(b), vreinterpretq_s8_u8(c));
        println!("| s8 {:02x?} {:02x?}", s8(veor3q_s8(a8, b8, c8)), s8(vbcaxq_s8(a8, b8, c8)));
        let (a, b, c) = (vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b), vreinterpretq_u16_u8(c));
        println!("| u16 {:02x?} {:02x?}", bytes(vreinterpretq_u8_u16(veor3q_u16(a, b, c))), bytes(vreinterpretq_u8_u16(vbcaxq_u16(a, b, c))));
        let (a, b, c) = (vreinterpretq_s16_u16(a), vreinterpretq_s16_u16(b), vreinterpretq_s16_u16(c));
        println!("| s16 {:02x?} {:02x?}", bytes(vreinterpretq_u8_s16(veor3q_s16(a, b, c))), bytes(vreinterpretq_u8_s16(vbcaxq_s16(a, b, c))));
        let (a, b, c) = (vreinterpretq_u32_s16(a), vreinterpretq_u32_s16(b), vreinterpretq_u32_s16(c));
        println!("| u32 {:02x?} {:02x?}", bytes(vreinterpretq_u8_u32(veor3q_u32(a, b, c))), bytes(vreinterpretq_u8_u32(vbcaxq_u32(a, b, c))));
        let (a, b, c) = (vreinterpretq_s32_u32(a), vreinterpretq_s32_u32(b), vreinterpretq_s32_u32(c));
        println!("| s32 {:02x?} {:02x?}", bytes(vreinterpretq_u8_s32(veor3q_s32(a, b, c))), bytes(vreinterpretq_u8_s32(vbcaxq_s32(a, b, c))));
        let (a, b, c) = (vreinterpretq_u64_s32(a), vreinterpretq_u64_s32(b), vreinterpretq_u64_s32(c));
        println!("| u64 {:02x?} {:02x?}", bytes(vreinterpretq_u8_u64(veor3q_u64(a, b, c))), bytes(vreinterpretq_u8_u64(vbcaxq_u64(a, b, c))));
        let (a, b, c) = (vreinterpretq_s64_u64(a), vreinterpretq_s64_u64(b), vreinterpretq_s64_u64(c));
        println!("| s64 {:02x?} {:02x?}", bytes(vreinterpretq_u8_s64(veor3q_s64(a, b, c))), bytes(vreinterpretq_u8_s64(vbcaxq_s64(a, b, c))));
    }
}

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

// One 128-byte block, two rounds per step: the state's four pairs (ab, cd, ef, gh) trade roles each
// step, and the schedule keeps eight pairs of words.
fn compress(state: &mut [u64; 8], block: &[u8]) {
    unsafe {
        let start = [q([state[0], state[1]]), q([state[2], state[3]]), q([state[4], state[5]]), q([state[6], state[7]])];
        let mut w = [q([0, 0]); 8];
        for (j, w) in w.iter_mut().enumerate() {
            *w = vreinterpretq_u64_u8(vrev64q_u8(vld1q_u8(block[j * 16..].as_ptr())));
        }
        let (mut a, mut c, mut e, mut g) = (start[0], start[1], start[2], start[3]);
        for i in 0..40 {
            if i >= 8 {
                let j = i % 8;
                w[j] = vsha512su1q_u64(vsha512su0q_u64(w[j], w[(j + 1) % 8]), w[(j + 7) % 8], vextq_u64(w[(j + 4) % 8], w[(j + 5) % 8], 1));
            }
            let wk = vaddq_u64(w[i % 8], vld1q_u64(K[2 * i..].as_ptr()));
            let sum = vaddq_u64(vextq_u64(wk, wk, 1), g);
            let mid = vsha512hq_u64(sum, vextq_u64(e, g, 1), vextq_u64(c, e, 1));
            let g2 = vsha512h2q_u64(mid, c, a);
            let c2 = vaddq_u64(c, mid);
            (a, c, e, g) = (g2, a, c2, e);
        }
        let end = [vaddq_u64(start[0], a), vaddq_u64(start[1], c), vaddq_u64(start[2], e), vaddq_u64(start[3], g)];
        for (k, r) in end.into_iter().enumerate() {
            let r = out(r);
            state[2 * k] = r[0];
            state[2 * k + 1] = r[1];
        }
    }
}

fn sha512(msg: &[u8]) -> String {
    let mut state: [u64; 8] = [
        0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
        0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
    ];
    let mut data = msg.to_vec();
    data.push(0x80);
    while data.len() % 128 != 112 {
        data.push(0);
    }
    data.extend_from_slice(&((msg.len() as u128) * 8).to_be_bytes());
    for block in data.chunks(128) {
        compress(&mut state, block);
    }
    state.iter().map(|x| format!("{x:016x}")).collect()
}

#[test]
fn a03_sha512_digest() {
    println!("| abc {}", sha512(b"abc"));
    println!("| 200 {}", sha512(&(0..200u8).collect::<Vec<u8>>()));
    println!("| empty {}", &sha512(b"")[..32]);
}
