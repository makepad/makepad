//! A file's published token array in 4 bytes per token (the 9 byte `Tok`, packed).
//!
//! Layout, `u32` words (the whole array is one aligned word slice, the same bytes whether
//! built in this process or mapped from a cache file):
//!   [n tokens, n escapes, bases (one per block of 64 tokens), rows (one per token), escapes]
//! A row is `kind | len << 8 | off << 16`: `lo = base[block] + off`, `hi = lo + len`.
//! A token that does not fit (length over 255, or more than 65535 bytes past the block's
//! lowest start) has the escape bit (0x80) in its row and the index of its `(lo, hi)` pair
//! in the escape table in the row's upper 24 bits. `get(i)` is O(1): one base, one row, at
//! most one escape pair; there is no cursor and no search.

use crate::lexer::{Tok, T};

const BLOCK: usize = 64;
const ESCAPE: u32 = 0x80;

// the kind shares its byte with the escape bit
const _: () = assert!((T::OtherPunct as u8) < 0x80);

pub struct PackedToks {
    store: Store,
    n: usize,
    bases: *const u32,
    rows: *const u32,
    esc: *const u32,
}

enum Store {
    Owned(#[allow(dead_code)] Vec<u32>),
    Mapped(&'static [u32]),
}

// the pointers point into `store` (a heap Vec that is never changed, or a mapping that lives
// as long as the process)
unsafe impl Send for PackedToks {}
unsafe impl Sync for PackedToks {}

impl Default for PackedToks {
    fn default() -> Self { Self::encode(&[]) }
}

impl PackedToks {
    /// The array for `toks`.
    pub fn encode(toks: &[Tok]) -> Self {
        let n = toks.len();
        let nb = n.div_ceil(BLOCK);
        let mut words: Vec<u32> = Vec::with_capacity(2 + nb + n);
        words.extend_from_slice(&[n as u32, 0]);
        let mut esc: Vec<u32> = Vec::new();
        for block in toks.chunks(BLOCK) {
            words.push(block.iter().map(|t| t.lo).min().unwrap_or(0));
        }
        for (i, t) in toks.iter().enumerate() {
            let (lo, hi, kind) = (t.lo, t.hi, t.kind as u8 as u32);
            let base = words[2 + i / BLOCK];
            let row = if hi >= lo && hi - lo <= 255 && lo - base <= 0xffff {
                kind | (hi - lo) << 8 | (lo - base) << 16
            } else {
                let e = (esc.len() / 2) as u32;
                assert!(e < 1 << 24, "more than 16M escaped tokens in one file");
                esc.extend_from_slice(&[lo, hi]);
                kind | ESCAPE | e << 8
            };
            words.push(row);
        }
        words[1] = (esc.len() / 2) as u32;
        words.extend_from_slice(&esc);
        words.shrink_to_fit();
        let (bases, rows, escp) = Self::pointers(&words, n, nb);
        Self { n, bases, rows, esc: escp, store: Store::Owned(words) }
    }

    /// An array stored by `words()`; None when the words are not a whole array.
    pub fn mapped(words: &'static [u32]) -> Option<Self> {
        let (n, ne) = (*words.first()? as usize, *words.get(1)? as usize);
        let nb = n.div_ceil(BLOCK);
        if words.len() != 2 + nb + n + 2 * ne { return None; }
        let (bases, rows, esc) = Self::pointers(words, n, nb);
        Some(Self { n, bases, rows, esc, store: Store::Mapped(words) })
    }

    fn pointers(words: &[u32], n: usize, nb: usize) -> (*const u32, *const u32, *const u32) {
        let p = words.as_ptr();
        unsafe { (p.add(2), p.add(2 + nb), p.add(2 + nb + n)) }
    }

    /// The array's words (what a cache image stores).
    pub fn words(&self) -> &[u32] {
        match &self.store {
            Store::Owned(v) => v,
            Store::Mapped(s) => s,
        }
    }
    pub fn is_mapped(&self) -> bool { matches!(self.store, Store::Mapped(_)) }
    /// Heap bytes this array holds (0 when mapped).
    pub fn heap_bytes(&self) -> usize {
        match &self.store {
            Store::Owned(v) => v.capacity() * 4,
            Store::Mapped(_) => 0,
        }
    }
    /// The same array over a mapping: the owned words are dropped.
    pub fn len(&self) -> usize { self.n }
    pub fn is_empty(&self) -> bool { self.n == 0 }

    /// Token `i` (the caller keeps `i < len()`).
    #[inline]
    pub fn at(&self, i: usize) -> Tok {
        assert!(i < self.n, "token {i} of {}", self.n);
        unsafe {
            let row = *self.rows.add(i);
            let kind: T = std::mem::transmute::<u8, T>((row & 0x7f) as u8);
            if row & ESCAPE == 0 {
                let lo = *self.bases.add(i / BLOCK) + (row >> 16);
                Tok { kind, lo, hi: lo + (row >> 8 & 0xff) }
            } else {
                let e = self.esc.add(2 * (row >> 8) as usize);
                Tok { kind, lo: *e, hi: *e.add(1) }
            }
        }
    }
    #[inline]
    pub fn get(&self, i: usize) -> Option<Tok> { (i < self.n).then(|| self.at(i)) }
    pub fn last(&self) -> Option<Tok> { self.n.checked_sub(1).map(|i| self.at(i)) }
    pub fn iter(&self) -> impl Iterator<Item = Tok> + '_ { (0..self.n).map(move |i| self.at(i)) }
    /// Tokens from `start` on.
    pub fn iter_from(&self, start: usize) -> impl Iterator<Item = Tok> + '_ { (start..self.n).map(move |i| self.at(i)) }
    /// Tokens [lo, hi) (the caller keeps lo <= hi <= len()).
    pub fn range(&self, lo: usize, hi: usize) -> Vec<Tok> { (lo..hi).map(|i| self.at(i)).collect() }
    pub fn to_vec(&self) -> Vec<Tok> { self.range(0, self.n) }
    /// slice::partition_point over the tokens
    pub fn partition_point(&self, mut pred: impl FnMut(&Tok) -> bool) -> usize {
        let (mut lo, mut hi) = (0, self.n);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if pred(&self.at(mid)) { lo = mid + 1 } else { hi = mid }
        }
        lo
    }
}

impl From<&[Tok]> for PackedToks {
    fn from(toks: &[Tok]) -> Self { Self::encode(toks) }
}
impl From<Vec<Tok>> for PackedToks {
    fn from(toks: Vec<Tok>) -> Self { Self::encode(&toks) }
}

impl std::fmt::Debug for PackedToks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_list().entries(self.iter()).finish() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_shape() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |m: u64| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed % m) as u32 };
        for n in [0usize, 1, 63, 64, 65, 200, 1000] {
            let mut toks = Vec::new();
            let mut pos = 0u32;
            for _ in 0..n {
                let kind = unsafe { std::mem::transmute::<u8, T>(next(T::OtherPunct as u64 + 1) as u8) };
                pos += next(4);
                let len = match next(20) { 0 => 256 + next(70000), 1 => 255, 2 => 0, _ => next(12) };
                let lo = match next(30) { 0 => next(1 << 31), 1 => pos.saturating_sub(next(100000)), 2 => pos + 70000, _ => pos };
                // hi < lo only for a degenerate token
                toks.push(Tok { kind, lo, hi: if next(50) == 0 { lo.wrapping_sub(1) } else { lo + len } });
                pos = pos.max(lo);
            }
            let p = PackedToks::encode(&toks);
            assert_eq!(p.len(), n);
            assert_eq!(p.to_vec(), toks);
            let words: &'static [u32] = Box::leak(p.words().to_vec().into_boxed_slice());
            let m = PackedToks::mapped(words).unwrap();
            for (i, t) in toks.iter().enumerate() { assert_eq!(m.at(i), *t); }
            assert!(m.get(n).is_none());
            for key in [0, 100, 5000, 1 << 20] {
                let sorted: Vec<Tok> = { let mut v = toks.clone(); v.sort_by_key(|t| t.lo); v };
                let ps = PackedToks::encode(&sorted);
                assert_eq!(ps.partition_point(|t| t.lo <= key), sorted.partition_point(|t| t.lo <= key));
            }
        }
        assert!(PackedToks::mapped(Box::leak(vec![5u32, 0].into_boxed_slice())).is_none());
    }

    /// get(i) against the 9 byte array, sequential and random:
    /// MAKEPAD_RUN_BENCH_DIR=<dir of .rs files> cargo test --release -p relax-lex bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_get() {
        use std::hint::black_box;
        use std::time::Instant;
        let dir = std::env::var("MAKEPAD_RUN_BENCH_DIR").expect("MAKEPAD_RUN_BENCH_DIR");
        let mut toks: Vec<Tok> = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        let mut files = Vec::new();
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() { stack.push(p) } else if p.extension().is_some_and(|x| x == "rs") { files.push(p) }
            }
        }
        files.sort();
        // each file's tokens, rebased as consecutive files of one array (as a crate's would be per file)
        let mut packed: Vec<PackedToks> = Vec::new();
        let mut all: Vec<Vec<Tok>> = Vec::new();
        for f in files {
            let Ok(src) = std::fs::read(&f) else { continue };
            let mut v = Vec::new();
            if crate::lexer::lex(&src, 2021, &mut v).is_err() { continue; }
            toks.extend_from_slice(&v);
            packed.push(PackedToks::encode(&v));
            all.push(v);
        }
        let n = toks.len();
        let esc: usize = packed.iter().map(|p| p.words()[1] as usize).sum();
        let bytes: usize = packed.iter().map(|p| p.words().len() * 4).sum();
        println!("{} files, {n} tokens, {esc} escapes ({:.3}%), packed {bytes} B ({:.2} B/tok) vs {} B", all.len(), esc as f64 * 100.0 / n as f64, bytes as f64 / n as f64, n * 9);
        // random indices per file (a fixed pseudo-random walk)
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
        for round in 0..3 {
            let t = Instant::now();
            let mut acc = 0u64;
            for v in &all { for t in v.iter() { acc += (t.lo ^ t.hi) as u64 + t.kind as u64; } }
            black_box(acc);
            let seq9 = t.elapsed().as_nanos() as f64 / n as f64;
            let t = Instant::now();
            let mut acc = 0u64;
            for p in &packed { for i in 0..p.len() { let t = p.at(i); acc += (t.lo ^ t.hi) as u64 + t.kind as u64; } }
            black_box(acc);
            let seq4 = t.elapsed().as_nanos() as f64 / n as f64;
            let t = Instant::now();
            let mut acc = 0u64;
            for p in &packed { for t in p.iter() { acc += (t.lo ^ t.hi) as u64 + t.kind as u64; } }
            black_box(acc);
            let iter4 = t.elapsed().as_nanos() as f64 / n as f64;
            let picks: Vec<(usize, usize)> = (0..n).map(|_| { let f = (rnd() % all.len() as u64) as usize; (f, (rnd() % all[f].len() as u64) as usize) }).collect();
            let t = Instant::now();
            let mut acc = 0u64;
            for &(f, i) in &picks { let t = all[f][i]; acc += (t.lo ^ t.hi) as u64 + t.kind as u64; }
            black_box(acc);
            let rnd9 = t.elapsed().as_nanos() as f64 / n as f64;
            let t = Instant::now();
            let mut acc = 0u64;
            for &(f, i) in &picks { let t = packed[f].at(i); acc += (t.lo ^ t.hi) as u64 + t.kind as u64; }
            black_box(acc);
            let rnd4 = t.elapsed().as_nanos() as f64 / n as f64;
            println!("round {round}: sequential ns/op 9B {seq9:.2} packed {seq4:.2} (iter {iter4:.2}); random 9B {rnd9:.2} packed {rnd4:.2}");
        }
    }
}
