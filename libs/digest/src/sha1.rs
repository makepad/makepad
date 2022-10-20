// sha1 digest impl from Rust crypto minus all the wrappers

pub struct Sha1 {
    state: [u32; STATE_LEN],
    block: [u8; U8_BLOCK_LEN],
    total: usize,
    in_block: usize
}

impl Sha1 {
    pub fn new() -> Sha1 {
        Self {
            state: SHA1_INIT_STATE.clone(),
            block: [0u8; U8_BLOCK_LEN],
            in_block: 0,
            total: 0
        }
    }
    
    pub fn update(&mut self, bytes: &[u8]) {
        // first write bytes into block,
        for i in 0..bytes.len() {
            self.block[self.in_block] = bytes[i];
            self.in_block += 1;
            if self.in_block == U8_BLOCK_LEN {
                sha1_digest_bytes(&mut self.state, &self.block);
                self.block = [0u8; U8_BLOCK_LEN];
                self.in_block = 0;
                self.total += U8_BLOCK_LEN;
            }
        }
    }
    
    pub fn finalise(mut self) -> [u8; U8_STATE_LEN] {
        
        let bits = (self.total as u64 + (self.in_block as u64)) * 8;
        let extra = bits.to_be_bytes();
        let mut last_one = [0u8; U8_BLOCK_LEN];
        let mut last_two = [0u8; U8_BLOCK_LEN];
        last_one[..self.in_block].clone_from_slice(&self.block[..self.in_block]);
        last_one[self.in_block] = 0x80;
        if self.in_block < 56 {
            last_one[56..64].clone_from_slice(&extra);
            sha1_digest_bytes(&mut self.state, &last_one);
        } else {
            last_two[56..64].clone_from_slice(&extra);
            sha1_digest_bytes(&mut self.state, &last_one);
            sha1_digest_bytes(&mut self.state, &last_two);
        }
        
        sha1_state_to_bytes(&self.state)
    }
}

pub const STATE_LEN: usize = 5;
pub const BLOCK_LEN: usize = 16;
pub const U8_BLOCK_LEN: usize = BLOCK_LEN * 4;
pub const U8_STATE_LEN: usize = STATE_LEN * 4;
pub const K0: u32 = 0x5A827999u32;
pub const K1: u32 = 0x6ED9EBA1u32;
pub const K2: u32 = 0x8F1BBCDCu32;
pub const K3: u32 = 0xCA62C1D6u32;
pub const SHA1_INIT_STATE: [u32; STATE_LEN] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

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
pub fn sha1_first_add(e: u32, w0: [u32; 4]) -> [u32; 4] {
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
    const K0V: [u32; 4] = [K0, K0, K0, K0];
    const K1V: [u32; 4] = [K1, K1, K1, K1];
    const K2V: [u32; 4] = [K2, K2, K2, K2];
    const K3V: [u32; 4] = [K3, K3, K3, K3];
    
    match i {
        0 => sha1rnds4c(abcd, add(work, K0V)),
        1 => sha1rnds4p(abcd, add(work, K1V)),
        2 => sha1rnds4m(abcd, add(work, K2V)),
        3 => sha1rnds4p(abcd, add(work, K3V)),
        _ => unreachable!("unknown icosaround index"),
    }
}

fn sha1rnds4c(abcd: [u32; 4], msg: [u32; 4]) -> [u32; 4] {
    let [mut a, mut b, mut c, mut d] = abcd;
    let [t, u, v, w] = msg;
    let mut e = 0u32;
    
    macro_rules!bool3ary_202 {
        ( $ a: expr, $ b: expr, $ c: expr) => {
            $ c ^ ( $ a & ( $ b ^ $ c))
        };
    } // Choose, MD5F, SHA1C
    
    e = e
        .wrapping_add(a.rotate_left(5))
        .wrapping_add(bool3ary_202!(b, c, d))
        .wrapping_add(t);
    b = b.rotate_left(30);
    
    d = d
        .wrapping_add(e.rotate_left(5))
        .wrapping_add(bool3ary_202!(a, b, c))
        .wrapping_add(u);
    a = a.rotate_left(30);
    
    c = c
        .wrapping_add(d.rotate_left(5))
        .wrapping_add(bool3ary_202!(e, a, b))
        .wrapping_add(v);
    e = e.rotate_left(30);
    
    b = b
        .wrapping_add(c.rotate_left(5))
        .wrapping_add(bool3ary_202!(d, e, a))
        .wrapping_add(w);
    d = d.rotate_left(30);
    
    [b, c, d, e]
}

fn sha1rnds4p(abcd: [u32; 4], msg: [u32; 4]) -> [u32; 4] {
    let [mut a, mut b, mut c, mut d] = abcd;
    let [t, u, v, w] = msg;
    let mut e = 0u32;
    
    macro_rules!bool3ary_150 {
        ( $ a: expr, $ b: expr, $ c: expr) => {
            $ a ^ $ b ^ $ c
        };
    } // Parity, XOR, MD5H, SHA1P
    
    e = e
        .wrapping_add(a.rotate_left(5))
        .wrapping_add(bool3ary_150!(b, c, d))
        .wrapping_add(t);
    b = b.rotate_left(30);
    
    d = d
        .wrapping_add(e.rotate_left(5))
        .wrapping_add(bool3ary_150!(a, b, c))
        .wrapping_add(u);
    a = a.rotate_left(30);
    
    c = c
        .wrapping_add(d.rotate_left(5))
        .wrapping_add(bool3ary_150!(e, a, b))
        .wrapping_add(v);
    e = e.rotate_left(30);
    
    b = b
        .wrapping_add(c.rotate_left(5))
        .wrapping_add(bool3ary_150!(d, e, a))
        .wrapping_add(w);
    d = d.rotate_left(30);
    
    [b, c, d, e]
}

