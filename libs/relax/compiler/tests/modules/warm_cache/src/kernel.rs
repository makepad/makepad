// A mixing loop, a word count and a sort: std's small helpers (rotates, wrapping adds, range
// steps), a hash map and a sort (main.rs runs this as host code and as a sandboxed module).
// The map's hasher has fixed keys: `HashMap::new()`'s RandomState reads thread-local keys.
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;

pub fn mix(n: u32) -> u64 {
    let mut w = [0u32; 16];
    let mut h = 0x6a09e667u32;
    let mut out = 0u64;
    for i in 0..n {
        for k in 0..16 {
            w[k] = w[k].rotate_right(7) ^ i.wrapping_add(k as u32).wrapping_mul(0x9e3779b9);
        }
        for k in 0..16 {
            h = h.rotate_left(5).wrapping_add(w[k] ^ w[(k + 9) & 15].rotate_right(13));
        }
        out = out.rotate_left(9) ^ h as u64;
    }
    out
}

fn words(n: u32) -> u64 {
    let mut text = String::new();
    for i in 0..n {
        text.push_str(["alpha", "beta", "gamma", "delta", "epsilon"][(i * 7 % 5) as usize]);
        text.push(' ');
    }
    let mut m: HashMap<&str, u32, BuildHasherDefault<DefaultHasher>> = HashMap::default();
    for w in text.split_whitespace() {
        *m.entry(w).or_insert(0) += 1;
    }
    ((m.len() as u64) << 32) | m.values().copied().max().unwrap_or(0) as u64
}

fn sorted(n: u32) -> u64 {
    let mut x = 0x2545F4914F6CDD1Du64;
    let mut v: Vec<u64> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.push(x);
    }
    v.sort_unstable();
    v.iter().enumerate().fold(0u64, |h, (i, x)| h.rotate_left(7) ^ x.wrapping_mul(i as u64 + 1))
}

pub fn rest(n: u32) -> u64 {
    words(n).rotate_left(21) ^ sorted(n).rotate_left(42)
}
