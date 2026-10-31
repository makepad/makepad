//! A fast, deterministic hasher for the geometry kernels' own maps (cell
//! coordinates, vertex and edge ids): one multiply-rotate per word, where
//! the standard library's DoS-resistant SipHash costs several times more
//! per lookup. Keys here come from the kernels, not from untrusted input.

use std::hash::{BuildHasherDefault, Hasher};

#[derive(Default, Clone, Copy)]
pub struct FastHasher(u64);

const K: u64 = 0x9e37_79b9_7f4a_7c15;

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(w));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.write_u64(i as u64);
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64);
    }
    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.write_u64(i as u64);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(K);
    }
    #[inline]
    fn finish(&self) -> u64 {
        // Spread the high bits down: hashbrown takes buckets from the low bits.
        self.0 ^ (self.0 >> 29)
    }
}

pub type FastBuild = BuildHasherDefault<FastHasher>;
pub type FastMap<K, V> = std::collections::HashMap<K, V, FastBuild>;
pub type FastSet<K> = std::collections::HashSet<K, FastBuild>;
