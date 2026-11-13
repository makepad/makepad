// HashMap and HashSet random operations, collision families, ownership and hash distribution.
// Differential file: `| ` lines equal under rustc --test and makepad test.
// Raw control bytes and allocation internals have no public API; timing checks are omitted.

use std::cell::Cell;
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hash, Hasher, RandomState};
use std::rc::Rc;

struct D(u64, u32);
impl D {
    fn new() -> D {
        D(0xcbf29ce484222325, 0)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn add<T: std::fmt::Debug + ?Sized>(&mut self, value: &T) {
        use std::fmt::Write;
        write!(self, "{:?};", value).unwrap();
        self.1 += 1;
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}
impl std::fmt::Write for D {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

struct Counted {
    v: u32,
    drops: Rc<Cell<usize>>,
}
impl Drop for Counted {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

fn sorted<T: Ord>(values: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut values: Vec<_> = values.into_iter().collect();
    values.sort_unstable();
    values
}
fn caught<T>(f: impl FnOnce() -> T) -> std::thread::Result<T> {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| std::panic::set_hook(Box::new(|_| {})));
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
}

#[test]
fn a01_random_ops_u32() {
    for seed in 1..6u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut a: HashMap<u32, u64> = HashMap::new();
        let range = [16u64, 200, 5000][seed as usize % 3];
        let mut d = D::new();
        for step in 0..6_000u64 {
            let k = rng.below(range) as u32;
            let v = rng.next();
            match rng.below(12) {
                0..=2 => d.add(&a.insert(k, v)),
                3 | 4 => d.add(&a.remove(&k)),
                5 => d.add(&a.get(&k)),
                6 => d.add(&a.contains_key(&k)),
                7 => *a.entry(k).or_insert(v) += 1,
                8 => {
                    a.entry(k).and_modify(|x| *x ^= v).or_default();
                }
                9 => {
                    if let Some(x) = a.get_mut(&k) {
                        *x = x.wrapping_add(v);
                    }
                }
                10 => d.add(&a.remove_entry(&k)),
                _ => {
                    if step % 97 == 0 {
                        let m = rng.below(4);
                        a.retain(|k, v| (*k as u64).wrapping_add(*v) % 4 != m);
                    }
                }
            }
            d.add(&a.len());
            assert!(a.capacity() >= a.len());
        }
        d.add(&sorted(a.iter().map(|(k, v)| (*k, *v))));
        d.add(&sorted(a.keys().copied()));
        d.add(&a.values().fold(0u64, |sum, v| sum.wrapping_add(*v)));
        for v in a.values_mut() {
            *v = v.wrapping_mul(3);
        }
        for (_, v) in a.iter_mut() {
            *v = v.wrapping_add(1);
        }
        let c = a.clone();
        assert!(c == a);
        d.add(&sorted(c));
        d.add(&sorted(a.drain()));
        assert!(a.is_empty());
        a.insert(1, 2);
        assert_eq!(a.get(&1), Some(&2));
        a.shrink_to_fit();
        assert_eq!(a.get(&1), Some(&2));
        d.line(&format!("hash random seed {seed}"));
    }
}

#[test]
fn a02_string_keys_and_borrow() {
    let mut rng = Rng(77);
    let mut a = HashMap::new();
    let mut d = D::new();
    for i in 0..2_000usize {
        let k = format!("key{}", rng.below(3000));
        match rng.below(4) {
            0 | 1 => d.add(&a.insert(k, i)),
            2 => d.add(&a.remove(k.as_str())),
            _ => d.add(&a.get(k.as_str())),
        }
    }
    d.add(&a.len());
    for k in sorted(a.keys()) {
        d.add(&a[k.as_str()]);
        d.add(&a.get_key_value(k.as_str()));
    }
    let result = caught(|| {
        let a: HashMap<u32, u32> = HashMap::new();
        a[&5]
    });
    d.add(&result.is_err());
    let error = result.unwrap_err();
    let message = error.downcast_ref::<String>().map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied());
    assert_eq!(message, Some("no entry found for key"));
    d.line("hash strings");
}

#[test]
fn a03_collect_extend_eq() {
    let a: HashMap<_, _> = (0..1000u32).map(|i| (i % 300, i)).collect();
    let mut d = D::new();
    d.add(&a.len());
    for k in sorted(a.keys()) {
        d.add(&a.get(k));
    }
    let mut c = HashMap::with_capacity(10);
    c.extend(a.iter().map(|(k, v)| (*k, *v)));
    assert!(c == a);
    c.insert(5000, 1);
    assert!(c != a);
    let mut extended = HashMap::new();
    extended.extend(a.iter());
    assert!(extended == a);
    let e = HashMap::from([(1u32, 2u32), (3, 4)]);
    assert_eq!(e.len(), 2);
    assert_eq!(e[&3], 4);
    d.line("hash collect extend");
}

#[test]
fn a04_entry_api() {
    let mut a: HashMap<&'static str, Vec<u32>> = HashMap::new();
    a.entry("x").or_insert_with(Vec::new).push(1);
    a.entry("x").or_default().push(2);
    a.entry("y").or_insert_with_key(|k| vec![k.len() as u32]);
    assert_eq!(a["x"], vec![1, 2]);
    assert_eq!(a["y"], vec![1]);
    match a.entry("x") {
        Entry::Occupied(mut entry) => {
            assert_eq!(*entry.key(), "x");
            entry.get_mut().push(3);
            assert_eq!(entry.insert(vec![9]), vec![1, 2, 3]);
            assert_eq!(entry.remove(), vec![9]);
        }
        Entry::Vacant(_) => panic!("missing occupied entry"),
    }
    assert!(!a.contains_key("x"));
    match a.entry("z") {
        Entry::Vacant(entry) => {
            assert_eq!(*entry.key(), "z");
            entry.insert(vec![7]);
        }
        Entry::Occupied(_) => panic!("unexpected occupied entry"),
    }
    assert_eq!(a["z"], vec![7]);
    assert_eq!(*a.entry("q").key(), "q");
    println!("| hash entries {:?}", sorted(a));
}

#[test]
fn a05_drops_exactly_once() {
    let drops = Rc::new(Cell::new(0usize));
    let mut made = 0;
    {
        let mut a = HashMap::new();
        let mut rng = Rng(5);
        for _ in 0..500 {
            let k = rng.below(700) as u32;
            made += 1;
            a.insert(k, Counted { v: k, drops: drops.clone() });
            if rng.below(3) == 0 {
                a.remove(&(rng.below(700) as u32));
            }
        }
        a.retain(|k, v| *k % 3 != 0 && v.v == *k);
        let ids: HashMap<_, _> = a.iter().map(|(k, v)| (*k, v.v)).collect();
        let mut ids = ids.into_iter();
        let _ = ids.next();
        let mut drain = a.drain();
        let _ = drain.next();
        drop(drain);
        assert!(a.is_empty());
        for k in 0..100u32 {
            made += 1;
            a.insert(k, Counted { v: k, drops: drops.clone() });
        }
        let mut it = a.into_iter();
        let _ = it.next();
    }
    assert_eq!(drops.get(), made);
    println!("| hash drops {}", drops.get());
}

#[test]
fn a06_debug_format() {
    let a = HashMap::from([(7u32, 8u32)]);
    assert_eq!(format!("{:?}", a), "{7: 8}");
    assert_eq!(format!("{:#?}", a), "{\n    7: 8,\n}");
    let empty: HashMap<u32, u32> = HashMap::new();
    assert_eq!(format!("{:?}", empty), "{}");
    let set = HashSet::from([3u32]);
    assert_eq!(format!("{:?}", set), "{3}");
    assert_eq!(format!("{:?}", set.iter()), "[3]");
    assert_eq!(format!("{:?}", a.iter()), "[(7, 8)]");
    println!("| hash debug {:?} {:?}", a, set);
}

#[test]
fn a07_set_ops() {
    let mut rng = Rng(99);
    let mut d = D::new();
    for _ in 0..50 {
        let xa: Vec<_> = (0..rng.below(200)).map(|_| rng.below(150) as u32).collect();
        let xb: Vec<_> = (0..rng.below(200)).map(|_| rng.below(150) as u32).collect();
        let a: HashSet<_> = xa.iter().copied().collect();
        let b: HashSet<_> = xb.iter().copied().collect();
        d.add(&sorted(a.union(&b)));
        d.add(&sorted(a.intersection(&b)));
        d.add(&sorted(a.difference(&b)));
        d.add(&sorted(a.symmetric_difference(&b)));
        d.add(&a.is_subset(&b));
        d.add(&a.is_superset(&b));
        d.add(&a.is_disjoint(&b));
        d.add(&sorted(&a | &b));
        d.add(&sorted(&a & &b));
        d.add(&sorted(&a - &b));
        d.add(&sorted(&a ^ &b));
        let mut c = a.clone();
        for x in &xb {
            d.add(&c.insert(*x));
            d.add(&c.remove(&(x + 1)));
            d.add(&c.contains(&(x + 2)));
            d.add(&c.take(&(x + 3)));
        }
        d.add(&c.len());
        c.retain(|x| x % 2 == 0);
        d.add(&sorted(c.iter()));
        // The combined set entry API is unstable; use its public insert/get equivalent.
        c.insert(1000);
        assert_eq!(*c.get(&1000).unwrap(), 1000);
        assert!(c.contains(&1000));
        assert_eq!(c.replace(1000), Some(1000));
    }
    d.line("hash set ops");
}

#[derive(Clone)]
struct Builder(u8, u64);
struct Cheap {
    mode: u8,
    salt: u64,
    value: u64,
}
impl BuildHasher for Builder {
    type Hasher = Cheap;
    fn build_hasher(&self) -> Cheap {
        Cheap { mode: self.0, salt: self.1, value: 0 }
    }
}
impl Hasher for Cheap {
    fn finish(&self) -> u64 {
        match self.mode {
            0 => self.value.wrapping_mul(0x9e3779b97f4a7c15),
            1 => 0,
            2 => (self.value & 1) << 57,
            3 => self.salt,
            4 => 63 | (if self.salt != 0 { (self.value & 1) << 57 } else { 0 }),
            _ => self.value,
        }
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.value = self.value.wrapping_mul(257).wrapping_add(*b as u64);
        }
    }
    fn write_u64(&mut self, value: u64) {
        self.value = value;
    }
}
fn random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
fn contents(a: &HashMap<u64, u64, Builder>, d: &mut D) {
    d.add(&a.len());
    assert!(a.capacity() >= a.len());
    d.add(&sorted(a.iter().map(|(k, v)| (*k, *v))));
}

