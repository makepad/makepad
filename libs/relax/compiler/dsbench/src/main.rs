// Core data-structure microbenchmarks (lane DATASTRUCT): the same source under rustc --release and
// `makepad run`. Each workload is `setup() -> iteration closure` with fixed, seeded inputs; an iteration
// is about a millisecond of rustc time. The timing loop, output line and env are rwbench's harness
// (included by path). Usage: relax-dsbench <workload>...
#[path = "../../rwbench/src/harness.rs"]
mod harness;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::hint::black_box;

fn main() {
    let all: [(&str, harness::Setup); 20] = [
        ("vec_push", vec_push),
        ("vec_extend", vec_extend),
        ("vec_insert", vec_insert),
        ("vec_retain", vec_retain),
        ("iter_chain", iter_chain),
        ("iter_chunks", iter_chunks),
        ("hash_u64", hash_u64),
        ("hash_str", hash_str),
        ("hashset_grow", hashset_grow),
        ("btree_map", btree_map),
        ("vecdeque", vecdeque),
        ("sort_unstable", sort_unstable),
        ("sort_stable", sort_stable),
        ("sort_strings", sort_strings),
        ("binary_search", binary_search),
        ("string_build", string_build),
        ("string_fmt", string_fmt),
        ("bounds_two_slices", bounds_two_slices),
        ("bounds_stencil", bounds_stencil),
        ("bounds_gather", bounds_gather),
    ];
    harness::main_with(&all);
}

fn fold(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn rand_u32s(seed: u64, n: usize) -> Vec<u32> {
    let mut r = Rng(seed);
    (0..n).map(|_| r.next() as u32).collect()
}

fn rand_words(seed: u64, n: usize) -> Vec<String> {
    let mut r = Rng(seed);
    (0..n)
        .map(|_| {
            let len = 3 + (r.next() % 10) as usize;
            (0..len).map(|_| (b'a' + (r.next() % 26) as u8) as char).collect()
        })
        .collect()
}

// --- Vec ---------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Item {
    id: u32,
    x: f32,
    y: f32,
    flags: u16,
}

fn vec_push() -> Box<dyn FnMut() -> u64> {
    Box::new(|| {
        let mut h = 0;
        // growth from empty (realloc path) plus pushes of a 16-byte struct
        for round in 0..8u32 {
            let mut a: Vec<u32> = Vec::new();
            for i in 0..20_000u32 {
                a.push(i ^ round);
            }
            let mut b: Vec<Item> = Vec::with_capacity(64);
            for i in 0..5_000u32 {
                b.push(Item { id: i, x: i as f32, y: round as f32, flags: (i & 7) as u16 });
            }
            h = fold(h, a[black_box(12_345)] as u64 + a.len() as u64 + b[black_box(4_000)].id as u64 + b.len() as u64);
            black_box(&a);
            black_box(&b);
        }
        h
    })
}

fn vec_extend() -> Box<dyn FnMut() -> u64> {
    let src = rand_u32s(1, 4096);
    Box::new(move || {
        let mut h = 0;
        for round in 0..16u32 {
            let mut v: Vec<u32> = Vec::new();
            v.extend_from_slice(&src);
            v.extend(src.iter().map(|x| x.wrapping_add(round)));
            v.extend(src.iter().copied().filter(|x| x & 3 == 0));
            let w: Vec<u64> = src.iter().map(|&x| x as u64 * 3).collect();
            v.extend((0..1024u32).map(|i| i * round));
            h = fold(h, v.len() as u64 + v[black_box(5000)] as u64 + w[black_box(100)]);
            black_box(&v);
        }
        h
    })
}

fn vec_insert() -> Box<dyn FnMut() -> u64> {
    let keys = rand_u32s(2, 3000);
    Box::new(move || {
        // sorted insert into a growing vec (memmove + binary search), then removes
        let mut v: Vec<u32> = Vec::new();
        for &k in &keys {
            let at = v.partition_point(|&x| x < k);
            v.insert(at, k);
        }
        let mut h = fold(0, v[black_box(1000)] as u64);
        for i in 0..1000 {
            let at = (keys[i] as usize) % v.len();
            h = fold(h, v.remove(at) as u64);
        }
        h ^ v.len() as u64
    })
}

