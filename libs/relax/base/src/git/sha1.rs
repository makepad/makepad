// SHA-1 implementation based on RustCrypto/hashes (MIT/Apache-2.0)
// Stripped of the `digest` framework for zero-dependency use.
// Includes hardware-accelerated paths for aarch64 (SHA1 instructions)
// and x86/x86_64 (SHA-NI).

const BLOCK_SIZE: usize = 64;

pub struct Sha1 {
    state: [u32; 5],
    buffer: [u8; BLOCK_SIZE],
    buffer_len: usize,
    total_len: u64,
}

impl Sha1 {
    pub fn new() -> Self {
        Sha1 {
            state: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buffer: [0u8; BLOCK_SIZE],
            buffer_len: 0,
            total_len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u64;
        let mut offset = 0;

        // Fill buffer if partially full
        if self.buffer_len > 0 {
            let space = BLOCK_SIZE - self.buffer_len;
            let n = data.len().min(space);
            self.buffer[self.buffer_len..self.buffer_len + n].copy_from_slice(&data[..n]);
            self.buffer_len += n;
            offset += n;

            if self.buffer_len == BLOCK_SIZE {
                let block = self.buffer;
                compress(&mut self.state, &[block]);
                self.buffer_len = 0;
            }
        }

        // Process full blocks directly
        while offset + BLOCK_SIZE <= data.len() {
            let mut block = [0u8; BLOCK_SIZE];
            block.copy_from_slice(&data[offset..offset + BLOCK_SIZE]);
            compress(&mut self.state, &[block]);
            offset += BLOCK_SIZE;
        }

        // Buffer remaining
        let remaining = data.len() - offset;
        if remaining > 0 {
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    pub fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.total_len * 8;

        // Padding: append 0x80, then zeros, then 8-byte big-endian bit length
        let mut pad = [0u8; BLOCK_SIZE];
        pad[0] = 0x80;

        let pad_len = if self.buffer_len < 56 {
            56 - self.buffer_len
        } else {
            120 - self.buffer_len
        };
        self.update(&pad[..pad_len]);
        self.update(&bit_len.to_be_bytes());

        let mut out = [0u8; 20];
        for (chunk, &v) in out.chunks_exact_mut(4).zip(self.state.iter()) {
            chunk.copy_from_slice(&v.to_be_bytes());
        }
        out
    }
}

// --- Platform dispatch ---

fn compress(state: &mut [u32; 5], blocks: &[[u8; BLOCK_SIZE]]) {
    #[cfg(target_arch = "aarch64")]
    {
        // Apple Silicon always has SHA1 hardware instructions
        unsafe { compress_aarch64(state, blocks) }
        return;
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if is_x86_feature_detected!("sha") {
            unsafe { compress_x86(state, blocks) }
            return;
        }
    }

    #[allow(unreachable_code)]
    compress_soft(state, blocks);
}

// --- aarch64 hardware SHA1 (using NEON SHA1 instructions) ---

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "sha2")]
unsafe fn compress_aarch64(state: &mut [u32; 5], blocks: &[[u8; BLOCK_SIZE]]) {
    use std::arch::aarch64::*;

    let mut abcd = vld1q_u32(state.as_ptr());
    let mut e0: u32 = state[4];

    for block in blocks {
        let abcd_saved = abcd;
        let e0_saved = e0;

        // Load block as 4 x uint32x4_t, byte-swapping from big-endian
        let mut msg0 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(block.as_ptr())));
        let mut msg1 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(block.as_ptr().add(16))));
        let mut msg2 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(block.as_ptr().add(32))));
        let mut msg3 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(block.as_ptr().add(48))));

        let mut tmp0: uint32x4_t;
        let mut tmp1: uint32x4_t;

        // Rounds 0-3
        tmp0 = vaddq_u32(msg0, vdupq_n_u32(0x5A827999));
        let mut e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1cq_u32(abcd, e0, tmp0);
        msg0 = vsha1su0q_u32(msg0, msg1, msg2);

        // Rounds 4-7
        tmp1 = vaddq_u32(msg1, vdupq_n_u32(0x5A827999));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1cq_u32(abcd, e1, tmp1);
        msg0 = vsha1su1q_u32(msg0, msg3);
        msg1 = vsha1su0q_u32(msg1, msg2, msg3);

        // Rounds 8-11
        tmp0 = vaddq_u32(msg2, vdupq_n_u32(0x5A827999));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1cq_u32(abcd, e0, tmp0);
        msg1 = vsha1su1q_u32(msg1, msg0);
        msg2 = vsha1su0q_u32(msg2, msg3, msg0);

        // Rounds 12-15
        tmp1 = vaddq_u32(msg3, vdupq_n_u32(0x5A827999));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1cq_u32(abcd, e1, tmp1);
        msg2 = vsha1su1q_u32(msg2, msg1);
        msg3 = vsha1su0q_u32(msg3, msg0, msg1);

        // Rounds 16-19
        tmp0 = vaddq_u32(msg0, vdupq_n_u32(0x5A827999));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1cq_u32(abcd, e0, tmp0);
        msg3 = vsha1su1q_u32(msg3, msg2);
        msg0 = vsha1su0q_u32(msg0, msg1, msg2);

        // Rounds 20-23
        tmp1 = vaddq_u32(msg1, vdupq_n_u32(0x6ED9EBA1));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e1, tmp1);
        msg0 = vsha1su1q_u32(msg0, msg3);
        msg1 = vsha1su0q_u32(msg1, msg2, msg3);

        // Rounds 24-27
        tmp0 = vaddq_u32(msg2, vdupq_n_u32(0x6ED9EBA1));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e0, tmp0);
        msg1 = vsha1su1q_u32(msg1, msg0);
        msg2 = vsha1su0q_u32(msg2, msg3, msg0);

        // Rounds 28-31
        tmp1 = vaddq_u32(msg3, vdupq_n_u32(0x6ED9EBA1));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e1, tmp1);
        msg2 = vsha1su1q_u32(msg2, msg1);
        msg3 = vsha1su0q_u32(msg3, msg0, msg1);

        // Rounds 32-35
        tmp0 = vaddq_u32(msg0, vdupq_n_u32(0x6ED9EBA1));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e0, tmp0);
        msg3 = vsha1su1q_u32(msg3, msg2);
        msg0 = vsha1su0q_u32(msg0, msg1, msg2);

        // Rounds 36-39
        tmp1 = vaddq_u32(msg1, vdupq_n_u32(0x6ED9EBA1));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e1, tmp1);
        msg0 = vsha1su1q_u32(msg0, msg3);
        msg1 = vsha1su0q_u32(msg1, msg2, msg3);

        // Rounds 40-43
        tmp0 = vaddq_u32(msg2, vdupq_n_u32(0x8F1BBCDC));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1mq_u32(abcd, e0, tmp0);
        msg1 = vsha1su1q_u32(msg1, msg0);
        msg2 = vsha1su0q_u32(msg2, msg3, msg0);

        // Rounds 44-47
        tmp1 = vaddq_u32(msg3, vdupq_n_u32(0x8F1BBCDC));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1mq_u32(abcd, e1, tmp1);
        msg2 = vsha1su1q_u32(msg2, msg1);
        msg3 = vsha1su0q_u32(msg3, msg0, msg1);

        // Rounds 48-51
        tmp0 = vaddq_u32(msg0, vdupq_n_u32(0x8F1BBCDC));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1mq_u32(abcd, e0, tmp0);
        msg3 = vsha1su1q_u32(msg3, msg2);
        msg0 = vsha1su0q_u32(msg0, msg1, msg2);

        // Rounds 52-55
        tmp1 = vaddq_u32(msg1, vdupq_n_u32(0x8F1BBCDC));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1mq_u32(abcd, e1, tmp1);
        msg0 = vsha1su1q_u32(msg0, msg3);
        msg1 = vsha1su0q_u32(msg1, msg2, msg3);

        // Rounds 56-59
        tmp0 = vaddq_u32(msg2, vdupq_n_u32(0x8F1BBCDC));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1mq_u32(abcd, e0, tmp0);
        msg1 = vsha1su1q_u32(msg1, msg0);
        msg2 = vsha1su0q_u32(msg2, msg3, msg0);

        // Rounds 60-63
        tmp1 = vaddq_u32(msg3, vdupq_n_u32(0xCA62C1D6));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e1, tmp1);
        msg2 = vsha1su1q_u32(msg2, msg1);
        msg3 = vsha1su0q_u32(msg3, msg0, msg1);

        // Rounds 64-67
        tmp0 = vaddq_u32(msg0, vdupq_n_u32(0xCA62C1D6));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e0, tmp0);
        msg3 = vsha1su1q_u32(msg3, msg2);

        // Rounds 68-71
        tmp1 = vaddq_u32(msg1, vdupq_n_u32(0xCA62C1D6));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e1, tmp1);

        // Rounds 72-75
        tmp0 = vaddq_u32(msg2, vdupq_n_u32(0xCA62C1D6));
        e1 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e0, tmp0);

        // Rounds 76-79
        tmp1 = vaddq_u32(msg3, vdupq_n_u32(0xCA62C1D6));
        e0 = vsha1h_u32(vgetq_lane_u32(abcd, 0));
        abcd = vsha1pq_u32(abcd, e1, tmp1);

        e0 = e0.wrapping_add(e0_saved);
        abcd = vaddq_u32(abcd, abcd_saved);
    }

    vst1q_u32(state.as_mut_ptr(), abcd);
    state[4] = e0;
}

