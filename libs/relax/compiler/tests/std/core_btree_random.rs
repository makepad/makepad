// BTreeMap and BTreeSet random operations, ranges, ownership and panic safety.
// Differential file: `| ` lines equal under rustc --test and makepad test.
// Replaces the former std/check btree.rs and coll_btree.rs.

use std::cell::Cell;
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};
use std::ops::Bound::{self, Excluded, Included, Unbounded};
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

fn random(s: &mut u64) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 16) as u32
}
fn bounds(s: &mut u64) -> (Bound<u32>, Bound<u32>) {
    let a = random(s) % 512;
    let b = random(s) % 512;
    let lo = a.min(b);
    let hi = a.max(b) + 1;
    let left = match random(s) % 3 {
        0 => Unbounded,
        1 => Included(lo),
        _ => Excluded(lo),
    };
    let right = match random(s) % 3 {
        0 => Unbounded,
        1 => Included(hi),
        _ => Excluded(hi),
    };
    (left, right)
}
fn observe<K: Ord + std::fmt::Debug, V: std::fmt::Debug>(a: &BTreeMap<K, V>, d: &mut D) {
    d.add(&a.len());
    d.add(&a.iter().collect::<Vec<_>>());
    d.add(&a.iter().rev().collect::<Vec<_>>());
    d.add(&a.first_key_value());
    d.add(&a.last_key_value());
}
fn caught<T>(f: impl FnOnce() -> T) -> std::thread::Result<T> {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| std::panic::set_hook(Box::new(|_| {})));
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
}
fn panic_message<T>(f: impl FnOnce() -> T) -> Option<String> {
    caught(f).err().map(|error| {
        if let Some(s) = error.downcast_ref::<String>() {
            s.clone()
        } else {
            error.downcast_ref::<&str>().unwrap().to_string()
        }
    })
}

#[test]
fn a01_randomized_300k() {
    let mut a = BTreeMap::new();
    let mut set = BTreeSet::new();
    let mut seed = 0xdead_beef_8172_9311;
    let mut d = D::new();
    for step in 0..3_000 {
        let k = random(&mut seed) % 512;
        let v = random(&mut seed) as i64;
        match random(&mut seed) % 24 {
            0..=9 => {
                d.add(&a.insert(k, v));
                d.add(&set.insert(k));
            }
            10 => {
                d.add(&a.remove(&k));
                d.add(&set.remove(&k));
            }
            11 => {
                d.add(&a.get(&k));
                d.add(&set.get(&k));
                if let Some(x) = a.get_mut(&k) {
                    *x ^= v;
                }
            }
            12 => *a.entry(k).and_modify(|x| *x += 1).or_insert(v) += 2,
            13 => d.add(&a.remove_entry(&k)),
            14 => {
                let range = bounds(&mut seed);
                d.add(&a.range(range).collect::<Vec<_>>());
                d.add(&set.range(range).collect::<Vec<_>>());
                let mut it = a.range_mut(range);
                loop {
                    let next = if random(&mut seed) & 1 == 0 { it.next_back() } else { it.next() };
                    d.add(&next);
                    match next {
                        Some((_, value)) => *value ^= 7,
                        None => break,
                    }
                }
                assert!(it.next().is_none() && it.next_back().is_none());
            }
            15 => {
                let mut it = a.iter_mut();
                loop {
                    let next = if random(&mut seed) & 1 == 0 { it.next_back() } else { it.next() };
                    d.add(&next);
                    match next {
                        Some((_, value)) => *value += 1,
                        None => break,
                    }
                }
            }
            16 => {
                d.add(&a.pop_first());
                d.add(&set.pop_first());
            }
            17 => {
                d.add(&a.pop_last());
                d.add(&set.pop_last());
            }
            18 => {
                a.retain(|k, value| {
                    *value ^= 3;
                    k % 31 != 0
                });
                set.retain(|k| k % 31 != 0);
            }
            19 => {
                let mut right = a.split_off(&k);
                observe(&right, &mut d);
                observe(&a, &mut d);
                a.insert(k, v + 1);
                a.append(&mut right);
                assert!(right.is_empty());
                let mut right = set.split_off(&k);
                d.add(&right.iter().collect::<Vec<_>>());
                set.append(&mut right);
            }
            20 => {
                observe(&a.clone(), &mut d);
                d.add(&set.clone().iter().collect::<Vec<_>>());
            }
            21 => {
                let mut it = a.clone().into_iter();
                for _ in 0..7 {
                    d.add(&it.next());
                    d.add(&it.next_back());
                }
            }
            22 => {
                if let Some(mut entry) = a.first_entry() {
                    *entry.get_mut() ^= 11;
                }
                d.add(&a.last_entry().map(|entry| entry.remove_entry()));
            }
            _ => {
                let removed: Vec<_> = a.extract_if(.., |k, value| {
                    *value += 1;
                    k % 17 == 0
                }).collect();
                d.add(&removed);
            }
        }
        if step % 101 == 0 {
            observe(&a, &mut d);
            d.add(&set.iter().collect::<Vec<_>>());
        }
    }
    observe(&a, &mut d);
    d.add(&set.iter().collect::<Vec<_>>());
    d.line("btree random");
}