fn vec_retain() -> Box<dyn FnMut() -> u64> {
    let base: Vec<Item> = rand_u32s(3, 20_000)
        .into_iter()
        .enumerate()
        .map(|(i, r)| Item { id: i as u32, x: (r & 0xffff) as f32, y: (r >> 16) as f32, flags: (r & 15) as u16 })
        .collect();
    let ints = rand_u32s(4, 40_000);
    Box::new(move || {
        let mut a = base.clone();
        a.retain(|it| it.flags & 3 != 0);
        a.retain_mut(|it| {
            it.x += 1.0;
            it.y < 40_000.0
        });
        let mut b = ints.clone();
        b.retain(|x| x % 3 != 0);
        b.dedup_by_key(|x| *x >> 28);
        fold(fold(a.len() as u64, a[black_box(10)].id as u64), b.len() as u64)
    })
}

// --- slice iteration / iterator chains ---------------------------------------------------------

fn iter_chain() -> Box<dyn FnMut() -> u64> {
    let a: Vec<f32> = rand_u32s(5, 16_384).iter().map(|&x| (x >> 8) as f32 / 65536.0).collect();
    let b: Vec<f32> = rand_u32s(6, 16_384).iter().map(|&x| (x >> 8) as f32 / 65536.0).collect();
    let ids = rand_u32s(7, 16_384);
    Box::new(move || {
        let mut h = 0u64;
        for _ in 0..4 {
            let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
            let cnt = ids.iter().filter(|&&x| x & 1 == 0).count();
            let m = a.iter().enumerate().filter(|(_, &v)| v > 128.0).map(|(i, _)| i).max().unwrap_or(0);
            let s: u64 = ids.iter().map(|&x| (x as u64) & 0xff).sum();
            let any = b.iter().any(|&v| v < 0.0);
            let pos = ids.iter().position(|&x| x == ids[black_box(16_000)]).unwrap_or(0);
            let mn = a.iter().copied().fold(f32::MAX, f32::min);
            let w: u32 = ids.windows(2).map(|w| w[1].wrapping_sub(w[0]) >> 31).sum();
            h = fold(h, dot.to_bits() as u64 ^ cnt as u64 ^ (m as u64) << 20 ^ s ^ any as u64 ^ (pos as u64) << 40 ^ mn.to_bits() as u64 ^ w as u64);
        }
        h
    })
}

fn iter_chunks() -> Box<dyn FnMut() -> u64> {
    let bytes: Vec<u8> = rand_u32s(8, 65_536).iter().map(|&x| x as u8).collect();
    let px: Vec<u32> = rand_u32s(9, 32_768);
    Box::new(move || {
        let mut h = 0u64;
        // chunks_exact sums (codec-style), chunks with remainder, rgba channel extraction
        let s1: u64 = bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap()) >> 7).fold(0, |a, x| a ^ x);
        let s2: u32 = bytes.chunks(13).map(|c| c.iter().map(|&b| b as u32).sum::<u32>()).max().unwrap_or(0);
        let mut lum = 0u64;
        for p in px.chunks_exact(4) {
            for &c in p {
                lum += ((c & 0xff) * 3 + ((c >> 8) & 0xff) * 6 + ((c >> 16) & 0xff)) as u64;
            }
        }
        let rev: u64 = bytes.iter().rev().step_by(3).map(|&b| b as u64).sum();
        h = fold(h, s1 ^ s2 as u64 ^ lum ^ rev);
        h
    })
}

// --- HashMap / HashSet -------------------------------------------------------------------------

fn hash_u64() -> Box<dyn FnMut() -> u64> {
    let keys: Vec<u64> = { let mut r = Rng(10); (0..8192).map(|_| r.next() >> 20).collect() };
    let probes: Vec<u64> = { let mut r = Rng(11); (0..8192).map(|_| r.next() >> 20).collect() };
    let mut map: HashMap<u64, u32> = HashMap::new();
    for (i, &k) in keys.iter().enumerate() {
        map.insert(k, i as u32);
    }
    Box::new(move || {
        let mut h = 0u64;
        // hits, misses, entry() updates on a warm table
        for &k in &keys {
            h = h.wrapping_add(*map.get(&k).unwrap() as u64);
        }
        for &k in &probes {
            if map.contains_key(&k) {
                h ^= k;
            }
        }
        for &k in keys.iter().step_by(4) {
            *map.entry(k).or_insert(0) += 1;
        }
        for &k in keys.iter().step_by(4) {
            let v = map.get_mut(&k).unwrap();
            h = h.wrapping_add(*v as u64);
            *v -= 1;
        }
        fold(h, map.len() as u64)
    })
}

