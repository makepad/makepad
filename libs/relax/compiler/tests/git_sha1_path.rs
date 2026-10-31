// makepad_git's own SHA-1 (libs/git/src/sha1.rs, the hashing Scope's indexing runs) under Relax: its dispatch picks the
// hardware path (SHA-NI on x86_64 when the CPU has it, Arm SHA1 always on aarch64); every input length 0..=300 and a
// few large inputs must hash as a scalar reference SHA-1 does. Also runs that file's own unit tests (cfg(test)).
// `makepad test libs/relax/compiler/tests/git_sha1_path.rs`; also runnable with rustc -O --test.
#![allow(dead_code)]

#[path = "../../../../libs/git/src/sha1.rs"]
mod git_sha1;

fn reference(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for i in 0..5 {
        out[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

fn data(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(len as u32) | 1;
    (0..len).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; (x >> 7) as u8 }).collect()
}

#[test]
fn git_sha1_matches_reference() {
    #[cfg(target_arch = "x86_64")]
    println!("hardware path: sha={}", std::is_x86_feature_detected!("sha"));
    let mut lens: Vec<usize> = (0..=300).collect();
    lens.extend_from_slice(&[1000, 4095, 4096, 4097, 65536, 1 << 20]);
    for (k, &len) in lens.iter().enumerate() {
        let d = data(len, k as u32);
        let mut one = git_sha1::Sha1::new();
        one.update(&d);
        assert_eq!(one.finalize(), reference(&d), "one-shot, len {len}");
        // split updates cross the 64-byte buffer at every offset class
        let mut split = git_sha1::Sha1::new();
        let cut = (len * 7 / 13).min(len);
        split.update(&d[..cut]);
        split.update(&d[cut..]);
        assert_eq!(split.finalize(), reference(&d), "split at {cut}, len {len}");
    }
    let mut abc = git_sha1::Sha1::new();
    abc.update(b"abc");
    let hex: String = abc.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
}
