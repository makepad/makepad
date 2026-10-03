//! Differential tests: Rapid std's Vec and slice algorithms vs real std.

use rapid_std_check::slice as hs;
use rapid_std_check::vec::Vec as HVec;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}

fn same(h: &HVec<i64>, r: &Vec<i64>) {
    assert_eq!(h.len(), r.len());
    assert_eq!(h.as_slice(), r.as_slice());
}

#[test]
fn vec_random_ops() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for round in 0..300 {
        let mut h: HVec<i64> = HVec::new();
        let mut r: Vec<i64> = Vec::new();
        for _ in 0..200 {
            let op = rng.below(16);
            let x = rng.below(50) as i64 - 10;
            match op {
                0 | 1 | 2 => {
                    h.push(x);
                    r.push(x);
                }
                3 => assert_eq!(h.pop(), r.pop()),
                4 => {
                    let i = rng.below(r.len() as u64 + 1) as usize;
                    h.insert(i, x);
                    r.insert(i, x);
                }
                5 if !r.is_empty() => {
                    let i = rng.below(r.len() as u64) as usize;
                    assert_eq!(h.remove(i), r.remove(i));
                }
                6 if !r.is_empty() => {
                    let i = rng.below(r.len() as u64) as usize;
                    assert_eq!(h.swap_remove(i), r.swap_remove(i));
                }
                7 => {
                    h.retain(|v| v % 3 != 0);
                    r.retain(|v| v % 3 != 0);
                }
                8 => {
                    h.dedup();
                    r.dedup();
                }
                9 => {
                    let a = rng.below(r.len() as u64 + 1) as usize;
                    let b = a + rng.below((r.len() - a) as u64 + 1) as usize;
                    let hd: std::vec::Vec<i64> = h.drain(a..b).collect();
                    let rd: Vec<i64> = r.drain(a..b).collect();
                    assert_eq!(hd, rd);
                }
                10 => {
                    let n = rng.below(20) as usize;
                    h.resize(n, x);
                    r.resize(n, x);
                }
                11 => {
                    let n = rng.below(r.len() as u64 + 1) as usize;
                    h.truncate(n);
                    r.truncate(n);
                }
                12 => {
                    let ext = [x, x + 1, x + 2];
                    h.extend_from_slice(&ext);
                    r.extend_from_slice(&ext);
                }
                13 => {
                    let at = rng.below(r.len() as u64 + 1) as usize;
                    let hs2 = h.split_off(at);
                    let rs2 = r.split_off(at);
                    assert_eq!(hs2.as_slice(), rs2.as_slice());
                }
                14 => {
                    h.dedup_by_key(|v| *v / 4);
                    r.dedup_by_key(|v| *v / 4);
                }
                _ => {
                    let a = rng.below(r.len() as u64 + 1) as usize;
                    let b = a + rng.below((r.len() - a) as u64 + 1) as usize;
                    let hs2: std::vec::Vec<i64> = h.splice(a..b, [x, x * 2]).collect();
                    let rs2: Vec<i64> = r.splice(a..b, [x, x * 2]).collect();
                    assert_eq!(hs2, rs2);
                }
            }
            same(&h, &r);
        }
        let hi: std::vec::Vec<i64> = h.clone().into_iter().rev().collect();
        let ri: Vec<i64> = r.clone().into_iter().rev().collect();
        assert_eq!(hi, ri, "round {}", round);
    }
}

#[test]
fn vec_capacity_growth_matches() {
    let mut h: HVec<u8> = HVec::new();
    let mut r: Vec<u8> = Vec::new();
    for i in 0..1000 {
        h.push(i as u8);
        r.push(i as u8);
        assert_eq!(h.capacity(), r.capacity(), "u8 at {}", i);
    }
    let mut h: HVec<u64> = HVec::new();
    let mut r: Vec<u64> = Vec::new();
    for i in 0..1000 {
        h.push(i);
        r.push(i);
        assert_eq!(h.capacity(), r.capacity(), "u64 at {}", i);
    }
    let h: HVec<u32> = HVec::with_capacity(13);
    assert_eq!(h.capacity(), 13);
}