// --- x86/x86_64 hardware SHA-NI ---

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "sha,sse2,ssse3,sse4.1")]
unsafe fn compress_x86(state: &mut [u32; 5], blocks: &[[u8; BLOCK_SIZE]]) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
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
        h1 = _mm_sha1rnds4_epu32(h0, h1, 0);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w1), 0);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w2), 0);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w3), 0);
        w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w0, w1), w2), w3);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w4), 0);

        // Rounds 20..40
        w0 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w1, w2), w3), w4);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w0), 1);
        w1 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w2, w3), w4), w0);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w1), 1);
        w2 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w3, w4), w0), w1);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w2), 1);
        w3 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w4, w0), w1), w2);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w3), 1);
        w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w0, w1), w2), w3);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w4), 1);

        // Rounds 40..60
        w0 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w1, w2), w3), w4);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w0), 2);
        w1 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w2, w3), w4), w0);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w1), 2);
        w2 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w3, w4), w0), w1);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w2), 2);
        w3 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w4, w0), w1), w2);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w3), 2);
        w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w0, w1), w2), w3);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w4), 2);

        // Rounds 60..80
        w0 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w1, w2), w3), w4);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w0), 3);
        w1 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w2, w3), w4), w0);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w1), 3);
        w2 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w3, w4), w0), w1);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w2), 3);
        w3 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w4, w0), w1), w2);
        h1 = _mm_sha1rnds4_epu32(h0, _mm_sha1nexte_epu32(h1, w3), 3);
        w4 = _mm_sha1msg2_epu32(_mm_xor_si128(_mm_sha1msg1_epu32(w0, w1), w2), w3);
        h0 = _mm_sha1rnds4_epu32(h1, _mm_sha1nexte_epu32(h0, w4), 3);

        state_abcd = _mm_add_epi32(state_abcd, h0);
        state_e = _mm_sha1nexte_epu32(h1, state_e);
    }

    state[0] = _mm_extract_epi32(state_abcd, 3) as u32;
    state[1] = _mm_extract_epi32(state_abcd, 2) as u32;
    state[2] = _mm_extract_epi32(state_abcd, 1) as u32;
    state[3] = _mm_extract_epi32(state_abcd, 0) as u32;
    state[4] = _mm_extract_epi32(state_e, 3) as u32;
}