fn hash_str() -> Box<dyn FnMut() -> u64> {
    let words = rand_words(12, 4096);
    Box::new(move || {
        // word counting: String keys, &str lookups
        let mut counts: HashMap<&str, u32> = HashMap::new();
        for w in &words {
            *counts.entry(w.as_str()).or_insert(0) += 1;
        }
        let mut owned: HashMap<String, usize> = HashMap::with_capacity(64);
        for (i, w) in words.iter().enumerate().step_by(3) {
            owned.insert(w.clone(), i);
        }
        let mut h = 0u64;
        for w in &words {
            if let Some(&i) = owned.get(w.as_str()) {
                h = h.wrapping_add(i as u64);
            }
            h = h.wrapping_add(counts[w.as_str()] as u64);
        }
        fold(h, counts.len() as u64 ^ (owned.len() as u64) << 32)
    })
}

fn hashset_grow() -> Box<dyn FnMut() -> u64> {
    let keys = rand_u32s(13, 30_000);
    Box::new(move || {
        // growth from empty (rehash path), removes, re-inserts
        let mut s: HashSet<u32> = HashSet::new();
        for &k in &keys {
            s.insert(k >> 4);
        }
        let mut h = s.len() as u64;
        for &k in keys.iter().step_by(2) {
            if s.remove(&(k >> 4)) {
                h += 1;
            }
        }
        for &k in keys.iter().step_by(5) {
            s.insert(k);
        }
        let mut pairs: HashMap<(u32, u32), f32> = HashMap::new();
        for w in keys.windows(2).take(8000) {
            pairs.insert((w[0] & 0xfff, w[1] & 0xf), w[0] as f32);
        }
        fold(fold(h, s.len() as u64), pairs.len() as u64)
    })
}

// --- BTreeMap ----------------------------------------------------------------------------------

fn btree_map() -> Box<dyn FnMut() -> u64> {
    let keys = rand_u32s(14, 6000);
    Box::new(move || {
        let mut m: BTreeMap<u32, u32> = BTreeMap::new();
        for (i, &k) in keys.iter().enumerate() {
            m.insert(k >> 8, i as u32);
        }
        let mut h = 0u64;
        for &k in keys.iter().step_by(2) {
            if let Some(v) = m.get(&(k >> 8)) {
                h = h.wrapping_add(*v as u64);
            }
        }
        for (k, v) in m.range(1000..2_000_000).take(500) {
            h ^= (*k as u64) << 8 ^ *v as u64;
        }
        let first: u64 = m.iter().take(2000).map(|(k, v)| (*k ^ *v) as u64).sum();
        for &k in keys.iter().step_by(3) {
            m.remove(&(k >> 8));
        }
        fold(fold(h, first), m.len() as u64)
    })
}

// --- VecDeque ----------------------------------------------------------------------------------

fn vecdeque() -> Box<dyn FnMut() -> u64> {
    let ops = rand_u32s(15, 50_000);
    Box::new(move || {
        // BFS-style queue: push_back/pop_front with wrap-around, some push_front, indexing
        let mut q: VecDeque<u32> = VecDeque::new();
        let mut h = 0u64;
        for &o in &ops {
            match o & 7 {
                0..=3 => q.push_back(o),
                4 | 5 => {
                    if let Some(x) = q.pop_front() {
                        h = h.wrapping_add(x as u64);
                    }
                }
                6 => q.push_front(o >> 3),
                _ => {
                    if !q.is_empty() {
                        let i = (o as usize) % q.len();
                        h ^= q[i] as u64;
                    }
                }
            }
        }
        let s: u64 = q.iter().map(|&x| (x & 0xffff) as u64).sum();
        q.extend(ops.iter().take(1000));
        q.retain(|&x| x & 1 == 0);
        fold(fold(h, s), q.len() as u64)
    })
}

// --- sort / binary_search ----------------------------------------------------------------------

