//! VecDeque vs real std.
mod coll_common;
use coll_common::*;
use hotrust_std_check::collections::VecDeque;
use std::cell::Cell;
use std::collections::VecDeque as SVecDeque;
use std::rc::Rc as StdRc;

fn same(a: &VecDeque<i64>, b: &SVecDeque<i64>) {
    assert_eq!(a.len(), b.len());
    let va: Vec<i64> = a.iter().copied().collect();
    let vb: Vec<i64> = b.iter().copied().collect();
    assert_eq!(va, vb);
    let ra: Vec<i64> = a.iter().rev().copied().collect();
    let rb: Vec<i64> = b.iter().rev().copied().collect();
    assert_eq!(ra, rb);
    assert_eq!(a.front(), b.front());
    assert_eq!(a.back(), b.back());
}

#[test]
fn random_ops() {
    for seed in 1..8u64 {
        let mut rng = Rng(seed * 0x1234_5678_9abc_def1);
        let mut a: VecDeque<i64> = VecDeque::new();
        let mut b: SVecDeque<i64> = SVecDeque::new();
        for _ in 0..20_000 {
            let v = rng.next() as i64 % 1000;
            let len = b.len();
            match rng.below(20) {
                0 | 1 | 2 => {
                    a.push_back(v);
                    b.push_back(v)
                }
                3 | 4 | 5 => {
                    a.push_front(v);
                    b.push_front(v)
                }
                6 | 7 => assert_eq!(a.pop_back(), b.pop_back()),
                8 | 9 => assert_eq!(a.pop_front(), b.pop_front()),
                10 => {
                    let i = rng.below(len as u64 + 1) as usize;
                    a.insert(i, v);
                    b.insert(i, v);
                }
                11 => {
                    let i = rng.below(len as u64 + 2) as usize;
                    assert_eq!(a.remove(i), b.remove(i));
                }
                12 => {
                    if len > 0 {
                        let i = rng.below(len as u64) as usize;
                        let j = rng.below(len as u64) as usize;
                        a.swap(i, j);
                        b.swap(i, j);
                        assert_eq!(a[i], b[i]);
                        a[j] += 1;
                        b[j] += 1;
                    }
                }
                13 => {
                    let i = rng.below(len as u64 + 2) as usize;
                    assert_eq!(a.swap_remove_back(i), b.swap_remove_back(i));
                    let i = rng.below(len as u64 + 2) as usize;
                    assert_eq!(a.swap_remove_front(i), b.swap_remove_front(i));
                }
                14 => {
                    if len > 0 {
                        let s = rng.below(len as u64 + 1) as usize;
                        let e = s + rng.below((len - s) as u64 + 1) as usize;
                        let da: Vec<i64> = a.drain(s..e).collect();
                        let db: Vec<i64> = b.drain(s..e).collect();
                        assert_eq!(da, db);
                    }
                }
                15 => {
                    let n = rng.below(len as u64 + 1) as usize;
                    if rng.below(2) == 0 {
                        a.rotate_left(n);
                        b.rotate_left(n);
                    } else {
                        a.rotate_right(n);
                        b.rotate_right(n);
                    }
                }
                16 => {
                    let m = rng.below(5) as i64;
                    a.retain(|x| x % 5 != m);
                    b.retain(|x| x % 5 != m);
                }
                17 => {
                    let at = rng.below(len as u64 + 1) as usize;
                    let mut ta = a.split_off(at);
                    let mut tb = b.split_off(at);
                    same(&ta, &tb);
                    a.append(&mut ta);
                    b.append(&mut tb);
                }
                18 => {
                    let (x, y) = a.as_slices();
                    let joined: Vec<i64> = x.iter().chain(y.iter()).copied().collect();
                    let vb: Vec<i64> = b.iter().copied().collect();
                    assert_eq!(joined, vb);
                    if rng.below(8) == 0 {
                        assert_eq!(a.make_contiguous().to_vec(), b.make_contiguous().to_vec());
                    }
                }
                _ => {
                    let n = rng.below(40) as usize;
                    a.resize(n, v);
                    b.resize(n, v);
                    a.truncate(n / 2 + 3);
                    b.truncate(n / 2 + 3);
                }
            }
            assert!(a.capacity() >= a.len());
            same(&a, &b);
            assert_eq!(a.contains(&v), b.contains(&v));
        }
        // range / range_mut / iter_mut
        let len = a.len();
        if len > 2 {
            let ra: Vec<i64> = a.range(1..len - 1).copied().collect();
            let rb: Vec<i64> = b.range(1..len - 1).copied().collect();
            assert_eq!(ra, rb);
            for x in a.range_mut(..2) {
                *x *= 7;
            }
            for x in b.range_mut(..2) {
                *x *= 7;
            }
        }
        for x in a.iter_mut() {
            *x += 1;
        }
        for x in b.iter_mut() {
            *x += 1;
        }
        same(&a, &b);
        let ia: Vec<i64> = a.clone().into_iter().rev().collect();
        let ib: Vec<i64> = b.clone().into_iter().rev().collect();
        assert_eq!(ia, ib);
        let va: Vec<i64> = hotrust_std_check::vec::Vec::from(a.clone()).iter().copied().collect();
        let vb: Vec<i64> = Vec::from(b.clone());
        assert_eq!(va, vb);
        assert!(a == a.clone());
    }
}