// --- Software fallback (from RustCrypto sha1 soft.rs) ---

const K: [u32; 4] = [0x5A827999, 0x6ED9EBA1, 0x8F1BBCDC, 0xCA62C1D6];

#[inline(always)]
fn add(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    [
        a[0].wrapping_add(b[0]),
        a[1].wrapping_add(b[1]),
        a[2].wrapping_add(b[2]),
        a[3].wrapping_add(b[3]),
    ]
}

#[inline(always)]
fn xor(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    [a[0] ^ b[0], a[1] ^ b[1], a[2] ^ b[2], a[3] ^ b[3]]
}

#[inline]
fn sha1_first_add(e: u32, w0: [u32; 4]) -> [u32; 4] {
    let [a, b, c, d] = w0;
    [e.wrapping_add(a), b, c, d]
}

fn sha1msg1(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let [_, _, w2, w3] = a;
    let [w4, w5, _, _] = b;
    [a[0] ^ w2, a[1] ^ w3, a[2] ^ w4, a[3] ^ w5]
}

fn sha1msg2(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let [x0, x1, x2, x3] = a;
    let [_, w13, w14, w15] = b;

    let w16 = (x0 ^ w13).rotate_left(1);
    let w17 = (x1 ^ w14).rotate_left(1);
    let w18 = (x2 ^ w15).rotate_left(1);
    let w19 = (x3 ^ w16).rotate_left(1);

    [w16, w17, w18, w19]
}

