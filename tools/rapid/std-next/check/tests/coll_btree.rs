//! BTreeMap / BTreeSet vs real std (ordered: everything compares directly).
mod coll_common;
use coll_common::*;
use rapid_std_check::collections::{BTreeMap, BTreeSet};
use std::cell::Cell;
use std::collections::{BTreeMap as SBTreeMap, BTreeSet as SBTreeSet};
use std::ops::Bound;
use std::rc::Rc as StdRc;

fn same(a: &BTreeMap<u32, u64>, b: &SBTreeMap<u32, u64>) {
    assert_eq!(a.len(), b.len());
    assert!(a.iter().map(|(k, v)| (*k, *v)).eq(b.iter().map(|(k, v)| (*k, *v))));
    assert!(a.iter().rev().map(|(k, v)| (*k, *v)).eq(b.iter().rev().map(|(k, v)| (*k, *v))));
    assert_eq!(a.first_key_value(), b.first_key_value());
    assert_eq!(a.last_key_value(), b.last_key_value());
}

fn bound(rng: &mut Rng, range: u64) -> Bound<u32> {
    match rng.below(3) {
        0 => Bound::Included(rng.below(range) as u32),
        1 => Bound::Excluded(rng.below(range) as u32),
        _ => Bound::Unbounded,
    }
}

#[test]
fn random_ops() {
    for seed in 1..6u64 {
        let mut rng = Rng(seed * 0xdead_beef_1234_5677);
        let range = [20u64, 500, 20000][seed as usize % 3];
        let mut a: BTreeMap<u32, u64> = BTreeMap::new();
        let mut b: SBTreeMap<u32, u64> = SBTreeMap::new();
        for step in 0..40_000u64 {
            let k = rng.below(range) as u32;
            let v = rng.next();
            match rng.below(14) {
                0 | 1 | 2 | 3 => assert_eq!(a.insert(k, v), b.insert(k, v)),
                4 | 5 => assert_eq!(a.remove(&k), b.remove(&k)),
                6 => assert_eq!(a.get(&k), b.get(&k)),
                7 => {
                    *a.entry(k).or_insert(v) += 1;
                    *b.entry(k).or_insert(v) += 1;
                }
                8 => assert_eq!(a.pop_first(), b.pop_first()),
                9 => assert_eq!(a.pop_last(), b.pop_last()),
                10 => {
                    let (lo, hi) = (bound(&mut rng, range), bound(&mut rng, range));
                    let ok = match (lo, hi) {
                        (Bound::Included(s) | Bound::Excluded(s), Bound::Included(e) | Bound::Excluded(e)) if s > e => false,
                        (Bound::Excluded(s), Bound::Excluded(e)) if s == e => false,
                        _ => true,
                    };
                    if ok {
                        let ra: Vec<(u32, u64)> = a.range((lo, hi)).map(|(k, v)| (*k, *v)).collect();
                        let rb: Vec<(u32, u64)> = b.range((lo, hi)).map(|(k, v)| (*k, *v)).collect();
                        assert_eq!(ra, rb);
                        let ra: Vec<u32> = a.range((lo, hi)).rev().map(|(k, _)| *k).collect();
                        let rb: Vec<u32> = b.range((lo, hi)).rev().map(|(k, _)| *k).collect();
                        assert_eq!(ra, rb);
                        let mut ia = a.range((lo, hi));
                        let mut ib = b.range((lo, hi));
                        loop {
                            let (x, y) = if rng.below(2) == 0 { (ia.next(), ib.next()) } else { (ia.next_back(), ib.next_back()) };
                            assert_eq!(x, y);
                            if x.is_none() {
                                break;
                            }
                        }
                        for (_, v) in a.range_mut((lo, hi)) {
                            *v ^= 1;
                        }
                        for (_, v) in b.range_mut((lo, hi)) {
                            *v ^= 1;
                        }
                    }
                }
                11 => {
                    if step % 500 == 0 {
                        let m = rng.below(3);
                        a.retain(|k, _| *k as u64 % 3 != m);
                        b.retain(|k, _| *k as u64 % 3 != m);
                    }
                }
                12 => {
                    if step % 700 == 0 {
                        let mut ta = a.split_off(&k);
                        let mut tb = b.split_off(&k);
                        same(&ta, &tb);
                        same(&a, &b);
                        a.append(&mut ta);
                        b.append(&mut tb);
                        assert!(ta.is_empty());
                    }
                }
                _ => assert_eq!(a.contains_key(&k), b.contains_key(&k)),
            }
            if step % 64 == 0 {
                same(&a, &b);
            }
            assert_eq!(a.len(), b.len());
        }
        same(&a, &b);
        // mixed-direction full iteration
        let mut ia = a.iter();
        let mut ib = b.iter();
        loop {
            let (x, y) = if rng.below(2) == 0 { (ia.next(), ib.next()) } else { (ia.next_back(), ib.next_back()) };
            assert_eq!(x, y);
            assert_eq!(ia.len(), ib.len());
            if x.is_none() {
                break;
            }
        }
        for v in a.values_mut() {
            *v = v.wrapping_add(5);
        }
        for (_, v) in b.iter_mut() {
            *v = v.wrapping_add(5);
        }
        let c = a.clone();
        assert!(c == a);
        same(&c, &b);
        let ka: Vec<u32> = a.keys().rev().copied().collect();
        let kb: Vec<u32> = b.keys().rev().copied().collect();
        assert_eq!(ka, kb);
        let ia: Vec<(u32, u64)> = a.into_iter().rev().collect();
        let ib: Vec<(u32, u64)> = b.into_iter().rev().collect();
        assert_eq!(ia, ib);
    }
}