#[test]
fn a02_deep_splits_and_merges() {
    let mut a = BTreeMap::new();
    let mut seed = 78123;
    let mut d = D::new();
    for k in 0..30_000 {
        a.insert(k, random(&mut seed) as i64);
    }
    observe(&a, &mut d);
    for _ in 0..9_000 {
        d.add(&a.remove_entry(&(random(&mut seed) % 30_000)));
    }
    observe(&a, &mut d);
    while !a.is_empty() {
        d.add(&a.pop_first());
        d.add(&a.pop_last());
    }
    for k in (0..10_000).rev() {
        a.insert(k, k as i64);
    }
    a.retain(|k, _| k % 7 == 0);
    observe(&a, &mut d);
    a.retain(|_, _| false);
    assert!(a.is_empty());
    d.line("btree deep");
}

#[test]
fn a03_strings_300k() {
    let mut a = BTreeMap::new();
    let mut seed = 971321;
    let mut d = D::new();
    for i in 0..3_000 {
        let key = format!("key{:03}", random(&mut seed) % 256);
        match random(&mut seed) % 8 {
            0..=3 => d.add(&a.insert(key.clone(), format!("value{i}"))),
            4 => d.add(&a.remove_entry(key.as_str())),
            5 => d.add(&a.get(key.as_str())),
            6 => {
                a.entry(key.clone()).and_modify(|v| v.push('x')).or_insert(key);
            }
            _ => d.add(&a.range::<str, _>((Included(key.as_str()), Unbounded)).collect::<Vec<_>>()),
        }
        if i % 503 == 0 {
            d.add(&a.iter().collect::<Vec<_>>());
            d.add(&a.clone().into_iter().collect::<Vec<_>>());
        }
    }
    d.line("btree strings");
}

#[derive(Debug)]
struct Value {
    id: usize,
    counts: Rc<Vec<Cell<usize>>>,
}
impl Drop for Value {
    fn drop(&mut self) {
        let count = &self.counts[self.id];
        count.set(count.get() + 1);
        assert_eq!(count.get(), 1, "double drop {}", self.id);
    }
}
fn value(id: usize, counts: &Rc<Vec<Cell<usize>>>) -> Value {
    Value { id, counts: counts.clone() }
}

#[test]
fn a04_drops_and_partial_iterators() {
    let counts = Rc::new((0..12_000).map(|_| Cell::new(0)).collect::<Vec<_>>());
    {
        let mut a = BTreeMap::new();
        for i in 0..10_000 {
            a.insert(i, value(i, &counts));
        }
        for i in 0..1000 {
            a.insert(i, value(10_000 + i, &counts));
        }
        for i in 1000..2000 {
            drop(a.remove(&i));
        }
        a.retain(|k, _| k % 3 != 0);
        {
            let mut it = a.extract_if(.., |k, _| k % 2 == 0);
            for _ in 0..17 {
                drop(it.next());
            }
        }
        let mut right = a.split_off(&7000);
        a.append(&mut right);
        for i in 0..1000 {
            a.insert(i, value(11_000 + i, &counts));
        }
        let mut it = a.into_iter();
        for _ in 0..100 {
            drop(it.next());
            drop(it.next_back());
        }
    }
    assert!(counts.iter().all(|c| c.get() == 1));
    let mut d = D::new();
    d.add(&counts.iter().map(Cell::get).collect::<Vec<_>>());
    d.line("btree partial drops");
}

struct Key {
    n: u32,
    fuel: Rc<Cell<i32>>,
    dropped: Rc<Cell<usize>>,
}
impl Drop for Key {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.n == other.n
    }
}
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let fuel = self.fuel.get();
        assert!(fuel != 0, "comparison panic");
        self.fuel.set(fuel - 1);
        self.n.cmp(&other.n)
    }
}

