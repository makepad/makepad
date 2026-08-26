//! Self-contained streaming SHA-256 (FIPS 180-4). Kept in-crate for the same
//! reason game_net's auth.rs and game_pkg keep theirs: the shared content
//! contract names bytes by digest and must not pull a transport crate to do it.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Incremental hasher, so a server can hash a manifest or blob while streaming
/// it instead of buffering the whole file.
pub struct Sha256 {
    h: [u32; 8],
    block: [u8; 64],
    block_len: usize,
    total_len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            h: H0,
            block: [0; 64],
            block_len: 0,
            total_len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.block_len > 0 {
            let take = (64 - self.block_len).min(data.len());
            self.block[self.block_len..self.block_len + take].copy_from_slice(&data[..take]);
            self.block_len += take;
            data = &data[take..];
            if self.block_len == 64 {
                let block = self.block;
                self.compress(&block);
                self.block_len = 0;
            }
            if data.is_empty() {
                // A still-partial buffered block must survive; the remainder
                // write below would zero block_len and drop it.
                return;
            }
        }
        let mut chunks = data.chunks_exact(64);
        for chunk in &mut chunks {
            let mut block = [0u8; 64];
            block.copy_from_slice(chunk);
            self.compress(&block);
        }
        let rest = chunks.remainder();
        self.block[..rest.len()].copy_from_slice(rest);
        self.block_len = rest.len();
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        let mut tail = [0u8; 128];
        tail[..self.block_len].copy_from_slice(&self.block[..self.block_len]);
        tail[self.block_len] = 0x80;
        // One padded block if the length fits, otherwise two.
        let tail_len = if self.block_len < 56 { 64 } else { 128 };
        tail[tail_len - 8..tail_len].copy_from_slice(&bit_len.to_be_bytes());
        let mut block = [0u8; 64];
        block.copy_from_slice(&tail[..64]);
        self.compress(&block);
        if tail_len == 128 {
            block.copy_from_slice(&tail[64..]);
            self.compress(&block);
        }
        let mut out = [0u8; 32];
        for (i, v) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    /// One 64-byte block. Dispatches to a hardware-accelerated kernel when
    /// this process has confirmed (once, at first use — see [`hw::verified`])
    /// that this CPU both claims the relevant crypto extension AND produces
    /// byte-identical output to the software path below on a known vector.
    /// Anything else (feature absent, self-check failed, an architecture
    /// with no kernel here) runs the software path, which is the permanent
    /// fallback and the oracle every hardware kernel is checked against.
    fn compress(&mut self, chunk: &[u8; 64]) {
        #[cfg(target_arch = "aarch64")]
        {
            if hw::verified() {
                // SAFETY: `verified()` only returns true after confirming
                // (this process, this call) that `is_aarch64_feature_detected!("sha2")`
                // is true, which is exactly what this kernel requires.
                unsafe { hw::aarch64::compress(&mut self.h, chunk) };
                return;
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            if hw::verified() {
                // SAFETY: `verified()` only returns true after confirming
                // `is_x86_feature_detected!("sha")` is true, which is
                // exactly what this kernel requires (sse2 is x86_64
                // baseline and needs no runtime check).
                unsafe { hw::x86_64::compress(&mut self.h, chunk) };
                return;
            }
        }
        Self::compress_sw(&mut self.h, chunk);
    }

    /// The portable software path (FIPS 180-4 reference schedule +
    /// compression). Never gated on CPU features, so it is always
    /// available as the fallback and as the correctness oracle every
    /// hardware kernel is checked against — see `tests::hardware_matches_software_oracle`.
    fn compress_sw(h: &mut [u32; 8], chunk: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
}

/// Hardware SHA-256 compression kernels, one 64-byte block at a time.
/// Every kernel here takes the SAME `(h: &mut [u32; 8], chunk: &[u8; 64])`
/// shape as [`Sha256::compress_sw`] and MUST produce byte-identical output
/// to it — that equivalence is what `verified()` checks at runtime before
/// any kernel is ever allowed to run on real data, and what
/// `tests::hardware_matches_software_oracle` checks exhaustively in CI.
mod hw {
    /// True once this process has confirmed, for the current CPU, that a
    /// hardware kernel exists here AND agrees byte-for-byte with the
    /// software path on a known vector. Computed once and cached: a
    /// negative result (feature absent, or — belt and braces — a kernel
    /// that somehow does not match the software oracle) permanently
    /// disables hardware use for this process; it never turns the
    /// software fallback into a correctness risk, only a speed one.
    pub fn verified() -> bool {
        static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *OK.get_or_init(check)
    }

    /// SHA-256("abc") padded into its one 64-byte block, and the FIPS
    /// 180-4 published digest for it — the standard short test vector,
    /// independent of anything computed by this crate's own software path.
    const ABC_BLOCK: [u8; 64] = {
        let mut b = [0u8; 64];
        b[0] = b'a';
        b[1] = b'b';
        b[2] = b'c';
        b[3] = 0x80;
        b[63] = 24; // bit length of "abc" = 3 bytes * 8
        b
    };
    const ABC_DIGEST: [u32; 8] = [
        0xba7816bf, 0x8f01cfea, 0x414140de, 0x5dae2223, 0xb00361a3, 0x96177a9c, 0xb410ff61,
        0xf20015ad,
    ];

    #[cfg_attr(not(any(target_arch = "aarch64", target_arch = "x86_64")), allow(dead_code))]
    fn check() -> bool {
        #[cfg(target_arch = "aarch64")]
        {
            if std::arch::is_aarch64_feature_detected!("sha2") {
                let mut h = super::H0;
                // SAFETY: feature presence just confirmed above.
                unsafe { aarch64::compress(&mut h, &ABC_BLOCK) };
                return h == ABC_DIGEST;
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("sha") {
                let mut h = super::H0;
                // SAFETY: feature presence just confirmed above.
                unsafe { x86_64::compress(&mut h, &ABC_BLOCK) };
                return h == ABC_DIGEST;
            }
        }
        false
    }

    /// ARMv8 Crypto Extensions (`sha2`): `vsha256hq_u32`/`vsha256h2q_u32`
    /// each fold two SHA-256 rounds; 16 calls to the pair cover all 64
    /// rounds. The message schedule (`w[64]`) is expanded in plain scalar
    /// Rust — identical to [`super::Sha256::compress_sw`]'s — rather than
    /// with `vsha256su0/su1q_u32`: message expansion is a small fraction of
    /// the per-block cost, and reusing already-verified scalar code here
    /// removes an entire class of transcription risk from the one part of
    /// this kernel most likely to be gotten subtly wrong from memory.
    #[cfg(target_arch = "aarch64")]
    pub mod aarch64 {
        use core::arch::aarch64::*;

        #[target_feature(enable = "sha2")]
        pub unsafe fn compress(h: &mut [u32; 8], chunk: &[u8; 64]) {
            let mut w = [0u32; 64];
            for (i, word) in chunk.chunks_exact(4).enumerate() {
                w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }

            let mut state0 = vld1q_u32(h[0..4].as_ptr()); // a,b,c,d
            let mut state1 = vld1q_u32(h[4..8].as_ptr()); // e,f,g,h
            let abcd_save = state0;
            let efgh_save = state1;

            for group in 0..16usize {
                let base = group * 4;
                let mut wk = [0u32; 4];
                for j in 0..4 {
                    wk[j] = w[base + j].wrapping_add(super::super::K[base + j]);
                }
                let wkv = vld1q_u32(wk.as_ptr());
                // `state1` (efgh) is read here BEFORE state0 is reassigned,
                // so both calls below see the PRE-round values of the
                // other half, exactly as `vsha256h2q_u32` requires.
                let prev0 = state0;
                state0 = vsha256hq_u32(state0, state1, wkv);
                state1 = vsha256h2q_u32(state1, prev0, wkv);
            }

            state0 = vaddq_u32(state0, abcd_save);
            state1 = vaddq_u32(state1, efgh_save);
            vst1q_u32(h[0..4].as_mut_ptr(), state0);
            vst1q_u32(h[4..8].as_mut_ptr(), state1);
        }
    }

    /// Intel/AMD SHA Extensions (`sha`): `_mm_sha256rnds2_epu32` folds two
    /// rounds per call, so 32 calls cover all 64 rounds. The instruction
    /// requires state packed as `ABEF = {a,b,e,f}` / `CDGH = {c,d,g,h}`
    /// (Intel's documented layout for this instruction pair) — built here
    /// with `_mm_unpacklo/hi_epi64`, which needs no shuffle-immediate
    /// bit-twiddling to get right, unlike the classic reference's
    /// `_mm_shuffle_epi32`/`_mm_alignr_epi8`/`_mm_blend_epi16` dance. As
    /// with the aarch64 kernel, the message schedule is the plain scalar
    /// expansion, not `_mm_sha256msg1/2_epu32`.
    #[cfg(target_arch = "x86_64")]
    pub mod x86_64 {
        use core::arch::x86_64::*;

        #[target_feature(enable = "sha")]
        pub unsafe fn compress(h: &mut [u32; 8], chunk: &[u8; 64]) {
            let mut w = [0u32; 64];
            for (i, word) in chunk.chunks_exact(4).enumerate() {
                w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }

            let abcd = _mm_loadu_si128(h[0..4].as_ptr().cast());
            let efgh = _mm_loadu_si128(h[4..8].as_ptr().cast());
            let mut abef = _mm_unpacklo_epi64(abcd, efgh); // {a,b,e,f}
            let mut cdgh = _mm_unpackhi_epi64(abcd, efgh); // {c,d,g,h}
            let abef_save = abef;
            let cdgh_save = cdgh;

            for group in 0..16usize {
                let base = group * 4;
                let mut wk = [0u32; 4];
                for j in 0..4 {
                    wk[j] = w[base + j].wrapping_add(super::super::K[base + j]);
                }
                let wk_lo = _mm_loadu_si128(wk.as_ptr().cast());
                cdgh = _mm_sha256rnds2_epu32(cdgh, abef, wk_lo);
                let wk_hi = _mm_shuffle_epi32(wk_lo, 0x0E); // upper 64 bits -> lower
                abef = _mm_sha256rnds2_epu32(abef, cdgh, wk_hi);
            }

            abef = _mm_add_epi32(abef, abef_save);
            cdgh = _mm_add_epi32(cdgh, cdgh_save);
            let abcd_out = _mm_unpacklo_epi64(abef, cdgh);
            let efgh_out = _mm_unpackhi_epi64(abef, cdgh);
            _mm_storeu_si128(h[0..4].as_mut_ptr().cast(), abcd_out);
            _mm_storeu_si128(h[4..8].as_mut_ptr().cast(), efgh_out);
        }
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256(data);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        // NIST FIPS 180-4 two-block message: 56 bytes forces the length field
        // into a second padded block, the exact boundary finalize() special-
        // cases. External vector, not self-consistency.
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn streaming_matches_oneshot() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        // Uneven split sizes exercise the partial-block path on both sides.
        for split in [1usize, 7, 63, 64, 65, 4096, 99_999] {
            let mut h = Sha256::new();
            for chunk in data.chunks(split) {
                h.update(chunk);
            }
            assert_eq!(h.finalize(), sha256(&data), "split {split}");
        }
    }

    #[test]
    fn fifty_six_byte_boundary_pads_two_blocks() {
        for len in 55..=65usize {
            let data = vec![0x5au8; len];
            let mut h = Sha256::new();
            h.update(&data);
            assert_eq!(h.finalize(), sha256(&data), "len {len}");
        }
    }

    /// Hash `data` end to end using ONLY `kernel` as the block compressor —
    /// the same block-splitting and FIPS 180-4 padding [`Sha256::update`] /
    /// [`Sha256::finalize`] use, just parameterized so this test can force
    /// a SPECIFIC kernel (software, or one architecture's hardware one)
    /// regardless of what `Sha256::compress`'s runtime dispatch would pick.
    fn hash_with(data: &[u8], kernel: fn(&mut [u32; 8], &[u8; 64])) -> [u8; 32] {
        let mut h = H0;
        let mut chunks = data.chunks_exact(64);
        for chunk in &mut chunks {
            let mut block = [0u8; 64];
            block.copy_from_slice(chunk);
            kernel(&mut h, &block);
        }
        let rest = chunks.remainder();
        let bit_len = (data.len() as u64).wrapping_mul(8);
        let mut tail = [0u8; 128];
        tail[..rest.len()].copy_from_slice(rest);
        tail[rest.len()] = 0x80;
        let tail_len = if rest.len() < 56 { 64 } else { 128 };
        tail[tail_len - 8..tail_len].copy_from_slice(&bit_len.to_be_bytes());
        let mut block = [0u8; 64];
        block.copy_from_slice(&tail[..64]);
        kernel(&mut h, &block);
        if tail_len == 128 {
            block.copy_from_slice(&tail[64..]);
            kernel(&mut h, &block);
        }
        let mut out = [0u8; 32];
        for (i, v) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    /// A pseudo-random-looking but fully deterministic byte at index `i` —
    /// no `rand` dependency, just varied enough to exercise real bit
    /// patterns rather than a constant fill.
    fn fuzz_byte(i: usize) -> u8 {
        (i as u32).wrapping_mul(2_654_435_761).wrapping_add(i as u32 >> 3) as u8
    }

    fn check_hardware_kernel_against_software(hw_kernel: fn(&mut [u32; 8], &[u8; 64])) {
        // Every length 0..=4096 covers every partial-block / padding-
        // boundary shape a real stream can produce, one byte at a time —
        // including the 55/56/64/119/120/128-byte edges finalize() special-
        // cases (one padded block vs two).
        for len in 0..=4096usize {
            let data: Vec<u8> = (0..len).map(fuzz_byte).collect();
            assert_eq!(
                hash_with(&data, Sha256::compress_sw),
                hash_with(&data, hw_kernel),
                "len {len}"
            );
        }
        // Multi-MB buffers: many blocks back to back, not just the boundary
        // shapes above.
        for len in [1_000_003usize, 4_194_304, 8_388_611] {
            let data: Vec<u8> = (0..len).map(fuzz_byte).collect();
            assert_eq!(
                hash_with(&data, Sha256::compress_sw),
                hash_with(&data, hw_kernel),
                "len {len}"
            );
        }
    }

    /// The hardware kernel for this architecture (if any) must be
    /// BYTE-IDENTICAL to the software oracle across every block-boundary
    /// shape and well past it — this is what makes shipping the `unsafe`
    /// intrinsic kernels in this file safe: [`hw::verified`] runs a version
    /// of this same check (one fixed vector) before `Sha256::compress` is
    /// ever allowed to use hardware on real data, so a kernel that fails
    /// here would also refuse itself at runtime rather than silently
    /// producing a wrong digest.
    #[test]
    fn hardware_matches_software_oracle() {
        #[cfg(target_arch = "aarch64")]
        {
            if !std::arch::is_aarch64_feature_detected!("sha2") {
                return;
            }
            assert!(
                hw::verified(),
                "aarch64 sha2 detected but the hardware self-check refused it — \
                 compress() will never use hardware on this run"
            );
            check_hardware_kernel_against_software(|h, c| unsafe { hw::aarch64::compress(h, c) });
            return;
        }
        #[cfg(target_arch = "x86_64")]
        {
            if !std::is_x86_feature_detected!("sha") {
                return;
            }
            assert!(
                hw::verified(),
                "x86_64 sha detected but the hardware self-check refused it — \
                 compress() will never use hardware on this run"
            );
            check_hardware_kernel_against_software(|h, c| unsafe { hw::x86_64::compress(h, c) });
            return;
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            // No hardware kernel exists for this architecture; the software
            // path is the only one and there is nothing to compare it to.
        }
    }

    /// Throughput of the kernel `Sha256::compress` actually dispatches to
    /// on THIS machine right now, vs. the software path forced directly —
    /// printed with `--nocapture` for a before/after MB/s reading.
    #[test]
    fn measure_throughput() {
        let data = vec![0x37u8; 64 * 1024 * 1024];
        let rounds = 4;

        let t0 = std::time::Instant::now();
        for _ in 0..rounds {
            let mut h = Sha256::new();
            h.update(&data);
            std::hint::black_box(h.finalize());
        }
        let dispatched = t0.elapsed();

        let t0 = std::time::Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(hash_with(&data, Sha256::compress_sw));
        }
        let software = t0.elapsed();

        let mb = (data.len() * rounds) as f64 / (1024.0 * 1024.0);
        println!(
            "sha256 throughput over {} MiB: dispatched(compress) = {:.1} MB/s · forced-software = {:.1} MB/s · hw_verified = {}",
            data.len() / (1024 * 1024),
            mb / dispatched.as_secs_f64(),
            mb / software.as_secs_f64(),
            hw::verified(),
        );
    }
}
