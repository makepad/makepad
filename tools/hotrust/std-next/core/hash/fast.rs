//! The default hasher: a folded-multiply hash (the scheme of foldhash / ahash's fallback).
//! Fast on integer keys (one 64x64->128 multiply per word) and good enough in its low bits
//! (bucket index) and high bits (control byte) for open addressing.
//!
//! Differences from real std, both unobservable by correct programs: the seed is fixed
//! (RandomState is deterministic, so HashMap iteration order is the same on every run but
//! differs from real std's), and the hash function is not SipHash (no HashDoS resistance).

use super::{BuildHasher, Hasher};

const MULT: u64 = 0x5851_f42d_4c95_7f2d;
const K_BYTES: u64 = 0x243f_6a88_85a3_08d3;
const K_TAIL: u64 = 0x1319_8a2e_0370_7344;
const K_FINISH: u64 = 0xa409_3822_299f_31d0;
const DEFAULT_SEED: u64 = 0x082e_fa98_ec4e_6c89;

#[inline]
fn folded_multiply(x: u64, y: u64) -> u64 {
    let full = (x as u128) * (y as u128);
    (full as u64) ^ ((full >> 64) as u64)
}

#[inline]
fn read_u64(b: &[u8], i: usize) -> u64 {
    (b[i] as u64)
        | ((b[i + 1] as u64) << 8)
        | ((b[i + 2] as u64) << 16)
        | ((b[i + 3] as u64) << 24)
        | ((b[i + 4] as u64) << 32)
        | ((b[i + 5] as u64) << 40)
        | ((b[i + 6] as u64) << 48)
        | ((b[i + 7] as u64) << 56)
}

#[inline]
fn read_u32(b: &[u8], i: usize) -> u64 {
    (b[i] as u64) | ((b[i + 1] as u64) << 8) | ((b[i + 2] as u64) << 16) | ((b[i + 3] as u64) << 24)
}

/// The hasher behind RandomState / DefaultHasher.
#[derive(Clone, Debug)]
pub struct FastHasher {
    acc: u64,
    seed: u64,
}

impl FastHasher {
    pub const fn with_seed(seed: u64) -> FastHasher {
        FastHasher { acc: seed, seed }
    }
}

impl Default for FastHasher {
    fn default() -> FastHasher {
        FastHasher::with_seed(DEFAULT_SEED)
    }
}

/// Mixes a byte string of any length into `acc`.
fn mix_bytes(acc: u64, seed: u64, bytes: &[u8]) -> u64 {
    let len = bytes.len();
    let mut acc = acc ^ (len as u64).wrapping_mul(MULT);
    let mut i = 0;
    while i + 16 <= len {
        let a = read_u64(bytes, i);
        let b = read_u64(bytes, i + 8);
        acc = folded_multiply(a ^ acc, b ^ seed ^ K_BYTES);
        i += 16;
    }
    let rem = len - i;
    if rem > 0 {
        let (a, b) = if rem >= 8 {
            (read_u64(bytes, i), read_u64(bytes, len - 8))
        } else if rem >= 4 {
            (read_u32(bytes, i), read_u32(bytes, len - 4))
        } else {
            let lo = bytes[i] as u64;
            let mid = bytes[i + rem / 2] as u64;
            let hi = bytes[len - 1] as u64;
            (lo | (mid << 8) | (hi << 16), 0)
        };
        acc = folded_multiply(a ^ acc, b ^ seed ^ K_TAIL);
    }
    acc
}

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        folded_multiply(self.acc, self.seed ^ K_FINISH)
    }
    fn write(&mut self, bytes: &[u8]) {
        self.acc = mix_bytes(self.acc, self.seed, bytes);
    }
    fn write_u8(&mut self, i: u8) {
        self.write_u64(i as u64)
    }
    fn write_u16(&mut self, i: u16) {
        self.write_u64(i as u64)
    }
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64)
    }
    fn write_u64(&mut self, i: u64) {
        self.acc = folded_multiply(self.acc ^ i, MULT ^ self.seed);
    }
    fn write_u128(&mut self, i: u128) {
        self.write_u64(i as u64);
        self.write_u64((i >> 64) as u64);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64)
    }
    #[cfg(not(hotrust_check))]
    fn write_str(&mut self, s: &str) {
        // the length is mixed in by mix_bytes; no 0xff terminator needed
        self.acc = mix_bytes(self.acc, self.seed, s.as_bytes());
    }
}

/// `std::hash::RandomState`: builds FastHashers. Deterministic (fixed seed), see above.
#[derive(Clone, Debug)]
pub struct RandomState {
    seed: u64,
}

impl RandomState {
    pub fn new() -> RandomState {
        RandomState { seed: DEFAULT_SEED }
    }
}

impl Default for RandomState {
    fn default() -> RandomState {
        RandomState::new()
    }
}

impl BuildHasher for RandomState {
    type Hasher = DefaultHasher;
    fn build_hasher(&self) -> DefaultHasher {
        DefaultHasher { inner: FastHasher::with_seed(self.seed) }
    }
}

/// `std::hash::DefaultHasher` (also at std::collections::hash_map::DefaultHasher).
#[derive(Clone, Debug)]
pub struct DefaultHasher {
    inner: FastHasher,
}

impl DefaultHasher {
    pub fn new() -> DefaultHasher {
        DefaultHasher { inner: FastHasher::with_seed(DEFAULT_SEED) }
    }
}

impl Default for DefaultHasher {
    fn default() -> DefaultHasher {
        DefaultHasher::new()
    }
}

impl Hasher for DefaultHasher {
    fn finish(&self) -> u64 {
        self.inner.finish()
    }
    fn write(&mut self, bytes: &[u8]) {
        self.inner.write(bytes)
    }
    fn write_u8(&mut self, i: u8) {
        self.inner.write_u64(i as u64)
    }
    fn write_u16(&mut self, i: u16) {
        self.inner.write_u64(i as u64)
    }
    fn write_u32(&mut self, i: u32) {
        self.inner.write_u64(i as u64)
    }
    fn write_u64(&mut self, i: u64) {
        self.inner.write_u64(i)
    }
    fn write_u128(&mut self, i: u128) {
        self.inner.write_u128(i)
    }
    fn write_usize(&mut self, i: usize) {
        self.inner.write_u64(i as u64)
    }
    #[cfg(not(hotrust_check))]
    fn write_str(&mut self, s: &str) {
        self.inner.write_str(s)
    }
}