#[test]
fn a05_comparator_panic_owns_everything_once() {
    let mut d = D::new();
    for limit in 0..20 {
        let fuel = Rc::new(Cell::new(1_000_000));
        let dropped = Rc::new(Cell::new(0));
        let counts = Rc::new((0..201).map(|_| Cell::new(0)).collect::<Vec<_>>());
        let mut a = BTreeMap::new();
        for i in 0..200 {
            a.insert(
                Key { n: i * 2, fuel: fuel.clone(), dropped: dropped.clone() },
                value(i as usize, &counts),
            );
        }
        fuel.set(limit);
        d.add(&caught(|| {
            a.insert(Key { n: 201, fuel: fuel.clone(), dropped: dropped.clone() }, value(200, &counts))
        }).is_err());
        fuel.set(1_000_000);
        assert!(a.iter().map(|(k, _)| k.n).is_sorted());
        drop(a);
        assert_eq!(dropped.get(), 201);
        assert!(counts.iter().all(|c| c.get() == 1));
        d.add(&dropped.get());
        d.add(&counts.iter().map(Cell::get).collect::<Vec<_>>());
    }
    d.line("btree comparator panic");
}

#[repr(align(256))]
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
struct Aligned(u32);

#[test]
fn a06_alignment_zsts_and_live_mut_refs() {
    let mut a = BTreeMap::new();
    for i in 0..1000 {
        a.insert(Aligned(i), Aligned(i + 1));
    }
    let mut it = a.iter_mut();
    let (k, x) = it.next().unwrap();
    let (_, y) = it.next_back().unwrap();
    assert_eq!(k as *const Aligned as usize % 256, 0);
    assert_eq!(x as *mut Aligned as usize % 256, 0);
    x.0 += 2;
    y.0 += 3;
    assert_eq!(x.0, 3);
    assert_eq!(y.0, 1003);
    let rest: Vec<_> = it.collect();
    for (_, x) in rest {
        x.0 += 1;
    }
    let mut z = BTreeMap::new();
    z.insert((), ());
    assert_eq!(z.pop_first(), Some(((), ())));
    let mut z = BTreeMap::new();
    for i in 0..1000 {
        z.insert(i, ());
    }
    assert_eq!(z.into_values().count(), 1000);
    println!("| btree aligned and zst true");
}

#[test]
fn a07_range_panics_and_bounds() {
    let a: BTreeMap<_, _> = (0..20).map(|k| (k, k)).collect();
    let all = [Unbounded, Included(0), Excluded(0), Included(10), Excluded(10), Included(30), Excluded(30)];
    let mut d = D::new();
    for left in all {
        for right in all {
            let result = caught(|| a.range((left, right)).map(|(k, v)| (*k, *v)).collect::<Vec<_>>());
            d.add(&result.is_err());
            if let Ok(values) = result {
                d.add(&values);
            }
        }
    }
    d.line("btree bounds");
}

struct PanicDrop {
    id: usize,
    counts: Rc<Vec<Cell<usize>>>,
    panic: Rc<Cell<bool>>,
}
impl Drop for PanicDrop {
    fn drop(&mut self) {
        self.counts[self.id].set(self.counts[self.id].get() + 1);
        if self.id == 31 && self.panic.replace(false) {
            panic!("drop panic");
        }
    }
}

#[test]
fn a08_destructor_panic_finishes_drain() {
    let counts = Rc::new((0..1000).map(|_| Cell::new(0)).collect::<Vec<_>>());
    let panic = Rc::new(Cell::new(true));
    let mut a = BTreeMap::new();
    for id in 0..1000 {
        a.insert(id, PanicDrop { id, counts: counts.clone(), panic: panic.clone() });
    }
    let panicked = caught(|| a.clear()).is_err();
    assert!(panicked);
    assert!(a.is_empty());
    assert!(counts.iter().all(|x| x.get() == 1));
    let mut d = D::new();
    d.add(&panicked);
    d.add(&a.is_empty());
    d.add(&counts.iter().map(Cell::get).collect::<Vec<_>>());
    d.line("btree destructor panic");
}

struct CloneValue {
    live: Rc<Cell<usize>>,
    fuel: Rc<Cell<usize>>,
}
impl Clone for CloneValue {
    fn clone(&self) -> Self {
        let fuel = self.fuel.get();
        assert!(fuel != 0, "clone panic");
        self.fuel.set(fuel - 1);
        self.live.set(self.live.get() + 1);
        Self { live: self.live.clone(), fuel: self.fuel.clone() }
    }
}
impl Drop for CloneValue {
    fn drop(&mut self) {
        self.live.set(self.live.get() - 1);
    }
}