#[inline]
fn sha1_first_half(abcd: [u32; 4], msg: [u32; 4]) -> [u32; 4] {
    sha1_first_add(abcd[0].rotate_left(30), msg)
}

fn sha1_digest_round_x4(abcd: [u32; 4], work: [u32; 4], i: i8) -> [u32; 4] {
    match i {
        0 => sha1rnds4c(abcd, add(work, [K[0]; 4])),
        1 => sha1rnds4p(abcd, add(work, [K[1]; 4])),
        2 => sha1rnds4m(abcd, add(work, [K[2]; 4])),
        3 => sha1rnds4p(abcd, add(work, [K[3]; 4])),
        _ => unreachable!(),
    }
}

fn sha1rnds4c(abcd: [u32; 4], msg: [u32; 4]) -> [u32; 4] {
    let [mut a, mut b, mut c, mut d] = abcd;
    let [t, u, v, w] = msg;
    let mut e = 0u32;
    macro_rules! f {
        ($a:expr,$b:expr,$c:expr) => {
            $c ^ ($a & ($b ^ $c))
        };
    }
    e = e
        .wrapping_add(a.rotate_left(5))
        .wrapping_add(f!(b, c, d))
        .wrapping_add(t);
    b = b.rotate_left(30);
    d = d
        .wrapping_add(e.rotate_left(5))
        .wrapping_add(f!(a, b, c))
        .wrapping_add(u);
    a = a.rotate_left(30);
    c = c
        .wrapping_add(d.rotate_left(5))
        .wrapping_add(f!(e, a, b))
        .wrapping_add(v);
    e = e.rotate_left(30);
    b = b
        .wrapping_add(c.rotate_left(5))
        .wrapping_add(f!(d, e, a))
        .wrapping_add(w);
    d = d.rotate_left(30);
    [b, c, d, e]
}

fn sha1rnds4p(abcd: [u32; 4], msg: [u32; 4]) -> [u32; 4] {
    let [mut a, mut b, mut c, mut d] = abcd;
    let [t, u, v, w] = msg;
    let mut e = 0u32;
    macro_rules! f {
        ($a:expr,$b:expr,$c:expr) => {
            $a ^ $b ^ $c
        };
    }
    e = e
        .wrapping_add(a.rotate_left(5))
        .wrapping_add(f!(b, c, d))
        .wrapping_add(t);
    b = b.rotate_left(30);
    d = d
        .wrapping_add(e.rotate_left(5))
        .wrapping_add(f!(a, b, c))
        .wrapping_add(u);
    a = a.rotate_left(30);
    c = c
        .wrapping_add(d.rotate_left(5))
        .wrapping_add(f!(e, a, b))
        .wrapping_add(v);
    e = e.rotate_left(30);
    b = b
        .wrapping_add(c.rotate_left(5))
        .wrapping_add(f!(d, e, a))
        .wrapping_add(w);
    d = d.rotate_left(30);
    [b, c, d, e]
}

fn sha1rnds4m(abcd: [u32; 4], msg: [u32; 4]) -> [u32; 4] {
    let [mut a, mut b, mut c, mut d] = abcd;
    let [t, u, v, w] = msg;
    let mut e = 0u32;
    macro_rules! f {
        ($a:expr,$b:expr,$c:expr) => {
            ($a & $b) ^ ($a & $c) ^ ($b & $c)
        };
    }
    e = e
        .wrapping_add(a.rotate_left(5))
        .wrapping_add(f!(b, c, d))
        .wrapping_add(t);
    b = b.rotate_left(30);
    d = d
        .wrapping_add(e.rotate_left(5))
        .wrapping_add(f!(a, b, c))
        .wrapping_add(u);
    a = a.rotate_left(30);
    c = c
        .wrapping_add(d.rotate_left(5))
        .wrapping_add(f!(e, a, b))
        .wrapping_add(v);
    e = e.rotate_left(30);
    b = b
        .wrapping_add(c.rotate_left(5))
        .wrapping_add(f!(d, e, a))
        .wrapping_add(w);
    d = d.rotate_left(30);
    [b, c, d, e]
}

macro_rules! rounds4 {
    ($h0:ident, $h1:ident, $wk:expr, $i:expr) => {
        sha1_digest_round_x4($h0, sha1_first_half($h1, $wk), $i)
    };
}

macro_rules! schedule {
    ($v0:expr, $v1:expr, $v2:expr, $v3:expr) => {
        sha1msg2(xor(sha1msg1($v0, $v1), $v2), $v3)
    };
}