#[test]
fn a08_randomized_maps_and_sets_each_hasher() {
    for mode in 0..3 {
        let mut a = HashMap::with_hasher(Builder(mode, 0));
        let mut set = HashSet::with_hasher(Builder(mode, 0));
        let mut seed = 0x4321fedc9876abcd;
        let mut d = D::new();
        for step in 0..2_000 {
            let r = random(&mut seed);
            let k = (r >> 8) % 512;
            let v = r >> 32;
            match r % 100 {
                0..=39 => d.add(&a.insert(k, v)),
                40..=59 => d.add(&a.remove(&k)),
                60..=79 => d.add(&a.get(&k)),
                80..=94 => *a.entry(k).or_insert(v) += 1,
                95 => a.reserve(32),
                96 => a.shrink_to_fit(),
                97 => a.retain(|key, value| {
                    *value ^= 7;
                    key % 19 != k % 19
                }),
                98 => contents(&a.clone(), &mut d),
                _ => {
                    if step % 25 == 0 {
                        d.add(&sorted(a.drain()));
                    }
                }
            }
            if r & 1 == 0 {
                d.add(&set.insert(k));
            } else {
                d.add(&set.remove(&k));
            }
            d.add(&set.contains(&k));
            assert!(a.capacity() >= a.len());
            assert!(set.capacity() >= set.len());
            if step % 257 == 0 {
                contents(&a, &mut d);
            }
        }
        contents(&a, &mut d);
        d.add(&sorted(set.iter()));
        d.line(&format!("hash collision mode {mode}"));
    }
}