#[test]
fn a09_clone_panic_and_entry_removal() {
    let live = Rc::new(Cell::new(1000));
    let fuel = Rc::new(Cell::new(73));
    let mut a = BTreeMap::new();
    for k in 0..1000 {
        a.insert(k, CloneValue { live: live.clone(), fuel: fuel.clone() });
    }
    let panicked = caught(|| a.clone()).is_err();
    assert!(panicked);
    assert_eq!(live.get(), 1000);
    if let Entry::Occupied(entry) = a.entry(500) {
        assert_eq!(entry.remove_entry().0, 500);
    } else {
        panic!("missing occupied entry");
    }
    assert_eq!(live.get(), 999);
    fuel.set(1000);
    let b = a.clone();
    assert_eq!(live.get(), 1998);
    drop(b);
    a.clear();
    assert_eq!(live.get(), 0);
    println!("| btree clone panic {panicked} live {}", live.get());
}

#[test]
fn a10_range_validation_only_with_root() {
    for state in 0..3 {
        let mut a: BTreeMap<u32, u32> = BTreeMap::new();
        if state >= 1 {
            a.insert(1, 1);
            a.remove(&1);
        }
        if state == 2 {
            a.clear();
        }
        let equal = caught(|| a.range((Excluded(3), Excluded(3))).count()).is_err();
        let reversed = caught(|| a.range((Included(5), Included(3))).count()).is_err();
        println!("| btree root state {state} equal {equal} reversed {reversed}");
    }
}

fn bound(rng: &mut Rng, range: u64) -> Bound<u32> {
    match rng.below(3) {
        0 => Included(rng.below(range) as u32),
        1 => Excluded(rng.below(range) as u32),
        _ => Unbounded,
    }
}

#[test]
fn a11_random_ops() {
    for seed in 1..6u64 {
        let mut rng = Rng(seed.wrapping_mul(0xdead_beef_1234_5677));
        let range = [20u64, 500, 20000][seed as usize % 3];
        let mut a: BTreeMap<u32, u64> = BTreeMap::new();
        let mut d = D::new();
        for step in 0..4_000u64 {
            let k = rng.below(range) as u32;
            let v = rng.next();
            match rng.below(14) {
                0..=3 => d.add(&a.insert(k, v)),
                4 | 5 => d.add(&a.remove(&k)),
                6 => d.add(&a.get(&k)),
                7 => *a.entry(k).or_insert(v) += 1,
                8 => d.add(&a.pop_first()),
                9 => d.add(&a.pop_last()),
                10 => {
                    let bounds = (bound(&mut rng, range), bound(&mut rng, range));
                    let valid = match bounds {
                        (Included(s) | Excluded(s), Included(e) | Excluded(e)) if s > e => false,
                        (Excluded(s), Excluded(e)) if s == e => false,
                        _ => true,
                    };
                    if valid {
                        d.add(&a.range(bounds).map(|(k, v)| (*k, *v)).collect::<Vec<_>>());
                        d.add(&a.range(bounds).rev().map(|(k, _)| *k).collect::<Vec<_>>());
                        let mut it = a.range(bounds);
                        loop {
                            let next = if rng.below(2) == 0 { it.next() } else { it.next_back() };
                            d.add(&next);
                            if next.is_none() {
                                break;
                            }
                        }
                        for (_, v) in a.range_mut(bounds) {
                            *v ^= 1;
                        }
                    }
                }
                11 => {
                    if step % 50 == 0 {
                        let m = rng.below(3);
                        a.retain(|k, _| *k as u64 % 3 != m);
                    }
                }
                12 => {
                    if step % 70 == 0 {
                        let mut tail = a.split_off(&k);
                        observe(&tail, &mut d);
                        observe(&a, &mut d);
                        a.append(&mut tail);
                        assert!(tail.is_empty());
                    }
                }
                _ => d.add(&a.contains_key(&k)),
            }
            if step % 64 == 0 {
                observe(&a, &mut d);
            }
            d.add(&a.len());
        }
        observe(&a, &mut d);
        let mut it = a.iter();
        loop {
            let next = if rng.below(2) == 0 { it.next() } else { it.next_back() };
            d.add(&next);
            d.add(&it.len());
            if next.is_none() {
                break;
            }
        }
        for v in a.values_mut() {
            *v = v.wrapping_add(5);
        }
        let c = a.clone();
        assert!(c == a);
        observe(&c, &mut d);
        d.add(&a.keys().rev().copied().collect::<Vec<_>>());
        d.add(&a.into_iter().rev().collect::<Vec<_>>());
        d.line(&format!("btree coll random seed {seed}"));
    }
}

