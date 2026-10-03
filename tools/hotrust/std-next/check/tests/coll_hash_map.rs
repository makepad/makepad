//! HashMap / HashSet vs real std: random operation sequences, compared result by result.
mod coll_common;
use coll_common::*;
use hotrust_std_check::collections::{HashMap, HashSet};
use std::cell::Cell;
use std::collections::{HashMap as SHashMap, HashSet as SHashSet};
use std::rc::Rc as StdRc;

fn sorted_pairs<'a, I: Iterator<Item = (&'a u32, &'a u64)>>(it: I) -> Vec<(u32, u64)> {
    let mut v: Vec<(u32, u64)> = it.map(|(k, v)| (*k, *v)).collect();
    v.sort();
    v
}

#[test]
fn random_ops_u32() {
    for seed in 1..6u64 {
        let mut rng = Rng(seed * 0x9e37_79b9_7f4a_7c15);
        let mut a: HashMap<u32, u64> = HashMap::new();
        let mut b: SHashMap<u32, u64> = SHashMap::new();
        let range = [16u64, 200, 5000][seed as usize % 3];
        for step in 0..60_000u64 {
            let k = rng.below(range) as u32;
            let v = rng.next();
            match rng.below(12) {
                0 | 1 | 2 => assert_eq!(a.insert(k, v), b.insert(k, v)),
                3 | 4 => assert_eq!(a.remove(&k), b.remove(&k)),
                5 => assert_eq!(a.get(&k), b.get(&k)),
                6 => assert_eq!(a.contains_key(&k), b.contains_key(&k)),
                7 => {
                    *a.entry(k).or_insert(v) += 1;
                    *b.entry(k).or_insert(v) += 1;
                }
                8 => {
                    a.entry(k).and_modify(|x| *x ^= v).or_default();
                    b.entry(k).and_modify(|x| *x ^= v).or_default();
                }
                9 => {
                    if let Some(x) = a.get_mut(&k) {
                        *x = x.wrapping_add(v);
                    }
                    if let Some(x) = b.get_mut(&k) {
                        *x = x.wrapping_add(v);
                    }
                }
                10 => assert_eq!(a.remove_entry(&k), b.remove_entry(&k)),
                _ => {
                    if step % 997 == 0 {
                        let m = rng.below(4) as u64;
                        a.retain(|k, v| (*k as u64 + *v) % 4 != m);
                        b.retain(|k, v| (*k as u64 + *v) % 4 != m);
                    }
                }
            }
            assert_eq!(a.len(), b.len());
            assert!(a.capacity() >= a.len());
        }
        assert_eq!(sorted_pairs(a.iter()), sorted_pairs(b.iter()));
        let mut ka: Vec<u32> = a.keys().copied().collect();
        ka.sort();
        let mut kb: Vec<u32> = b.keys().copied().collect();
        kb.sort();
        assert_eq!(ka, kb);
        let sa: u64 = a.values().fold(0u64, |s, x| s.wrapping_add(*x));
        let sb: u64 = b.values().fold(0u64, |s, x| s.wrapping_add(*x));
        assert_eq!(sa, sb);
        for v in a.values_mut() {
            *v = v.wrapping_mul(3);
        }
        for (_, v) in a.iter_mut() {
            *v = v.wrapping_add(1);
        }
        for v in b.values_mut() {
            *v = v.wrapping_mul(3).wrapping_add(1);
        }
        let c = a.clone();
        assert!(c == a);
        let mut ia: Vec<(u32, u64)> = c.into_iter().collect();
        ia.sort();
        let mut ib: Vec<(u32, u64)> = b.clone().into_iter().collect();
        ib.sort();
        assert_eq!(ia, ib);
        let mut da: Vec<(u32, u64)> = a.drain().collect();
        da.sort();
        assert_eq!(da, ib);
        assert!(a.is_empty());
        a.insert(1, 2);
        assert_eq!(a.get(&1), Some(&2));
        a.shrink_to_fit();
        assert_eq!(a.get(&1), Some(&2));
    }
}