#[test]
fn a09_reserve_keeps_bucket_pointer_and_rebuilds_in_place() {
    let mut d = D::new();
    for cap in [1, 3, 4, 7, 8, 15, 28, 100, 1000] {
        let mut a = HashMap::with_capacity_and_hasher(cap, Builder(5, 0));
        a.insert(0u64, 0u64);
        a.reserve(cap);
        let pointer = a.get(&0).unwrap() as *const u64;
        for i in 1..=cap as u64 {
            a.insert(i, i);
            let same = std::ptr::eq(a.get(&0).unwrap(), pointer);
            assert!(same);
            d.add(&same);
            assert!(a.capacity() >= a.len());
        }
    }
    let mut a = HashMap::with_capacity_and_hasher(112, Builder(1, 0));
    for i in 0..112u64 {
        a.insert(i, i);
    }
    for i in 0..100 {
        assert_eq!(a.remove(&i), Some(i));
    }
    let additional = a.capacity() - a.len() + 1;
    a.reserve(additional);
    for i in 100..112 {
        assert!(a.contains_key(&i));
    }
    contents(&a, &mut d);
    d.line("hash reserve");
}

#[test]
fn a10_group_wraparound_small_tables_and_alignment() {
    #[derive(Clone)]
    #[repr(align(128))]
    struct Aligned(u64);
    let mut d = D::new();
    for cap in [1, 3, 4, 7, 8, 14, 28] {
        for start in 0..64u64 {
            let mut a = HashMap::with_capacity_and_hasher(cap, Builder(3, start));
            let limit = a.capacity();
            for i in 0..limit {
                a.insert(i as u64, Aligned(i as u64));
            }
            for i in 0..limit {
                let value = a.get(&(i as u64)).unwrap();
                assert_eq!(value as *const Aligned as usize % 128, 0);
            }
            assert!(!a.contains_key(&1000));
            let clone = a.clone();
            for i in 0..limit {
                assert_eq!(a.remove(&(i as u64)).unwrap().0, i as u64);
                assert!(clone.contains_key(&(i as u64)));
            }
            d.add(&limit);
            d.add(&sorted(clone.iter().map(|(k, v)| (*k, v.0))));
        }
    }
    let mut zst = HashMap::new();
    for i in 0..100 {
        zst.insert(i, ());
    }
    assert_eq!(zst.len(), 100);
    zst.clear();
    zst.shrink_to(0);
    assert_eq!(zst.capacity(), 0);
    d.add(&zst.len());
    d.add(&zst.capacity());
    d.line("hash wrap alignment zst");
}