macro_rules! schedule_rounds4 {
    ($h0:ident, $h1:ident, $w0:expr, $w1:expr, $w2:expr, $w3:expr, $w4:expr, $i:expr) => {
        $w4 = schedule!($w0, $w1, $w2, $w3);
        $h1 = rounds4!($h0, $h1, $w4, $i);
    };
}

#[inline(always)]
fn sha1_digest_block_u32(state: &mut [u32; 5], block: &[u32; 16]) {
    let mut w0 = [block[0], block[1], block[2], block[3]];
    let mut w1 = [block[4], block[5], block[6], block[7]];
    let mut w2 = [block[8], block[9], block[10], block[11]];
    let mut w3 = [block[12], block[13], block[14], block[15]];
    #[allow(clippy::needless_late_init)]
    let mut w4;

    let mut h0 = [state[0], state[1], state[2], state[3]];
    let mut h1 = sha1_first_add(state[4], w0);

    h1 = sha1_digest_round_x4(h0, h1, 0);
    h0 = rounds4!(h1, h0, w1, 0);
    h1 = rounds4!(h0, h1, w2, 0);
    h0 = rounds4!(h1, h0, w3, 0);
    schedule_rounds4!(h0, h1, w0, w1, w2, w3, w4, 0);

    schedule_rounds4!(h1, h0, w1, w2, w3, w4, w0, 1);
    schedule_rounds4!(h0, h1, w2, w3, w4, w0, w1, 1);
    schedule_rounds4!(h1, h0, w3, w4, w0, w1, w2, 1);
    schedule_rounds4!(h0, h1, w4, w0, w1, w2, w3, 1);
    schedule_rounds4!(h1, h0, w0, w1, w2, w3, w4, 1);

    schedule_rounds4!(h0, h1, w1, w2, w3, w4, w0, 2);
    schedule_rounds4!(h1, h0, w2, w3, w4, w0, w1, 2);
    schedule_rounds4!(h0, h1, w3, w4, w0, w1, w2, 2);
    schedule_rounds4!(h1, h0, w4, w0, w1, w2, w3, 2);
    schedule_rounds4!(h0, h1, w0, w1, w2, w3, w4, 2);

    schedule_rounds4!(h1, h0, w1, w2, w3, w4, w0, 3);
    schedule_rounds4!(h0, h1, w2, w3, w4, w0, w1, 3);
    schedule_rounds4!(h1, h0, w3, w4, w0, w1, w2, 3);
    schedule_rounds4!(h0, h1, w4, w0, w1, w2, w3, 3);
    schedule_rounds4!(h1, h0, w0, w1, w2, w3, w4, 3);

    let e = h1[0].rotate_left(30);
    let [a, b, c, d] = h0;

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

#[allow(dead_code)]
fn compress_soft(state: &mut [u32; 5], blocks: &[[u8; BLOCK_SIZE]]) {
    let mut block_u32 = [0u32; BLOCK_SIZE / 4];
    let mut state_cpy = *state;
    for block in blocks.iter() {
        for (o, chunk) in block_u32.iter_mut().zip(block.chunks_exact(4)) {
            *o = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        sha1_digest_block_u32(&mut state_cpy, &block_u32);
    }
    *state = state_cpy;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let hash = Sha1::new().finalize();
        assert_eq!(hex(&hash), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn test_hello_world() {
        let mut h = Sha1::new();
        h.update(b"hello world");
        assert_eq!(
            hex(&h.finalize()),
            "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
        );
    }

    #[test]
    fn test_git_blob() {
        let data = b"hello world\n";
        let header = format!("blob {}\0", data.len());
        let mut h = Sha1::new();
        h.update(header.as_bytes());
        h.update(data);
        assert_eq!(
            hex(&h.finalize()),
            "3b18e512dba79e4c8300dd08aeb37f8e728b8dad"
        );
    }

    #[test]
    fn test_incremental() {
        let mut h = Sha1::new();
        h.update(b"hello");
        h.update(b" ");
        h.update(b"world");
        assert_eq!(
            hex(&h.finalize()),
            "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
        );
    }

    #[test]
    fn test_large() {
        let mut h = Sha1::new();
        for _ in 0..10 {
            h.update(&[b'a'; 100]);
        }
        assert_eq!(
            hex(&h.finalize()),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
    }

    fn hex(bytes: &[u8; 20]) -> String {
        let mut s = String::with_capacity(40);
        for b in bytes {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }
}