#[test]
fn a12_panics_and_debug() {
    let a: BTreeMap<_, _> = (0..5u32).map(|i| (i, i * 10)).collect();
    assert_eq!(format!("{:?}", a), "{0: 0, 1: 10, 2: 20, 3: 30, 4: 40}");
    assert_eq!(format!("{:#?}", BTreeMap::from([(1u32, 2u32)])), "{\n    1: 2,\n}");
    let reversed = panic_message(|| a.range(5..2).count());
    assert_eq!(reversed.as_deref(), Some("range start is greater than range end in BTreeMap"));
    let excluded = panic_message(|| a.range((Excluded(3), Excluded(3))).count());
    assert_eq!(excluded.as_deref(), Some("range start and end are equal and excluded in BTreeMap"));
    let empty = panic_message(|| {
        let empty: BTreeMap<u32, u32> = BTreeMap::new();
        empty.range(5..2).count()
    });
    assert_eq!(empty, None);
    let missing = panic_message(|| a[&9]);
    assert_eq!(missing.as_deref(), Some("no entry found for key"));
    let strings = BTreeMap::from([("b".to_string(), 1), ("a".to_string(), 2)]);
    assert_eq!(strings.get("a"), Some(&2));
    assert_eq!(strings.range::<str, _>((Included("a"), Excluded("b"))).count(), 1);
    println!(
        "| btree panic cases {} {} {} {}",
        reversed.is_some(),
        excluded.is_some(),
        empty.is_some(),
        missing.is_some(),
    );
}

#[test]
fn a13_sets() {
    let mut rng = Rng(3);
    let mut d = D::new();
    for _ in 0..40 {
        let xa: Vec<_> = (0..rng.below(300)).map(|_| rng.below(200) as u32).collect();
        let xb: Vec<_> = (0..rng.below(300)).map(|_| rng.below(200) as u32).collect();
        let a: BTreeSet<_> = xa.iter().copied().collect();
        let b: BTreeSet<_> = xb.iter().copied().collect();
        d.add(&a.iter().collect::<Vec<_>>());
        d.add(&a.union(&b).collect::<Vec<_>>());
        d.add(&a.intersection(&b).collect::<Vec<_>>());
        d.add(&a.difference(&b).collect::<Vec<_>>());
        d.add(&a.symmetric_difference(&b).collect::<Vec<_>>());
        d.add(&a.is_subset(&b));
        d.add(&a.is_disjoint(&b));
        d.add(&(&a | &b).iter().collect::<Vec<_>>());
        d.add(&(&a - &b).iter().collect::<Vec<_>>());
        d.add(&a.first());
        d.add(&a.last());
        d.add(&a.range(10..50).collect::<Vec<_>>());
        d.add(&a.range(..=70).rev().collect::<Vec<_>>());
        let mut c = a.clone();
        for x in &xb {
            d.add(&c.insert(*x));
            d.add(&c.remove(&(x / 2)));
        }
        d.add(&c.iter().collect::<Vec<_>>());
        d.add(&c.pop_first());
        d.add(&c.pop_last());
        let right = c.split_off(&100);
        d.add(&right.iter().collect::<Vec<_>>());
        d.add(&c.iter().collect::<Vec<_>>());
        d.add(&(a == b));
        d.add(&a.cmp(&b));
    }
    assert_eq!(format!("{:?}", BTreeSet::from([3u32, 1, 2])), "{1, 2, 3}");
    d.line("btree sets");
}

struct Counted {
    drops: Rc<Cell<usize>>,
}
impl Drop for Counted {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

#[test]
fn a14_drops() {
    let drops = Rc::new(Cell::new(0usize));
    let mut made = 0;
    {
        let mut a = BTreeMap::new();
        let mut rng = Rng(11);
        for _ in 0..300 {
            let k = rng.below(400) as u32;
            made += 1;
            a.insert(k, Counted { drops: drops.clone() });
            if rng.below(4) == 0 {
                a.remove(&(rng.below(400) as u32));
            }
        }
        a.retain(|k, _| k % 2 == 0);
        let _ = a.pop_first();
        let extracted: Vec<_> = a.extract_if(.., |k, _| k % 3 == 0).map(|(k, _)| k).collect();
        assert!(extracted.iter().all(|k| k % 3 == 0));
        let mut it = a.into_iter();
        let _ = it.next_back();
    }
    assert_eq!(drops.get(), made);
    println!("| btree drops {}", drops.get());
}

