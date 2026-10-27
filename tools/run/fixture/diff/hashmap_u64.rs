// diff_fast corpus program: HashMap<u64, u32> filled from seeded keys, then every key looked up
// (dsbench hash_u64: Relax returned None for an inserted key). rustc --test -O and `makepad test`
// (all tiers) must print identical D lines.
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, DefaultHasher, Hash, Hasher};
static FIRST: std::sync::OnceLock<(Vec<u64>, Vec<u64>, u64)> = std::sync::OnceLock::new();
// one RandomState for every round (real std seeds each new one differently)
static BH: std::sync::OnceLock<std::collections::hash_map::RandomState> = std::sync::OnceLock::new();
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() {
    rounds(work);
    closure_rounds();
}

/// dsbench hash_u64's iteration, verbatim shape: x64 O3 hashed one way in the closure (a spilled
/// operand loaded over its window register) and the table was filled with the other.
fn lookups_closure() -> Box<dyn FnMut() -> u64> {
    let keys: Vec<u64> = { let mut r = 10u64; (0..8192).map(|_| next(&mut r) >> 20).collect() };
    let probes: Vec<u64> = { let mut r = 11u64; (0..8192).map(|_| next(&mut r) >> 20).collect() };
    let mut map: HashMap<u64, u32> = HashMap::new();
    for (i, &k) in keys.iter().enumerate() {
        map.insert(k, i as u32);
    }
    Box::new(move || {
        let mut h = 0u64;
        let mut missing = 0u64;
        for &k in &keys {
            match map.get(&k) {
                Some(v) => h = h.wrapping_add(*v as u64),
                None => missing += 1,
            }
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
            if let Some(v) = map.get_mut(&k) {
                h = h.wrapping_add(*v as u64);
                *v -= 1;
            }
        }
        h ^ missing << 48 ^ (map.len() as u64) << 32
    })
}

fn closure_rounds() {
    let mut f = lookups_closure();
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    // enough calls for the O3 tier to take the closure, then the rounds that print
    for _ in 0..50 { f(); }
    for r in 0..4 {
        d!("closure round {} {}", r, f());
        if ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); }
        for _ in 0..50 { f(); }
    }
}

fn next(x: &mut u64) -> u64 {
    let mut v = *x;
    v ^= v >> 12;
    v ^= v << 25;
    v ^= v >> 27;
    *x = v;
    v.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[inline(never)]
fn h<T: Hash + ?Sized>(x: &T) -> u64 {
    let mut s = DefaultHasher::new();
    x.hash(&mut s);
    s.finish()
}

fn work() {
    // hash values themselves are std's choice (Relax's std does not use SipHash), so only their
    // consistency is printed: the same key hashes the same through every path and in every round
    // (O3 code replaces baseline code between rounds)
    let keys = [0u64, 1, 12345, 1 << 40, u64::MAX, 0x2545_F491_4F6C_DD1D];
    let now: Vec<u64> = keys.iter().map(|k| h(k)).collect();
    let bh = BH.get_or_init(std::collections::hash_map::RandomState::new);
    let via_build: Vec<u64> = keys.iter().map(|k| bh.hash_one(k)).collect();
    let via_build2: Vec<u64> = keys.iter().map(|k| { let mut s = bh.build_hasher(); k.hash(&mut s); s.finish() }).collect();
    d!("hash_one equals build_hasher+finish: {}", via_build == via_build2);
    let mut s = DefaultHasher::new();
    s.write_u64(99);
    s.write_u32(5);
    s.write(b"abcdefghijklmnopq");
    let mixed = s.finish();
    let first = FIRST.get_or_init(|| (now.clone(), via_build.clone(), mixed));
    d!("hash consistent across rounds {} {} {}", first.0 == now, first.1 == via_build, first.2 == mixed);
    d!("hash distinct {}", { let mut v = now.clone(); v.sort_unstable(); v.dedup(); v.len() });
    // a set grown from empty, removes, re-inserts (dsbench hashset_grow)
    let mut r = 13u64;
    let keys: Vec<u32> = (0..30_000).map(|_| next(&mut r) as u32).collect();
    let mut set: HashSet<u32> = HashSet::new();
    for &k in &keys { set.insert(k >> 4); }
    let a = set.len();
    let mut removed = 0;
    for &k in keys.iter().step_by(2) { if set.remove(&(k >> 4)) { removed += 1; } }
    for &k in keys.iter().step_by(5) { set.insert(k); }
    let mut pairs: HashMap<(u32, u32), f32> = HashMap::new();
    for w in keys.windows(2).take(8000) { pairs.insert((w[0] & 0xfff, w[1] & 0xf), w[0] as f32); }
    d!("set {} removed {} after {} pairs {}", a, removed, set.len(), pairs.len());
    for n in [10usize, 100, 1000, 8192] {
        let mut r = 10u64;
        let keys: Vec<u64> = (0..n).map(|_| next(&mut r) >> 20).collect();
        let mut map: HashMap<u64, u32> = HashMap::new();
        for (i, &k) in keys.iter().enumerate() {
            map.insert(k, i as u32);
        }
        let mut missing = 0usize;
        let mut first = None;
        let mut sum = 0u64;
        for (i, &k) in keys.iter().enumerate() {
            match map.get(&k) {
                Some(v) => sum = sum.wrapping_add(*v as u64),
                None => {
                    missing += 1;
                    if first.is_none() { first = Some((i, k)); }
                }
            }
        }
        let distinct = { let mut s = keys.clone(); s.sort_unstable(); s.dedup(); s.len() };
        d!("n={} len={} distinct={} missing={} first={:?} sum={}", n, map.len(), distinct, missing, first, sum);
        let moved = Box::new(move || map.get(&keys[keys.len() / 2]).copied());
        d!("moved {:?}", moved());
    }
}