#[test]
fn string_keys_and_borrow() {
    let mut rng = Rng(77);
    let mut a: HashMap<String, usize> = HashMap::new();
    let mut b: SHashMap<String, usize> = SHashMap::new();
    for i in 0..20_000usize {
        let k = format!("key{}", rng.below(3000));
        match rng.below(4) {
            0 | 1 => assert_eq!(a.insert(k.clone(), i), b.insert(k, i)),
            2 => assert_eq!(a.remove(k.as_str()), b.remove(k.as_str())),
            _ => assert_eq!(a.get(k.as_str()), b.get(k.as_str())),
        }
    }
    assert_eq!(a.len(), b.len());
    for (k, v) in b.iter() {
        assert_eq!(a[k.as_str()], *v);
        assert_eq!(a.get_key_value(k.as_str()), Some((k, v)));
    }
    let msg = panic_msg(|| {
        let m: HashMap<u32, u32> = HashMap::new();
        let _ = m[&5];
    });
    assert_eq!(msg.as_deref(), Some("no entry found for key"));
}

#[test]
fn collect_extend_eq() {
    let a: HashMap<u32, u32> = (0..1000u32).map(|i| (i % 300, i)).collect();
    let b: SHashMap<u32, u32> = (0..1000u32).map(|i| (i % 300, i)).collect();
    assert_eq!(a.len(), b.len());
    for (k, v) in &b {
        assert_eq!(a.get(k), Some(v));
    }
    let mut c: HashMap<u32, u32> = HashMap::with_capacity(10);
    c.extend(b.iter().map(|(k, v)| (*k, *v)));
    assert!(c == a);
    c.insert(5000, 1);
    assert!(c != a);
    let mut d: HashMap<u32, u32> = HashMap::new();
    d.extend(b.iter());
    assert!(d == a);
    let e = HashMap::from([(1u32, 2u32), (3, 4)]);
    assert_eq!(e.len(), 2);
    assert_eq!(e[&3], 4);
}

#[test]
fn entry_api() {
    let mut a: HashMap<&'static str, Vec<u32>> = HashMap::new();
    a.entry("x").or_insert_with(Vec::new).push(1);
    a.entry("x").or_default().push(2);
    a.entry("y").or_insert_with_key(|k| vec![k.len() as u32]);
    assert_eq!(a["x"], vec![1, 2]);
    assert_eq!(a["y"], vec![1]);
    match a.entry("x") {
        hotrust_std_check::collections::hash_map::Entry::Occupied(mut o) => {
            assert_eq!(*o.key(), "x");
            o.get_mut().push(3);
            assert_eq!(o.insert(vec![9]), vec![1, 2, 3]);
            assert_eq!(o.remove(), vec![9]);
        }
        _ => panic!(),
    }
    assert!(!a.contains_key("x"));
    match a.entry("z") {
        hotrust_std_check::collections::hash_map::Entry::Vacant(v) => {
            assert_eq!(*v.key(), "z");
            v.insert(vec![7]);
        }
        _ => panic!(),
    }
    assert_eq!(a["z"], vec![7]);
    assert_eq!(*a.entry("q").key(), "q");
}

#[test]
fn drops_exactly_once() {
    let drops = StdRc::new(Cell::new(0usize));
    let mut made = 0usize;
    {
        let mut m: HashMap<u32, Counted> = HashMap::new();
        let mut rng = Rng(5);
        for _ in 0..5000 {
            let k = rng.below(700) as u32;
            made += 1;
            m.insert(k, Counted { v: k, drops: drops.clone() });
            if rng.below(3) == 0 {
                m.remove(&(rng.below(700) as u32));
            }
        }
        m.retain(|k, v| *k % 3 != 0 && v.v == *k);
        let mut it = m.clone_shallow_ids().into_iter();
        let _ = it.next();
        let mut d = m.drain();
        let _ = d.next();
        drop(d);
        assert!(m.is_empty());
        for k in 0..100u32 {
            made += 1;
            m.insert(k, Counted { v: k, drops: drops.clone() });
        }
        let mut ii = m.into_iter();
        let _ = ii.next();
    }
    assert_eq!(drops.get(), made);
}