#[test]
fn search_and_misc() {
    let mut a: VecDeque<i32> = VecDeque::new();
    let mut b: SVecDeque<i32> = SVecDeque::new();
    for i in 0..50 {
        a.push_back(i / 3 * 2);
        b.push_back(i / 3 * 2);
    }
    // force a wrapped layout with the same front/back split as std's
    for _ in 0..10 {
        let x = a.pop_back().unwrap();
        a.push_front(x - 100);
        let y = b.pop_back().unwrap();
        b.push_front(y - 100);
    }
    let sa: Vec<i32> = a.iter().copied().collect();
    let mut sorted = sa.clone();
    sorted.sort();
    let mut a2: VecDeque<i32> = sorted.iter().copied().collect();
    let b2: SVecDeque<i32> = sorted.iter().copied().collect();
    for t in -120..120 {
        assert_eq!(a2.binary_search(&t).is_ok(), b2.binary_search(&t).is_ok());
        assert_eq!(a2.partition_point(|x| *x < t), b2.partition_point(|x| *x < t));
    }
    a2.clear();
    assert!(a2.is_empty());
    let msg = panic_msg(|| {
        let d: VecDeque<u8> = VecDeque::new();
        let _ = d[3];
    });
    assert_eq!(msg.as_deref(), Some("Out of bounds access"));
    let msg = panic_msg(|| {
        let mut d: VecDeque<u8> = VecDeque::new();
        d.insert(2, 1);
    });
    assert_eq!(msg.as_deref(), Some("index out of bounds"));
    let msg = panic_msg(|| {
        let mut d: VecDeque<u8> = VecDeque::from([1u8, 2, 3]);
        let _ = d.drain(1..5);
    });
    assert_eq!(msg.as_deref(), Some("range end index 5 out of range for slice of length 3"));
    assert_eq!(hdebug(&VecDeque::from([1u32, 2, 3]), false), "[1, 2, 3]");
    let mut hv = hotrust_std_check::vec::Vec::new();
    hv.push(4u32);
    hv.push(5);
    let from_vec: VecDeque<u32> = VecDeque::from(hv);
    assert_eq!(from_vec.len(), 2);
    let _ = from_vec;
    let mut c: VecDeque<u8> = VecDeque::with_capacity(0);
    c.push_back(1);
    let mut sc: SVecDeque<u8> = SVecDeque::with_capacity(0);
    sc.push_back(1);
    assert_eq!(c.capacity(), sc.capacity());
}

#[test]
fn drops() {
    let drops = StdRc::new(Cell::new(0usize));
    let mut made = 0;
    {
        let mut d: VecDeque<Counted> = VecDeque::new();
        for i in 0..300u32 {
            made += 1;
            if i % 2 == 0 {
                d.push_back(Counted { v: i, drops: drops.clone() });
            } else {
                d.push_front(Counted { v: i, drops: drops.clone() });
            }
        }
        d.truncate(250);
        d.retain(|c| c.v % 3 != 0);
        {
            let mut dr = d.drain(10..100);
            let _ = dr.next();
            let _ = dr.next_back();
        }
        let _ = d.remove(3);
        let mut it = d.into_iter();
        let _ = it.next();
    }
    assert_eq!(drops.get(), made);
}

#[test]
fn zst() {
    let mut d: VecDeque<()> = VecDeque::new();
    for _ in 0..100 {
        d.push_back(());
        d.push_front(());
    }
    assert_eq!(d.len(), 200);
    assert_eq!(d.iter().count(), 200);
    d.drain(10..20);
    assert_eq!(d.len(), 190);
}