fn sha1rnds4m(abcd: [u32; 4], msg: [u32; 4]) -> [u32; 4] {
    let [mut a, mut b, mut c, mut d] = abcd;
    let [t, u, v, w] = msg;
    let mut e = 0u32;
    
    macro_rules!bool3ary_232 {
        ( $ a: expr, $ b: expr, $ c: expr) => {
            ( $ a & $ b) ^ ( $ a & $ c) ^ ( $ b & $ c)
        };
    } // Majority, SHA1M
    
    e = e
        .wrapping_add(a.rotate_left(5))
        .wrapping_add(bool3ary_232!(b, c, d))
        .wrapping_add(t);
    b = b.rotate_left(30);
    
    d = d
        .wrapping_add(e.rotate_left(5))
        .wrapping_add(bool3ary_232!(a, b, c))
        .wrapping_add(u);
    a = a.rotate_left(30);
    
    c = c
        .wrapping_add(d.rotate_left(5))
        .wrapping_add(bool3ary_232!(e, a, b))
        .wrapping_add(v);
    e = e.rotate_left(30);
    
    b = b
        .wrapping_add(c.rotate_left(5))
        .wrapping_add(bool3ary_232!(d, e, a))
        .wrapping_add(w);
    d = d.rotate_left(30);
    
    [b, c, d, e]
}

macro_rules!rounds4 {
    ( $ h0: ident, $ h1: ident, $ wk: expr, $ i: expr) => {
        sha1_digest_round_x4( $ h0, sha1_first_half( $ h1, $ wk), $ i)
    };
}

macro_rules!schedule {
    ( $ v0: expr, $ v1: expr, $ v2: expr, $ v3: expr) => {
        sha1msg2(xor(sha1msg1( $ v0, $ v1), $ v2), $ v3)
    };
}

macro_rules!schedule_rounds4 {
    (
        $ h0: ident,
        $ h1: ident,
        $ w0: expr,
        $ w1: expr,
        $ w2: expr,
        $ w3: expr,
        $ w4: expr,
        $ i: expr
    ) => {
        $ w4 = schedule!( $ w0, $ w1, $ w2, $ w3);
        $ h1 = rounds4!( $ h0, $ h1, $ w4, $ i);
    };
}

#[inline(always)]
fn sha1_digest_block_u32(state: &mut [u32; 5], block: &[u32; 16]) {
    let mut w0 = [block[0], block[1], block[2], block[3]];
    let mut w1 = [block[4], block[5], block[6], block[7]];
    let mut w2 = [block[8], block[9], block[10], block[11]];
    let mut w3 = [block[12], block[13], block[14], block[15]];
    let mut w4;
    
    let mut h0 = [state[0], state[1], state[2], state[3]];
    let mut h1 = sha1_first_add(state[4], w0);
    
    // Rounds 0..20
    h1 = sha1_digest_round_x4(h0, h1, 0);
    h0 = rounds4!(h1, h0, w1, 0);
    h1 = rounds4!(h0, h1, w2, 0);
    h0 = rounds4!(h1, h0, w3, 0);
    schedule_rounds4!(h0, h1, w0, w1, w2, w3, w4, 0);
    
    // Rounds 20..40
    schedule_rounds4!(h1, h0, w1, w2, w3, w4, w0, 1);
    schedule_rounds4!(h0, h1, w2, w3, w4, w0, w1, 1);
    schedule_rounds4!(h1, h0, w3, w4, w0, w1, w2, 1);
    schedule_rounds4!(h0, h1, w4, w0, w1, w2, w3, 1);
    schedule_rounds4!(h1, h0, w0, w1, w2, w3, w4, 1);
    
    // Rounds 40..60
    schedule_rounds4!(h0, h1, w1, w2, w3, w4, w0, 2);
    schedule_rounds4!(h1, h0, w2, w3, w4, w0, w1, 2);
    schedule_rounds4!(h0, h1, w3, w4, w0, w1, w2, 2);
    schedule_rounds4!(h1, h0, w4, w0, w1, w2, w3, 2);
    schedule_rounds4!(h0, h1, w0, w1, w2, w3, w4, 2);
    
    // Rounds 60..80
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

pub fn sha1_digest_bytes(state: &mut[u32; STATE_LEN], bytes: &[u8; U8_BLOCK_LEN]) {
    let mut block_u32 = [0u32; 16];
    for i in 0..16 {
        let off = i * 4;
        block_u32[i] = (bytes[off + 3] as u32)
            | ((bytes[off + 2] as u32) << 8)
            | ((bytes[off + 1] as u32) << 16)
            | ((bytes[off] as u32) << 24);
    }
    sha1_digest_block_u32(state, &block_u32);
}

pub fn sha1_state_to_bytes(state: &[u32; STATE_LEN]) -> [u8; U8_STATE_LEN] {
    let mut state_bytes = [0u8; STATE_LEN * 4];
    for i in 0..STATE_LEN {
        let bytes = state[i].to_be_bytes();
        for j in 0..4 {
            state_bytes[i * 4 + j] = bytes[j];
        }
    };
    state_bytes
}