trait CloneIds {
    fn clone_shallow_ids(&self) -> HashMap<u32, u32>;
}

impl CloneIds for HashMap<u32, Counted> {
    fn clone_shallow_ids(&self) -> HashMap<u32, u32> {
        self.iter().map(|(k, v)| (*k, v.v)).collect()
    }
}

#[test]
fn debug_format() {
    let mut a: HashMap<u32, u32> = HashMap::new();
    a.insert(7, 8);
    assert_eq!(hdebug(&a, false), "{7: 8}");
    assert_eq!(hdebug(&a, true), "{\n    7: 8,\n}");
    let e: HashMap<u32, u32> = HashMap::new();
    assert_eq!(hdebug(&e, false), "{}");
    let mut s: HashSet<u32> = HashSet::new();
    s.insert(3);
    assert_eq!(hdebug(&s, false), "{3}");
    assert_eq!(hdebug(&s.iter(), false), "[3]");
    assert_eq!(hdebug(&a.iter(), false), "[(7, 8)]");
}

#[test]
fn set_ops() {
    let mut rng = Rng(99);
    for _ in 0..50 {
        let xa: Vec<u32> = (0..rng.below(200)).map(|_| rng.below(150) as u32).collect();
        let xb: Vec<u32> = (0..rng.below(200)).map(|_| rng.below(150) as u32).collect();
        let a: HashSet<u32> = xa.iter().copied().collect();
        let b: HashSet<u32> = xb.iter().copied().collect();
        let sa: SHashSet<u32> = xa.iter().copied().collect();
        let sb: SHashSet<u32> = xb.iter().copied().collect();
        let srt = |it: &mut dyn Iterator<Item = u32>| {
            let mut v: Vec<u32> = it.collect();
            v.sort();
            v
        };
        assert_eq!(srt(&mut a.union(&b).copied()), srt(&mut sa.union(&sb).copied()));
        assert_eq!(srt(&mut a.intersection(&b).copied()), srt(&mut sa.intersection(&sb).copied()));
        assert_eq!(srt(&mut a.difference(&b).copied()), srt(&mut sa.difference(&sb).copied()));
        assert_eq!(
            srt(&mut a.symmetric_difference(&b).copied()),
            srt(&mut sa.symmetric_difference(&sb).copied())
        );
        assert_eq!(a.is_subset(&b), sa.is_subset(&sb));
        assert_eq!(a.is_superset(&b), sa.is_superset(&sb));
        assert_eq!(a.is_disjoint(&b), sa.is_disjoint(&sb));
        assert_eq!(srt(&mut (&a | &b).into_iter()), srt(&mut (&sa | &sb).into_iter()));
        assert_eq!(srt(&mut (&a & &b).into_iter()), srt(&mut (&sa & &sb).into_iter()));
        assert_eq!(srt(&mut (&a - &b).into_iter()), srt(&mut (&sa - &sb).into_iter()));
        assert_eq!(srt(&mut (&a ^ &b).into_iter()), srt(&mut (&sa ^ &sb).into_iter()));
        let mut c = a.clone();
        let mut sc = sa.clone();
        for x in xb.iter() {
            assert_eq!(c.insert(*x), sc.insert(*x));
            assert_eq!(c.remove(&(x + 1)), sc.remove(&(x + 1)));
            assert_eq!(c.contains(&(x + 2)), sc.contains(&(x + 2)));
            assert_eq!(c.take(&(x + 3)), sc.take(&(x + 3)));
        }
        assert_eq!(c.len(), sc.len());
        c.retain(|x| x % 2 == 0);
        sc.retain(|x| x % 2 == 0);
        assert_eq!(srt(&mut c.iter().copied()), srt(&mut sc.iter().copied()));
        assert_eq!(*c.get_or_insert(1000), 1000);
        assert!(c.contains(&1000));
        assert_eq!(c.replace(1000), Some(1000));
    }
}