#[test]
fn panics_and_debug() {
    let m: BTreeMap<u32, u32> = (0..5u32).map(|i| (i, i * 10)).collect();
    assert_eq!(hdebug(&m, false), "{0: 0, 1: 10, 2: 20, 3: 30, 4: 40}");
    assert_eq!(hdebug(&BTreeMap::from([(1u32, 2u32)]), true), "{\n    1: 2,\n}");
    let msg = panic_msg(|| {
        let m: BTreeMap<u32, u32> = BTreeMap::new();
        let _ = m.range(5..2).count();
    });
    assert_eq!(msg.as_deref(), Some("range start is greater than range end in BTreeMap"));
    let msg = panic_msg(|| {
        let m: BTreeMap<u32, u32> = BTreeMap::new();
        let _ = m.range((Bound::Excluded(3), Bound::Excluded(3))).count();
    });
    assert_eq!(msg.as_deref(), Some("range start and end are equal and excluded in BTreeMap"));
    let msg = panic_msg(|| {
        let _ = m[&9];
    });
    assert_eq!(msg.as_deref(), Some("no entry found for key"));
    let s: BTreeMap<String, u32> = [("b".to_string(), 1), ("a".to_string(), 2)].into_iter().collect();
    assert_eq!(s.get("a"), Some(&2));
    assert_eq!(s.range::<str, _>((Bound::Included("a"), Bound::Excluded("b"))).count(), 1);
}

#[test]
fn sets() {
    let mut rng = Rng(3);
    for _ in 0..40 {
        let xa: Vec<u32> = (0..rng.below(300)).map(|_| rng.below(200) as u32).collect();
        let xb: Vec<u32> = (0..rng.below(300)).map(|_| rng.below(200) as u32).collect();
        let a: BTreeSet<u32> = xa.iter().copied().collect();
        let b: BTreeSet<u32> = xb.iter().copied().collect();
        let sa: SBTreeSet<u32> = xa.iter().copied().collect();
        let sb: SBTreeSet<u32> = xb.iter().copied().collect();
        assert!(a.iter().eq(sa.iter()));
        assert!(a.union(&b).eq(sa.union(&sb)));
        assert!(a.intersection(&b).eq(sa.intersection(&sb)));
        assert!(a.difference(&b).eq(sa.difference(&sb)));
        assert!(a.symmetric_difference(&b).eq(sa.symmetric_difference(&sb)));
        assert_eq!(a.is_subset(&b), sa.is_subset(&sb));
        assert_eq!(a.is_disjoint(&b), sa.is_disjoint(&sb));
        assert!((&a | &b).iter().eq((&sa | &sb).iter()));
        assert!((&a - &b).iter().eq((&sa - &sb).iter()));
        assert_eq!(a.first(), sa.first());
        assert_eq!(a.last(), sa.last());
        assert!(a.range(10..50).eq(sa.range(10..50)));
        assert!(a.range(..=70).rev().eq(sa.range(..=70).rev()));
        let mut c = a.clone();
        let mut sc = sa.clone();
        for x in &xb {
            assert_eq!(c.insert(*x), sc.insert(*x));
            assert_eq!(c.remove(&(x / 2)), sc.remove(&(x / 2)));
        }
        assert!(c.iter().eq(sc.iter()));
        assert_eq!(c.pop_first(), sc.pop_first());
        assert_eq!(c.pop_last(), sc.pop_last());
        let d = c.split_off(&100);
        let sd = sc.split_off(&100);
        assert!(d.iter().eq(sd.iter()));
        assert!(c.iter().eq(sc.iter()));
        assert_eq!(a == b, sa == sb);
        assert_eq!(a.cmp(&b), sa.cmp(&sb));
    }
    assert_eq!(hdebug(&BTreeSet::from([3u32, 1, 2]), false), "{1, 2, 3}");
}

#[test]
fn drops() {
    let drops = StdRc::new(Cell::new(0usize));
    let mut made = 0;
    {
        let mut m: BTreeMap<u32, Counted> = BTreeMap::new();
        let mut rng = Rng(11);
        for _ in 0..3000 {
            let k = rng.below(400) as u32;
            made += 1;
            m.insert(k, Counted { v: k, drops: drops.clone() });
            if rng.below(4) == 0 {
                m.remove(&(rng.below(400) as u32));
            }
        }
        m.retain(|k, _| k % 2 == 0);
        let _ = m.pop_first();
        let c: Vec<u32> = m.extract_if(|k, _| k % 3 == 0).map(|(k, _)| k).collect();
        assert!(c.iter().all(|k| k % 3 == 0));
        let mut it = m.into_iter();
        let _ = it.next_back();
    }
    assert_eq!(drops.get(), made);
}
