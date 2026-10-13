// Order-independent observations for grouped HashMap/HashSet probing.
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};

#[derive(Clone)]
struct Builder { mode: u64 }
struct Cheap { mode: u64, value: u64 }
impl BuildHasher for Builder {
    type Hasher = Cheap;
    fn build_hasher(&self) -> Cheap { Cheap { mode: self.mode, value: 0 } }
}
impl Hasher for Cheap {
    fn finish(&self) -> u64 {
        if self.mode == 0 { self.value.wrapping_mul(0x9e3779b97f4a7c15) }
        else if self.mode == 1 { 0 }
        else if self.mode == 3 { 63 }
        else if self.mode == 4 { 63 | ((self.value & 1) << 57) }
        else { (self.value & 1) << 57 }
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes { self.value = self.value.wrapping_mul(257).wrapping_add(*b as u64); }
    }
    fn write_u64(&mut self, value: u64) { self.value = value; }
}
fn random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
#[test]
fn a01_small_wraparound() {
    for cap in 0..16usize {
        let mut m = HashMap::with_capacity_and_hasher(cap, Builder { mode: 1 });
        for k in 0..32u64 { m.insert(k,k*3); }
        for k in 0..24u64 { m.remove(&k); }
        m.reserve(20);
        for k in 40..60u64 { m.insert(k,k*3); }
        m.shrink_to_fit();
        let mut keys: Vec<u64> = m.keys().copied().collect();
        keys.sort_unstable();
        let mut sum = 0;
        for k in keys { sum += k + *m.get(&k).unwrap(); }
        println!("| small {} {} {} {}",cap,m.len(),sum,m.capacity() >= m.len());
    }
}
#[test]
fn a02_randomized_collisions() {
    for mode in 0..3u64 {
        let mut m = HashMap::with_hasher(Builder { mode });
        let mut seed = 0x4321fedc9876abcdu64;
        let mut observations = 0u64;
        for step in 0..10000u64 {
            let r = random(&mut seed);
            let k = (r >> 8) % 128;
            let v = r >> 32;
            match r % 10 {
                0..=3 => { observations ^= m.insert(k,v).unwrap_or(0); }
                4..=5 => { observations ^= m.remove(&k).unwrap_or(0); }
                6 => { observations ^= m.get(&k).copied().unwrap_or(0); }
                7 => { *m.entry(k).or_insert(v) += 1; }
                8 => {
                    if step % 11 == 0 { m.retain(|key,val| { *val ^= 7; key % 5 != 0 }); }
                    else { m.reserve(16); }
                }
                _ => { if step % 13 == 0 { m.shrink_to_fit(); } }
            }
        }
        let mut clone = m.clone();
        let mut keys: Vec<u64> = clone.keys().copied().collect();
        keys.sort_unstable();
        println!("| random {} {} {}",mode,m.len(),observations);
        for k in keys { println!("| item {} {}",k,clone.get(&k).unwrap()); }
        let mut sum = 0u64;
        for (k,v) in clone.drain() { sum = sum.wrapping_add(k ^ v); }
        println!("| drained {} {} {}",mode,sum,clone.is_empty());
        clone.shrink_to_fit();
        println!("| empty capacity {}",clone.capacity());
    }
}
#[test]
fn a03_sets_and_partial_drain() {
    let mut s = HashSet::with_hasher(Builder { mode: 2 });
    for i in 0..200u64 { s.insert(i); }
    for i in 0..180u64 { s.remove(&i); }
    s.reserve(50);
    for i in 200..240u64 { s.insert(i); }
    s.retain(|k| k % 3 != 0);
    let mut values: Vec<u64> = s.iter().copied().collect();
    values.sort_unstable();
    for v in values { println!("| set {} {}",v,s.contains(&v)); }
    let mut m = HashMap::new();
    for i in 0..100u64 { m.insert(i,i); }
    { let mut d = m.drain(); d.next(); }
    println!("| partial {}",m.len());
    m.insert(999u64,123u64);
    println!("| reused {}",m.get(&999).unwrap());
}

#[test]
fn a04_group_boundaries_and_wrap() {
    for mode in 3..5u64 {
        for cap in 0..65usize {
            let mut m = HashMap::with_capacity_and_hasher(cap, Builder { mode });
            let mut seed = 0x3141592653589793u64;
            let mut observations = 0u64;
            for k in 0..65u64 { m.insert(k, k * 3); }
            for k in 0..48u64 { observations += m.remove(&k).unwrap_or(0); }
            m.reserve(12);
            for step in 0..512u64 {
                let r = random(&mut seed);
                let k = (r >> 8) % 96;
                if r & 1 == 0 { observations ^= m.insert(k, step).unwrap_or(0); }
                else { observations ^= m.remove(&k).unwrap_or(0); }
                if step % 31 == 0 { m.shrink_to_fit(); }
                observations ^= m.get(&k).copied().unwrap_or(0);
            }
            m.retain(|k, v| { *v += 1; k % 3 != 0 });
            let clone = m.clone();
            let mut sum = 0u64;
            for k in 0..96u64 { sum += clone.get(&k).copied().unwrap_or(0); }
            println!("| boundary {} {} {} {} {}", mode, cap, m.len(), observations, sum);
        }
    }
}