struct HashPolicy {
    mode: Cell<u8>,
    calls: Cell<usize>,
}
struct PanicKey {
    id: usize,
    policy: Rc<HashPolicy>,
}
impl Clone for PanicKey {
    fn clone(&self) -> Self {
        Self { id: self.id, policy: self.policy.clone() }
    }
}
impl PartialEq for PanicKey {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for PanicKey {}
impl Hash for PanicKey {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        match self.policy.mode.get() {
            1 if self.id == 50 => panic!("hash"),
            2 => {
                let calls = self.policy.calls.get() + 1;
                self.policy.calls.set(calls);
                assert!(calls != 4, "rehash");
            }
            _ => {}
        }
        hasher.write_u64(0);
    }
}
struct PanicValue {
    id: usize,
    drops: Rc<Cell<usize>>,
    clones: Rc<Cell<usize>>,
}
impl Clone for PanicValue {
    fn clone(&self) -> Self {
        assert!(self.id != 50, "clone");
        self.clones.set(self.clones.get() + 1);
        Self { id: self.id, drops: self.drops.clone(), clones: self.clones.clone() }
    }
}
impl Drop for PanicValue {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

#[test]
fn a11_panic_during_resize_clone_and_rehash_drops_once() {
    let policy = Rc::new(HashPolicy { mode: Cell::new(0), calls: Cell::new(0) });
    let drops = Rc::new(Cell::new(0));
    let clones = Rc::new(Cell::new(0));
    let key = |id| PanicKey { id, policy: policy.clone() };
    let mut a = HashMap::with_capacity_and_hasher(112, Builder(1, 0));
    for id in 0..112 {
        a.insert(key(id), PanicValue { id, drops: drops.clone(), clones: clones.clone() });
    }
    policy.mode.set(1);
    let resize_panic = caught(|| a.reserve(1000)).is_err();
    assert!(resize_panic);
    policy.mode.set(0);
    assert_eq!(drops.get(), 0);
    assert_eq!(a.len(), 112);
    for id in 0..112 {
        assert!(a.contains_key(&key(id)));
    }
    let clone_panic = caught(|| a.clone()).is_err();
    assert!(clone_panic);
    let clone_drops = drops.get();
    assert!(clone_drops > 0 && clone_drops < 112);
    assert_eq!(clone_drops, clones.get());
    for id in 0..100 {
        drop(a.remove(&key(id)));
    }
    let additional = a.capacity() - a.len() + 1;
    policy.mode.set(2);
    policy.calls.set(0);
    let rehash_panic = caught(|| a.reserve(additional)).is_err();
    assert!(rehash_panic);
    policy.mode.set(0);
    let mut d = D::new();
    d.add(&resize_panic);
    d.add(&clone_panic);
    d.add(&rehash_panic);
    d.add(&a.len());
    d.add(&a.iter().all(|(k, v)| k.id == v.id && (100..112).contains(&k.id)));
    drop(a);
    assert_eq!(drops.get(), 112 + clone_drops);
    // Clone traversal order is private: observe the original values' final drop count.
    d.add(&(drops.get() - clone_drops));
    d.line("hash panic ownership");
}

#[test]
fn a12_raw_iteration_mutation_and_drain() {
    let mut a = HashMap::with_capacity_and_hasher(100, Builder(5, 0));
    for i in 0..100u64 {
        a.insert(i, i);
    }
    a.retain(|_, v| *v % 3 == 0);
    for v in a.values_mut() {
        *v += 1;
    }
    let drained = sorted(a.drain());
    for (_, value) in &drained {
        assert_eq!(value % 3, 1);
    }
    assert_eq!(drained.len(), 34);
    assert_eq!(a.len(), 0);
    assert_eq!(a.capacity(), 112);
    println!("| hash drain {:?} len {} capacity {}", drained, a.len(), a.capacity());
}

#[test]
fn a13_size_classes_mirrors_and_wrap_deletion() {
    for requested in 0..=56 {
        for one_bit in [false, true] {
            let mut a = HashMap::with_capacity_and_hasher(requested, Builder(4, one_bit as u64));
            let mut seed = 0x3141592653589793;
            let mut d = D::new();
            for step in 0..108u64 {
                let k = if step < 64 { step } else { random(&mut seed) % 96 };
                let found = a.contains_key(&k);
                if step >= 64 && random(&mut seed) & 1 == 0 {
                    d.add(&a.remove(&k));
                } else if !found {
                    a.insert(k, k);
                }
                if step % 31 == 0 {
                    a.reserve(3);
                    a.shrink_to(0);
                }
                d.add(&a.len());
                for key in 0..96u64 {
                    d.add(&a.contains_key(&key));
                }
            }
            d.line(&format!("hash size {requested} high bit {one_bit}"));
        }
    }
}

#[test]
fn a14_distribution() {
    // DefaultHasher's fixed keys: the same hashes every run (RandomState's keys would make the
    // bounds below hold only almost always), and each family's hashes digested
    let state = BuildHasherDefault::<DefaultHasher>::default();
    for (name, step) in [("seq", 1u64), ("stride4096", 4096), ("high", 1u64 << 40)] {
        let mut low = [0u32; 1024];
        let mut top = [0u32; 128];
        let mut d = D::new();
        for i in 0..1u64 << 16 {
            let hash = state.hash_one(i.wrapping_mul(step));
            low[(hash & 1023) as usize] += 1;
            top[(hash >> 57) as usize] += 1;
            d.bytes(&hash.to_le_bytes());
        }
        let (lmin, lmax) = (*low.iter().min().unwrap(), *low.iter().max().unwrap());
        let (tmin, tmax) = (*top.iter().min().unwrap(), *top.iter().max().unwrap());
        assert!(lmin > 25 && lmax < 120, "{name}: low bits {lmin}..{lmax}");
        assert!(tmin > 380 && tmax < 650, "{name}: tag bits {tmin}..{tmax}");
        println!("| hash distribution {name} low {lmin}..{lmax} top {tmin}..{tmax} {:016x}", d.0);
    }
    let state = RandomState::new();
    let mut a = DefaultHasher::new();
    a.write(b"hello world, this is a longer string");
    let mut b = DefaultHasher::new();
    b.write(b"hello world, this is a longer strinG");
    assert_ne!(a.finish(), b.finish());
    let mut c = DefaultHasher::new();
    c.write(b"hello world, this is a longer string");
    assert_eq!(a.finish(), c.finish());
    assert_ne!(state.hash_one("ab"), state.hash_one("ba"));
    assert_ne!(state.hash_one(("a", "bc")), state.hash_one(("ab", "c")));
    println!("| hash determinism and distinct inputs true");
}