#[test]
fn vec_drops_everything_once() {
    use std::cell::Cell;
    use std::rc::Rc;
    struct D(Rc<Cell<u32>>);
    impl Drop for D {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let c = Rc::new(Cell::new(0));
    {
        let mut v = HVec::new();
        for _ in 0..100 {
            v.push(D(c.clone()));
        }
        v.truncate(90);
        assert_eq!(c.get(), 10);
        v.retain(|_| false);
        assert_eq!(c.get(), 100);
        for _ in 0..10 {
            v.push(D(c.clone()));
        }
        let mut it = v.into_iter();
        it.next();
        assert_eq!(c.get(), 101);
    }
    assert_eq!(c.get(), 110);
    let mut v = HVec::new();
    for _ in 0..10 {
        v.push(D(c.clone()));
    }
    {
        let mut d = v.drain(2..5);
        d.next();
    }
    assert_eq!(c.get(), 113);
    assert_eq!(v.len(), 7);
    drop(v);
    assert_eq!(c.get(), 120);
}

#[test]
#[should_panic(expected = "insertion index (is 5) should be <= len (is 3)")]
fn vec_insert_panic() {
    let mut v: HVec<u8> = HVec::new();
    v.extend_from_slice(&[1, 2, 3]);
    v.insert(5, 0);
}

#[test]
#[should_panic(expected = "removal index (is 3) should be < len (is 3)")]
fn vec_remove_panic() {
    let mut v: HVec<u8> = HVec::new();
    v.extend_from_slice(&[1, 2, 3]);
    v.remove(3);
}

#[test]
#[should_panic(expected = "range end index 7 out of range for slice of length 3")]
fn vec_drain_panic() {
    let mut v: HVec<u8> = HVec::new();
    v.extend_from_slice(&[1, 2, 3]);
    v.drain(1..7);
}

#[test]
#[should_panic(expected = "slice index starts at 2 but ends at 1")]
fn slice_order_panic() {
    let mut v: HVec<u8> = HVec::new();
    v.extend_from_slice(&[1, 2, 3]);
    let a = 2;
    let b = 1;
    let _ = &v[a..b];
}

#[test]
#[should_panic(expected = "index out of bounds: the len is 3 but the index is 3")]
fn vec_index_panic() {
    let mut v: HVec<u8> = HVec::new();
    v.extend_from_slice(&[1, 2, 3]);
    let i = 3;
    let _ = v[i];
}

#[test]
fn stable_sort_matches() {
    let mut rng = Rng(77);
    for n in [0usize, 1, 2, 3, 5, 19, 20, 21, 50, 100, 1000, 5000] {
        for _ in 0..5 {
            let base: Vec<(u32, u32)> = (0..n).map(|i| ((rng.below(30)) as u32, i as u32)).collect();
            let mut a = base.clone();
            let mut b = base.clone();
            hs::sort::merge_sort(&mut a, &mut |x: &(u32, u32), y: &(u32, u32)| x.0 < y.0);
            b.sort_by_key(|x| x.0);
            assert_eq!(a, b, "n {}", n);
        }
    }
}

#[test]
fn unstable_sort_sorts() {
    let mut rng = Rng(99);
    for n in [0usize, 1, 2, 3, 5, 19, 20, 21, 50, 100, 1000, 10000] {
        for kind in 0..4 {
            let mut a: Vec<u64> = (0..n)
                .map(|i| match kind {
                    0 => rng.next() % 1000,
                    1 => i as u64,
                    2 => (n - i) as u64,
                    _ => 7,
                })
                .collect();
            let mut b = a.clone();
            hs::sort::quicksort(&mut a, &mut |x: &u64, y: &u64| x < y);
            b.sort_unstable();
            assert_eq!(a, b);
            let mut c = b.clone();
            c.reverse();
            hs::sort::heapsort(&mut c, &mut |x: &u64, y: &u64| x < y);
            assert_eq!(c, b);
        }
    }
}

#[test]
fn sort_by_cached_key_matches() {
    let mut rng = Rng(5);
    let base: Vec<(u32, u32)> = (0..500).map(|i| (rng.below(20) as u32, i)).collect();
    let mut a = base.clone();
    let mut b = base.clone();
    hs::sort::sort_by_cached_key(&mut a, |x| x.0);
    b.sort_by_cached_key(|x| x.0);
    assert_eq!(a, b);
}

#[test]
fn binary_search_matches_core() {
    let mut rng = Rng(3);
    for n in 0..60usize {
        let mut v: Vec<u32> = (0..n).map(|_| rng.below(10) as u32).collect();
        v.sort();
        for x in 0..12u32 {
            assert_eq!(hs::binary_search_by(&v, |p| p.cmp(&x)), v.binary_search(&x), "n {} x {}", n, x);
            assert_eq!(hs::partition_point(&v, |p| *p < x), v.partition_point(|p| *p < x));
        }
    }
}

#[test]
fn slice_helpers_match() {
    let mut rng = Rng(11);
    for n in 0..40usize {
        let v: Vec<u8> = (0..n).map(|_| rng.below(4) as u8).collect();
        for k in 0..=n {
            let mut a = v.clone();
            let mut b = v.clone();
            hs::rotate_left(&mut a, k);
            b.rotate_left(k);
            assert_eq!(a, b);
            let mut a = v.clone();
            let mut b = v.clone();
            hs::rotate_right(&mut a, k);
            b.rotate_right(k);
            assert_eq!(a, b);
        }
        let mut a = v.clone();
        hs::reverse(&mut a);
        let mut b = v.clone();
        b.reverse();
        assert_eq!(a, b);
        assert_eq!(hs::contains(&v, &2), v.contains(&2));
        assert_eq!(hs::starts_with(&v, &[0, 1]), v.starts_with(&[0, 1]));
        assert_eq!(hs::ends_with(&v, &[3]), v.ends_with(&[3]));
        for size in 1..6 {
            let x: Vec<&[u8]> = hs::chunks(&v, size).collect();
            let y: Vec<&[u8]> = v.chunks(size).collect();
            assert_eq!(x, y);
            let x: Vec<&[u8]> = hs::chunks(&v, size).rev().collect();
            let y: Vec<&[u8]> = v.chunks(size).rev().collect();
            assert_eq!(x, y);
        }
        for size in 1..6 {
            let x: Vec<&[u8]> = hs::windows(&v, size).collect();
            let y: Vec<&[u8]> = v.windows(size).collect();
            assert_eq!(x, y);
            let x: Vec<&[u8]> = hs::chunks_exact(&v, size).rev().collect();
            let y: Vec<&[u8]> = v.chunks_exact(size).rev().collect();
            assert_eq!(x, y);
            let x: Vec<&[u8]> = hs::rchunks(&v, size).collect();
            let y: Vec<&[u8]> = v.rchunks(size).collect();
            assert_eq!(x, y);
        }
        let x: Vec<&[u8]> = hs::split(&v, |b| *b == 0).collect();
        let y: Vec<&[u8]> = v.split(|b| *b == 0).collect();
        assert_eq!(x, y);
        let x: Vec<&[u8]> = hs::split(&v, |b| *b == 0).rev().collect();
        let y: Vec<&[u8]> = v.split(|b| *b == 0).rev().collect();
        assert_eq!(x, y);
        let x: Vec<&[u8]> = hs::split_inclusive(&v, |b| *b == 1).collect();
        let y: Vec<&[u8]> = v.split_inclusive(|b| *b == 1).collect();
        assert_eq!(x, y);
        let x: Vec<&[u8]> = hs::split_inclusive(&v, |b| *b == 1).rev().collect();
        let y: Vec<&[u8]> = v.split_inclusive(|b| *b == 1).rev().collect();
        assert_eq!(x, y);
        for k in 0..4 {
            let x: Vec<&[u8]> = hs::splitn(&v, k, |b| *b == 2).collect();
            let y: Vec<&[u8]> = v.splitn(k, |b| *b == 2).collect();
            assert_eq!(x, y);
            let x: Vec<&[u8]> = hs::rsplitn(&v, k, |b| *b == 2).collect();
            let y: Vec<&[u8]> = v.rsplitn(k, |b| *b == 2).collect();
            assert_eq!(x, y);
        }
        assert_eq!(hs::trim_ascii(b"  ab c \n"), b"ab c");
        let mut s = *b"HeLLo";
        hs::make_ascii_lowercase(&mut s);
        assert_eq!(&s, b"hello");
        assert!(hs::eq_ignore_ascii_case(b"HeLLo", b"hello"));
    }
}

#[test]
fn join_concat() {
    let parts = ["a", "bc", "", "d"];
    assert_eq!(hs::join_str(&parts, ", "), parts.join(", "));
    assert_eq!(hs::concat_str(&parts), parts.concat());
    let vs = [vec![1, 2], vec![], vec![3]];
    assert_eq!(hs::join_vec(&vs, &[0]).as_slice(), vs.join(&0).as_slice());
    assert_eq!(hs::concat_vec(&vs).as_slice(), vs.concat().as_slice());
}
