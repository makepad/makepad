//! Default hasher sanity (distribution, determinism) and a rough speed comparison with std.
mod coll_common;
use rapid_std_check::collections::{BTreeMap, HashMap};
use rapid_std_check::hash::{BuildHasher, DefaultHasher, Hasher, RandomState};
use std::time::Instant;

#[test]
fn distribution() {
    let s = RandomState::new();
    // sequential, strided and high-bit-only keys must spread over low bits (bucket index)
    // and the top 7 bits (control byte)
    for (name, step) in [("seq", 1u64), ("stride4096", 4096), ("high", 1u64 << 40)] {
        let mut low = [0u32; 1024];
        let mut top = [0u32; 128];
        for i in 0..1u64 << 16 {
            let h = s.hash_one(i.wrapping_mul(step));
            low[(h & 1023) as usize] += 1;
            top[(h >> 57) as usize] += 1;
        }
        let (lmin, lmax) = (*low.iter().min().unwrap(), *low.iter().max().unwrap());
        let (tmin, tmax) = (*top.iter().min().unwrap(), *top.iter().max().unwrap());
        // expected 64 per low bucket, 512 per tag
        assert!(lmin > 25 && lmax < 120, "{name}: low bits {lmin}..{lmax}");
        assert!(tmin > 380 && tmax < 650, "{name}: tag bits {tmin}..{tmax}");
    }
    let mut a = DefaultHasher::new();
    a.write(b"hello world, this is a longer string");
    let mut b = DefaultHasher::new();
    b.write(b"hello world, this is a longer strinG");
    assert_ne!(a.finish(), b.finish());
    let mut c = DefaultHasher::new();
    c.write(b"hello world, this is a longer string");
    assert_eq!(a.finish(), c.finish());
    assert_ne!(s.hash_one("ab"), s.hash_one("ba"));
    assert_ne!(s.hash_one(("a", "bc")), s.hash_one(("ab", "c")));
}

#[test]
fn speed() {
    let n = 1_000_000u64;
    let t = Instant::now();
    let mut a: HashMap<u64, u64> = HashMap::new();
    for i in 0..n {
        a.insert(i.wrapping_mul(0x9e37_79b9), i);
    }
    let mut s = 0u64;
    for i in 0..n {
        s = s.wrapping_add(*a.get(&i.wrapping_mul(0x9e37_79b9)).unwrap());
    }
    let ours = t.elapsed();
    let t = Instant::now();
    let mut b: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for i in 0..n {
        b.insert(i.wrapping_mul(0x9e37_79b9), i);
    }
    let mut s2 = 0u64;
    for i in 0..n {
        s2 = s2.wrapping_add(*b.get(&i.wrapping_mul(0x9e37_79b9)).unwrap());
    }
    let std_t = t.elapsed();
    assert_eq!(s, s2);
    let t = Instant::now();
    let mut c: BTreeMap<u64, u64> = BTreeMap::new();
    for i in 0..n {
        c.insert(i.wrapping_mul(0x9e37_79b9) % 10_000_019, i);
    }
    let bt = t.elapsed();
    let t = Instant::now();
    let mut d: std::collections::BTreeMap<u64, u64> = std::collections::BTreeMap::new();
    for i in 0..n {
        d.insert(i.wrapping_mul(0x9e37_79b9) % 10_000_019, i);
    }
    let sbt = t.elapsed();
    println!("HashMap 1M insert+get: ours {:?} std {:?}; BTreeMap 1M insert: ours {:?} std {:?}", ours, std_t, bt, sbt);
}