fn sort_unstable() -> Box<dyn FnMut() -> u64> {
    let ints = rand_u32s(16, 20_000);
    let floats: Vec<f32> = rand_u32s(17, 8_000).iter().map(|&x| (x >> 9) as f32).collect();
    Box::new(move || {
        let mut a = ints.clone();
        a.sort_unstable();
        let mut b = floats.clone();
        b.sort_unstable_by(|x, y| x.total_cmp(y));
        let mut c = ints[..5000].to_vec();
        c.sort_unstable_by_key(|&x| std::cmp::Reverse(x & 0xffff));
        fold(fold(a[black_box(777)] as u64, b[black_box(333)].to_bits() as u64), c[black_box(10)] as u64)
    })
}

#[derive(Clone)]
struct Rec {
    key: u32,
    depth: f32,
    name_idx: u16,
}

fn sort_stable() -> Box<dyn FnMut() -> u64> {
    let recs: Vec<Rec> = rand_u32s(18, 8000)
        .iter()
        .enumerate()
        .map(|(i, &r)| Rec { key: r & 0x3ff, depth: (r >> 10) as f32, name_idx: i as u16 })
        .collect();
    let ints = rand_u32s(19, 10_000);
    Box::new(move || {
        // draw-list style stable sorts by key, by float, merge of presorted runs
        let mut a = recs.clone();
        a.sort_by_key(|r| r.key);
        let mut b = recs.clone();
        b.sort_by(|x, y| x.depth.partial_cmp(&y.depth).unwrap());
        let mut c = ints.clone();
        c[..5000].sort();
        c.sort();
        fold(fold(a[black_box(100)].name_idx as u64, b[black_box(200)].name_idx as u64), c[black_box(300)] as u64)
    })
}

fn sort_strings() -> Box<dyn FnMut() -> u64> {
    let words = rand_words(20, 3000);
    Box::new(move || {
        let mut a: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
        a.sort_unstable();
        let mut b = words.clone();
        b.sort_by(|x, y| x.len().cmp(&y.len()).then_with(|| y.cmp(x)));
        b.dedup();
        fold(fold(a[black_box(5)].len() as u64, a[1500].as_bytes()[0] as u64), b.len() as u64 ^ b[black_box(9)].len() as u64)
    })
}

fn binary_search() -> Box<dyn FnMut() -> u64> {
    let mut sorted = rand_u32s(21, 65_536);
    sorted.sort_unstable();
    let probes = rand_u32s(22, 20_000);
    let pairs: Vec<(u32, f32)> = sorted.iter().step_by(16).map(|&k| (k, k as f32)).collect();
    Box::new(move || {
        let mut h = 0u64;
        for &p in &probes {
            match sorted.binary_search(&p) {
                Ok(i) => h ^= i as u64,
                Err(i) => h = h.wrapping_add(i as u64),
            }
            let j = pairs.partition_point(|e| e.0 < p);
            h = h.wrapping_add(j as u64);
            if let Ok(k) = pairs.binary_search_by_key(&(p | 1), |e| e.0) {
                h ^= k as u64;
            }
        }
        h
    })
}

// --- strings -----------------------------------------------------------------------------------

fn string_build() -> Box<dyn FnMut() -> u64> {
    let words = rand_words(23, 4000);
    Box::new(move || {
        let mut s = String::new();
        for w in &words {
            s.push_str(w);
            s.push(' ');
        }
        let up = s.to_uppercase();
        let joined = words[..1000].join(", ");
        let replaced = joined.replace("ab", "XYZ");
        let mut cs = String::with_capacity(16);
        let mut n = 0usize;
        for c in s.chars() {
            if c == ' ' {
                n += cs.len();
                cs.clear();
            } else {
                cs.push(c);
            }
        }
        let parts = s.split(' ').filter(|p| p.starts_with('q')).count();
        let trimmed: usize = words.iter().map(|w| w.trim_start_matches('a').len()).sum();
        fold(fold(s.len() as u64 ^ up.len() as u64, replaced.len() as u64), (n ^ parts ^ trimmed) as u64)
    })
}

fn string_fmt() -> Box<dyn FnMut() -> u64> {
    let ints = rand_u32s(24, 3000);
    Box::new(move || {
        let mut s = String::with_capacity(1 << 16);
        for (i, &x) in ints.iter().enumerate() {
            let f = x as f64 / 1e6;
            write!(s, "{i}:{x} {f:.3} {:>8} {:x};", x & 0xffff, x).unwrap();
        }
        let t: Vec<String> = ints[..500].iter().map(|x| x.to_string()).collect();
        let parsed: u64 = t.iter().map(|x| x.parse::<u64>().unwrap()).sum();
        let d = format!("{:?}", &ints[..200]);
        fold(fold(s.len() as u64, parsed), d.len() as u64)
    })
}

