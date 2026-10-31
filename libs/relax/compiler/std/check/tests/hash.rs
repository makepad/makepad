use relax_std_check::collections::{HashMap, HashSet};
use std::collections::{HashMap as RealMap, HashSet as RealSet};
use std::hash::{BuildHasher, Hasher};
use std::{marker, mem, ptr, slice};
use relax_std_check::panicking;
mod heap {
    pub use std::alloc::{dealloc, Layout};
    pub fn alloc_or_abort(layout: Layout) -> *mut u8 {
        let p = unsafe { std::alloc::alloc(layout) };
        if p.is_null() { std::alloc::handle_alloc_error(layout); }
        p
    }
}
#[path = "../../core/collections/raw_table.rs"]
mod raw_table;
use raw_table::RawTable;

#[derive(Clone)]
struct Builder(u8);
struct Cheap { mode: u8, value: u64 }
impl BuildHasher for Builder {
    type Hasher = Cheap;
    fn build_hasher(&self) -> Cheap { Cheap { mode: self.0, value: 0 } }
}
impl Hasher for Cheap {
    fn finish(&self) -> u64 {
        match self.mode { 0 => self.value.wrapping_mul(0x9e3779b97f4a7c15), 1 => 0, 2 => (self.value & 1) << 57, 3 => self.value, _ => self.value.wrapping_mul(0x517cc1b727220a95) }
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
fn contents(m: &HashMap<u64, u64, Builder>, real: &RealMap<u64, u64, Builder>) {
    assert_eq!(m.len(), real.len());
    assert!(m.capacity() >= m.len());
    let mut a: Vec<_> = m.iter().map(|(k,v)| (*k,*v)).collect();
    let mut b: Vec<_> = real.iter().map(|(k,v)| (*k,*v)).collect();
    a.sort_unstable(); b.sort_unstable(); assert_eq!(a,b);
}
#[test]
fn randomized_maps_and_sets_200k_each_hasher() {
    for mode in 0..3 {
        let mut m = HashMap::with_hasher(Builder(mode));
        let mut real = RealMap::with_hasher(Builder(mode));
        let mut set = HashSet::with_hasher(Builder(mode));
        let mut real_set = RealSet::with_hasher(Builder(mode));
        let mut seed = 0x4321fedc9876abcd;
        for step in 0..200_000 {
            let r = random(&mut seed);
            let k = (r >> 8) % 512;
            let v = r >> 32;
            match r % 100 {
                0..=39 => { assert_eq!(m.insert(k,v),real.insert(k,v)); }
                40..=59 => { assert_eq!(m.remove(&k),real.remove(&k)); }
                60..=79 => { assert_eq!(m.get(&k),real.get(&k)); }
                80..=94 => {
                    *m.entry(k).or_insert(v) += 1;
                    *real.entry(k).or_insert(v) += 1;
                }
                95 => { m.reserve(32); real.reserve(32); }
                96 => { m.shrink_to_fit(); real.shrink_to_fit(); }
                97 => {
                    m.retain(|key,val| { *val ^= 7; key % 19 != k % 19 });
                    real.retain(|key,val| { *val ^= 7; key % 19 != k % 19 });
                }
                98 => { contents(&m.clone(), &real.clone()); }
                _ => {
                    if step % 251 == 0 {
                        let mut a: Vec<_> = m.drain().collect();
                        let mut b: Vec<_> = real.drain().collect();
                        a.sort_unstable(); b.sort_unstable(); assert_eq!(a,b);
                    }
                }
            }
            if r & 1 == 0 { assert_eq!(set.insert(k), real_set.insert(k)); }
            else { assert_eq!(set.remove(&k), real_set.remove(&k)); }
            assert_eq!(set.contains(&k),real_set.contains(&k));
            assert!(m.capacity() >= m.len());
            assert!(set.capacity() >= set.len());
            if step % 257 == 0 { contents(&m,&real); }
        }
        contents(&m,&real);
        let mut a: Vec<_> = set.iter().copied().collect();
        let mut b: Vec<_> = real_set.iter().copied().collect();
        a.sort_unstable(); b.sort_unstable(); assert_eq!(a,b);
    }
}

#[test]
fn reserve_keeps_bucket_pointer_and_rebuilds_in_place() {
    for cap in [1, 3, 4, 7, 8, 15, 28, 100, 1000] {
        let mut t = RawTable::with_capacity(cap);
        let hash = |v: &u64| *v;
        t.insert(0, 0, &hash);
        t.reserve(cap, &hash);
        let allocation = t.ctrl().as_ptr();
        let bucket = t.slot_ptr(t.find(0, |v| *v == 0).unwrap());
        for i in 1..=cap as u64 {
            t.insert(i,i,&hash);
            assert_eq!(t.ctrl().as_ptr(),allocation);
            assert_eq!(t.slot_ptr(t.find(0,|v| *v == 0).unwrap()),bucket);
            assert!(t.capacity() >= t.len());
        }
    }
    let mut t = RawTable::with_capacity(112);
    for i in 0..112 { t.insert(0,i,&|_| 0); }
    let allocation = t.ctrl().as_ptr();
    for i in 0..100 {
        let slot = t.find(0,|v| *v == i).unwrap();
        assert_eq!(t.remove(slot),i);
    }
    let additional = t.capacity() - t.len() + 1;
    assert!(t.len() + additional <= 56, "must trigger in-place branch");
    t.reserve(additional,&|_| 0);
    assert_eq!(t.ctrl().as_ptr(),allocation);
    assert_eq!(t.capacity(),112);
    for i in 100..112 { assert!(t.find(0,|v| *v == i).is_some()); }
}

#[test]
fn group_wraparound_small_tables_and_alignment() {
    #[derive(Clone)]
    #[repr(align(128))]
    struct Aligned(u64);
    for cap in [1,3,4,7,8,14,28] {
        for start in 0..64u64 {
            let mut t = RawTable::with_capacity(cap);
            let limit = t.capacity();
            for i in 0..limit { t.insert(start,Aligned(i as u64),&|_| start); }
            for i in 0..limit {
                let slot = t.find(start,|v| v.0 == i as u64).unwrap();
                assert_eq!(t.slot_ptr(slot) as usize % 128,0);
            }
            assert!(t.find(start,|v| v.0 == 1000).is_none());
            let clone = t.clone_table();
            for i in 0..limit {
                let slot = t.find(start,|v| v.0 == i as u64).unwrap();
                assert_eq!(t.remove(slot).0,i as u64);
                assert!(clone.find(start,|v| v.0 == i as u64).is_some());
            }
        }
    }
    let mut zst = RawTable::new();
    for _ in 0..100 { zst.insert(0,(),&|_| 0); }
    assert_eq!(zst.len(),100);
    zst.clear(); zst.shrink_to(0,&|_| 0);
    assert_eq!(zst.buckets(),0);
}

#[test]
fn panic_during_resize_clone_and_rehash_drops_once() {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    struct Value { id: usize, drops: Rc<Cell<usize>>, panic_clone: bool }
    impl Drop for Value { fn drop(&mut self) { self.drops.set(self.drops.get()+1); } }
    impl Clone for Value {
        fn clone(&self) -> Self {
            if self.panic_clone && self.id == 50 { panic!("clone"); }
            Self { id: self.id, drops: self.drops.clone(), panic_clone: self.panic_clone }
        }
    }
    let drops = Rc::new(Cell::new(0));
    let mut t = RawTable::with_capacity(112);
    for id in 0..112 { t.insert(0,Value { id,drops:drops.clone(),panic_clone:true },&|_| 0); }
    assert!(catch_unwind(AssertUnwindSafe(|| { t.reserve(1000,&|v| { if v.id == 50 { panic!("hash"); } 0 }); })).is_err());
    assert_eq!(drops.get(),0);
    assert_eq!(t.len(),112);
    for id in 0..112 { assert!(t.find(0,|v| v.id == id).is_some()); }
    assert!(catch_unwind(AssertUnwindSafe(|| { t.clone_table(); })).is_err());
    let clone_drops = drops.get();
    assert!(clone_drops > 0 && clone_drops < 112);
    for id in 0..100 { let slot = t.find(0,|v| v.id == id).unwrap(); drop(t.remove(slot)); }
    let additional = t.capacity() - t.len() + 1;
    let calls = Cell::new(0);
    assert!(catch_unwind(AssertUnwindSafe(|| t.reserve(additional,&|_| {
        calls.set(calls.get()+1); if calls.get() == 4 { panic!("rehash"); } 0
    }))).is_err());
    assert_eq!(t.len(),3);
    assert_eq!(t.capacity(),112);
    drop(t);
    assert_eq!(drops.get(),112 + clone_drops);
}

// Opt-in timing only; does not invoke the shared std/bench lane.
#[test]
#[ignore]
fn quick_timing() {
    use std::hint::black_box;
    use std::time::{Duration, Instant};
    let mut seed = 0x4321fedc9876abcd;
    let keys: Vec<u64> = (0..1_000_000).map(|_| random(&mut seed)).collect();
    for mode in [3,4] {
        let mut ported = Duration::MAX;
        let mut real = Duration::MAX;
        for _ in 0..5 {
            let mut a = HashMap::with_hasher(Builder(mode));
            let start = Instant::now();
            for &key in &keys { a.insert(key,key); }
            for key in &keys { black_box(a.get(black_box(key))); }
            ported = ported.min(start.elapsed());
            black_box(a);
            let mut b = RealMap::with_hasher(Builder(mode));
            let start = Instant::now();
            for &key in &keys { b.insert(key,key); }
            for key in &keys { black_box(b.get(black_box(key))); }
            real = real.min(start.elapsed());
            black_box(b);
        }
        println!("hash mode {mode}: ported {ported:?}, real {real:?}, ratio {:.3}",ported.as_secs_f64()/real.as_secs_f64());
    }
}

#[test]
fn raw_iteration_mutation_and_drain() {
    let mut t = RawTable::with_capacity(100);
    for i in 0..100u64 { t.insert(i, i, &|v| *v); }
    t.retain(|v| *v % 3 == 0);
    let mut i = raw_table::next_full(t.ctrl(), 0);
    let mut count = 0;
    while i < t.buckets() {
        unsafe { *t.get_mut(i) += 1; }
        assert_eq!(unsafe { t.take_for_iter(i) } % 3, 1);
        count += 1;
        i = raw_table::next_full(t.ctrl(), i + 1);
    }
    assert_eq!(count, 34);
    t.reset_after_drain();
    assert_eq!(t.len(), 0);
    assert_eq!(t.capacity(), 112);
}


#[test]
fn size_classes_mirrors_and_wrap_deletion() {
    let width = if cfg!(all(relax_check, not(target_arch = "x86_64"))) { 8 } else { 16 };
    // Requested capacities cover every allocated size class through 64 buckets,
    // plus growth through both SIMD widths. Start probes at the final bucket.
    for requested in 0..=56 {
        for one_bit in [false, true] {
            let mut t = RawTable::with_capacity(requested);
            let mut real = RealMap::new();
            let hash = |k: &u64| 63 | (if one_bit { (k & 1) << 57 } else { 0 });
            let mut seed = 0x3141592653589793;
            for step in 0..512u64 {
                let k = if step < 64 { step } else { random(&mut seed) % 96 };
                let found = t.find(hash(&k), |v: &u64| *v == k);
                if step >= 64 && random(&mut seed) & 1 == 0 {
                    let got = found.map(|i| t.remove(i));
                    assert_eq!(got, real.remove(&k));
                } else if found.is_none() {
                    t.insert(hash(&k), k, &hash);
                    real.insert(k, k);
                }
                if step % 31 == 0 {
                    t.reserve(3, &hash);
                    t.shrink_to(0, &hash);
                }
                assert_eq!(t.len(), real.len());
                for key in 0..96u64 {
                    assert_eq!(t.find(hash(&key), |v: &u64| *v == key).is_some(), real.contains_key(&key));
                }
                let buckets = t.buckets();
                if buckets != 0 {
                    for i in 0..buckets.min(width) {
                        let mirror = (i.wrapping_sub(width) & (buckets - 1)) + width;
                        assert_eq!(t.ctrl()[i], unsafe { *t.ctrl().as_ptr().add(mirror) });
                    }
                    for i in buckets..width {
                        assert_eq!(unsafe { *t.ctrl().as_ptr().add(i) }, 0xff);
                    }
                }
            }
        }
    }
}
