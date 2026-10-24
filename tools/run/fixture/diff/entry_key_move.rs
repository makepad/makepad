// diff_fast corpus program: HashMap::entry with a by-value struct key whose Drop reads every field.
// DFREE (Sandbox on 166, double free in glibc): the inlined rustc_entry moved the key into the
// Occupied variant one word above where it lay (same frame storage), with a forward copy, so the
// key the occupied path drops had its words shifted (len = ptr, the live-edit ext word = the tag
// word's stale stack bytes) and its drop freed a stack address. rustc --test -O and `makepad test`
// (all tiers) must print identical D lines.
use std::collections::HashMap;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() {
    rounds(work);
}

static DROPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(PartialEq, Eq, Hash)]
struct Key {
    name: String,
    kind: u8,
    tail: u64,
}
impl Drop for Key {
    fn drop(&mut self) {
        // every word of the key: a shifted copy shows here (len, tail)
        let h = self.name.len() as u64 ^ (self.kind as u64) << 8 ^ self.tail.rotate_left(17) ^ self.name.bytes().map(|b| b as u64).sum::<u64>() << 20;
        DROPS.fetch_add(h, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The shape of the Sandbox function: an owned key from a &str and a u8, the entry's counters.
#[inline(never)]
fn hit(map: &mut HashMap<Key, (u32, u32)>, name: &str, kind: u8, tail: u64, force: bool) -> bool {
    let e = map.entry(Key { name: name.to_string(), kind, tail }).or_insert((0, 0));
    e.1 += 1;
    if e.0 >= 6 && !force {
        return false;
    }
    if e.1 < 60 {
        return false;
    }
    e.1 = 0;
    if !force {
        e.0 += 1;
    }
    true
}

/// The key handed in by value (an argument slot, not a temporary of the caller).
#[inline(never)]
fn hit_key(map: &mut HashMap<Key, (u32, u32)>, k: Key) -> u32 {
    let e = map.entry(k).or_insert((0, 0));
    e.1 += 1;
    e.1
}

/// `Option<Key>` taken through entry (the occupied variant carries `Some(key)`).
#[inline(never)]
fn hit_opt(map: &mut HashMap<Key, u64>, k: Option<Key>) -> u64 {
    match k {
        Some(k) => {
            let v = map.entry(k).or_default();
            *v += 3;
            *v
        }
        None => 0,
    }
}

fn work() {
    DROPS.store(0, std::sync::atomic::Ordering::Relaxed);
    let names = ["Microsoft Xbox Controller", "sandbox/games/classic-doom", "pad", "gen/characters/real-goat", "x"];
    let mut map: HashMap<Key, (u32, u32)> = HashMap::new();
    let mut fired = 0u64;
    let mut sum = 0u64;
    for i in 0..2000u64 {
        let n = names[(i % 5) as usize];
        if hit(&mut map, n, (i % 3) as u8, i % 7, i % 11 == 0) {
            fired += 1 + i;
        }
        sum = sum.wrapping_add(hit_key(&mut map, Key { name: format!("{n}#{}", i % 13), kind: (i % 2) as u8, tail: i % 5 }) as u64);
    }
    let mut m2: HashMap<Key, u64> = HashMap::new();
    let mut s2 = 0u64;
    for i in 0..500u64 {
        let k = if i % 9 == 0 { None } else { Some(Key { name: names[(i % 4) as usize].to_string(), kind: (i % 3) as u8, tail: i % 4 }) };
        s2 = s2.wrapping_add(hit_opt(&mut m2, k));
    }
    let mut ks: Vec<(String, u8, u64, u32, u32)> = map.iter().map(|(k, v)| (k.name.clone(), k.kind, k.tail, v.0, v.1)).collect();
    ks.sort();
    let mut h = 0u64;
    for (n, k, t, a, b) in &ks {
        h = h.wrapping_mul(31).wrapping_add(n.len() as u64 ^ (*k as u64) << 8 ^ t << 16 ^ (*a as u64) << 24 ^ (*b as u64) << 40);
    }
    d!("entries {} fired {} sum {} s2 {} table {}", ks.len(), fired, sum, s2, h);
    drop(map);
    drop(m2);
    d!("drops {}", DROPS.load(std::sync::atomic::Ordering::Relaxed));
}