// --- bounds-checked index loops ---------------------------------------------------------------

#[inline(never)]
fn axpy_idx(y: &mut [f32], x: &[f32], k: f32) {
    // two slices of unrelated length: per-iteration check on x[i] unless versioned
    for i in 0..y.len() {
        y[i] += k * x[i];
    }
}

#[inline(never)]
fn mix_idx(out: &mut [u32], a: &[u32], b: &[u32], n: usize) {
    for i in 0..n {
        out[i] = a[i].wrapping_mul(3) ^ b[i] >> 1;
    }
}

fn bounds_two_slices() -> Box<dyn FnMut() -> u64> {
    let x: Vec<f32> = rand_u32s(25, 4096).iter().map(|&v| (v >> 10) as f32 / 1024.0).collect();
    let a = rand_u32s(26, 4096);
    let b = rand_u32s(27, 4096);
    let mut y = vec![0.5f32; 4096];
    let mut out = vec![0u32; 4096];
    Box::new(move || {
        y.fill(0.5);
        for r in 0..256 {
            axpy_idx(&mut y, black_box(&x), 0.001 * r as f32);
            mix_idx(&mut out, black_box(&a), black_box(&b), black_box(4096));
        }
        fold(y[black_box(4000)].to_bits() as u64, out[black_box(123)] as u64)
    })
}

#[inline(never)]
fn blur3(dst: &mut [f32], src: &[f32]) {
    // v[i + k] stencil: check on i+1, i+2
    for i in 0..src.len() - 2 {
        dst[i] = (src[i] + src[i + 1] * 2.0 + src[i + 2]) * 0.25;
    }
}

#[inline(never)]
fn blur_rows(dst: &mut [u16], src: &[u16], w: usize, h: usize) {
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = y * w + x;
            dst[c] = ((src[c - w] as u32 + src[c + w] as u32 + src[c - 1] as u32 + src[c + 1] as u32 + 4 * src[c] as u32) >> 3) as u16;
        }
    }
}

fn bounds_stencil() -> Box<dyn FnMut() -> u64> {
    let src: Vec<f32> = rand_u32s(28, 16_384).iter().map(|&v| (v >> 12) as f32).collect();
    let img: Vec<u16> = rand_u32s(29, 256 * 128).iter().map(|&v| v as u16).collect();
    let mut dst = vec![0f32; 16_384];
    let mut idst = vec![0u16; 256 * 128];
    Box::new(move || {
        for _ in 0..32 {
            blur3(&mut dst, black_box(&src));
        }
        for _ in 0..8 {
            blur_rows(&mut idst, black_box(&img), black_box(256), black_box(128));
        }
        fold(dst[black_box(9000)].to_bits() as u64, idst[black_box(3000)] as u64)
    })
}

#[inline(never)]
fn gather(out: &mut [f32], table: &[f32], idx: &[u32]) {
    // data-dependent index: the check cannot be hoisted (control: must stay)
    for i in 0..idx.len() {
        out[i] = table[idx[i] as usize & 1023] * 2.0;
    }
}

#[inline(never)]
fn prefix(v: &mut [u64]) {
    // loop-carried v[i - 1]
    for i in 1..v.len() {
        v[i] = v[i].wrapping_add(v[i - 1]);
    }
}

fn bounds_gather() -> Box<dyn FnMut() -> u64> {
    let table: Vec<f32> = (0..1024).map(|i| i as f32 * 0.5).collect();
    let idx = rand_u32s(30, 16_384);
    let mut out = vec![0f32; 16_384];
    let base: Vec<u64> = rand_u32s(31, 16_384).iter().map(|&v| v as u64).collect();
    Box::new(move || {
        for _ in 0..8 {
            gather(&mut out, black_box(&table), black_box(&idx));
        }
        let mut v = base.clone();
        for _ in 0..4 {
            prefix(&mut v);
        }
        fold(out[black_box(5)].to_bits() as u64, v[black_box(16_000)])
    })
}
