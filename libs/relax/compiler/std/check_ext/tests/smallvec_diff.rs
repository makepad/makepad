//! Relax's built-in smallvec vs smallvec 1.15.1: random operation sequences on both, every
//! observable (contents, len, capacity, spilled, returned values, drop counts, panics) compared.

use relax_std_ext_check::hr_smallvec as hs;
use smallvec as rs;
use std::cell::Cell;
use std::rc::Rc;

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

macro_rules! check_same {
    ($h:expr, $r:expr, $ctx:expr) => {
        assert_eq!($h.as_slice(), $r.as_slice(), "{}", $ctx);
        assert_eq!($h.len(), $r.len(), "{}", $ctx);
        assert_eq!($h.capacity(), $r.capacity(), "cap {}", $ctx);
        assert_eq!($h.spilled(), $r.spilled(), "spilled {}", $ctx);
        assert_eq!($h.inline_size(), $r.inline_size(), "{}", $ctx);
    };
}

macro_rules! random_ops {
    ($name:ident, $n:literal) => {
        #[test]
        fn $name() {
            let mut rng = Rng(0x2545F4914F6CDD1D ^ $n);
            for round in 0..400 {
                let mut h: hs::SmallVec<[u32; $n]> = hs::SmallVec::new();
                let mut r: rs::SmallVec<[u32; $n]> = rs::SmallVec::new();
                for step in 0..120 {
                    let ctx = format!("round {} step {}", round, step);
                    let x = rng.below(100) as u32;
                    match rng.below(26) {
                        0..=4 => {
                            h.push(x);
                            r.push(x);
                        }
                        5 => assert_eq!(h.pop(), r.pop()),
                        6 => {
                            let i = rng.below(r.len() as u64 + 1) as usize;
                            h.insert(i, x);
                            r.insert(i, x);
                        }
                        7 if !r.is_empty() => {
                            let i = rng.below(r.len() as u64) as usize;
                            assert_eq!(h.remove(i), r.remove(i));
                        }
                        8 if !r.is_empty() => {
                            let i = rng.below(r.len() as u64) as usize;
                            assert_eq!(h.swap_remove(i), r.swap_remove(i));
                        }
                        9 => {
                            h.retain(|v| *v % 3 != 0);
                            r.retain(|v| *v % 3 != 0);
                        }
                        10 => {
                            h.dedup();
                            r.dedup();
                        }
                        11 => {
                            let a = rng.below(r.len() as u64 + 1) as usize;
                            let b = a + rng.below((r.len() - a) as u64 + 1) as usize;
                            let hd: Vec<u32> = h.drain(a..b).collect();
                            let rd: Vec<u32> = r.drain(a..b).collect();
                            assert_eq!(hd, rd);
                        }
                        12 => {
                            let n = rng.below(20) as usize;
                            h.resize(n, x);
                            r.resize(n, x);
                        }
                        13 => {
                            let n = rng.below(r.len() as u64 + 1) as usize;
                            h.truncate(n);
                            r.truncate(n);
                        }
                        14 => {
                            let s = [x, x + 1, x + 2];
                            h.extend_from_slice(&s);
                            r.extend_from_slice(&s);
                        }
                        15 => {
                            let n = rng.below(40) as usize;
                            h.reserve(n);
                            r.reserve(n);
                        }
                        16 => {
                            let n = rng.below(40) as usize;
                            h.reserve_exact(n);
                            r.reserve_exact(n);
                        }
                        17 => {
                            h.shrink_to_fit();
                            r.shrink_to_fit();
                        }
                        18 => {
                            let n = rng.below(6) as u32;
                            h.extend((0..n).map(|i| i + x));
                            r.extend((0..n).map(|i| i + x));
                        }
                        19 => {
                            let i = rng.below(r.len() as u64 + 1) as usize;
                            let n = rng.below(5) as u32;
                            h.insert_many(i, (0..n).map(|k| k * 7));
                            r.insert_many(i, (0..n).map(|k| k * 7));
                        }
                        20 => {
                            let i = rng.below(r.len() as u64 + 1) as usize;
                            h.insert_from_slice(i, &[x, x]);
                            r.insert_from_slice(i, &[x, x]);
                        }
                        21 => {
                            let n = (r.len() + rng.below(10) as usize).max(r.len());
                            h.grow(n);
                            r.grow(n);
                        }
                        22 => {
                            h.dedup_by_key(|v| *v / 10);
                            r.dedup_by_key(|v| *v / 10);
                        }
                        23 => {
                            let n = rng.below(15) as usize;
                            let mut c = 0u32;
                            h.resize_with(n, || { c += 1; c });
                            let mut c = 0u32;
                            r.resize_with(n, || { c += 1; c });
                        }
                        24 => {
                            let mut ho: hs::SmallVec<[u32; 3]> = hs::SmallVec::new();
                            let mut ro: rs::SmallVec<[u32; 3]> = rs::SmallVec::new();
                            for k in 0..(x % 6) {
                                ho.push(k);
                                ro.push(k);
                            }
                            h.append(&mut ho);
                            r.append(&mut ro);
                            check_same!(ho, ro, ctx);
                        }
                        _ => {
                            h.clear();
                            r.clear();
                        }
                    }
                    check_same!(h, r, ctx);
                }
                let hv = h.clone().into_vec();
                let rv = r.clone().into_vec();
                assert_eq!(hv, rv);
                assert_eq!(hv.capacity(), rv.capacity());
                let hi: Vec<u32> = h.clone().into_iter().rev().collect();
                let ri: Vec<u32> = r.clone().into_iter().rev().collect();
                assert_eq!(hi, ri);
                assert_eq!(h.clone().into_inner().is_ok(), r.clone().into_inner().is_ok());
                assert_eq!(format!("{:?}", h), format!("{:?}", r));
            }
        }
    };
}

random_ops!(ops_inline1, 1);
random_ops!(ops_inline2, 2);
random_ops!(ops_inline4, 4);
random_ops!(ops_inline8, 8);
random_ops!(ops_inline0, 0);

#[test]
fn constructors() {
    for n in 0..20usize {
        let v: Vec<u16> = (0..n as u16).collect();
        let mut wc = Vec::with_capacity(n + 3);
        wc.extend_from_slice(&v);
        let h: hs::SmallVec<[u16; 4]> = hs::SmallVec::from_vec(wc.clone());
        let r: rs::SmallVec<[u16; 4]> = rs::SmallVec::from_vec(wc.clone());
        check_same!(h, r, n);
        let h: hs::SmallVec<[u16; 4]> = hs::SmallVec::from_slice(&v);
        let r: rs::SmallVec<[u16; 4]> = rs::SmallVec::from_slice(&v);
        check_same!(h, r, n);
        let h: hs::SmallVec<[u16; 4]> = hs::SmallVec::from(&v[..]);
        let r: rs::SmallVec<[u16; 4]> = rs::SmallVec::from(&v[..]);
        check_same!(h, r, n);
        let h: hs::SmallVec<[u16; 4]> = hs::SmallVec::from_elem(7, n);
        let r: rs::SmallVec<[u16; 4]> = rs::SmallVec::from_elem(7, n);
        check_same!(h, r, n);
        let h: hs::SmallVec<[u16; 4]> = hs::SmallVec::with_capacity(n);
        let r: rs::SmallVec<[u16; 4]> = rs::SmallVec::with_capacity(n);
        check_same!(h, r, n);
        let h: hs::SmallVec<[u16; 4]> = v.iter().copied().collect();
        let r: rs::SmallVec<[u16; 4]> = v.iter().copied().collect();
        check_same!(h, r, n);
    }
    let h = hs::SmallVec::from_buf([1u8, 2, 3]);
    let r = rs::SmallVec::from_buf([1u8, 2, 3]);
    check_same!(h, r, "buf");
    let h = hs::SmallVec::from_buf_and_len([1u8, 2, 3, 4], 2);
    let r = rs::SmallVec::from_buf_and_len([1u8, 2, 3, 4], 2);
    check_same!(h, r, "buf_len");
    assert_eq!(h.into_inner().is_err(), r.into_inner().is_err());
    let h: hs::SmallVec<[(); 2]> = (0..100).map(|_| ()).collect();
    let r: rs::SmallVec<[(); 2]> = (0..100).map(|_| ()).collect();
    check_same!(h, r, "zst");
}

#[test]
fn traits_and_iterators() {
    let mut h: hs::SmallVec<[i32; 4]> = hs::SmallVec::new();
    let mut r: rs::SmallVec<[i32; 4]> = rs::SmallVec::new();
    for i in [5, 3, 9, 1, 7, 3] {
        h.push(i);
        r.push(i);
    }
    h.sort();
    r.sort();
    check_same!(h, r, "sort");
    assert_eq!(h[2], r[2]);
    assert_eq!(&h[1..4], &r[1..4]);
    let mut hs2 = h.clone();
    hs2[0] = 100;
    assert!(hs2 > h);
    assert_eq!(h == h.clone(), r == r.clone());
    for x in &mut h {
        *x *= 2;
    }
    for x in &mut r {
        *x *= 2;
    }
    let hsum: i32 = (&h).into_iter().sum();
    let rsum: i32 = (&r).into_iter().sum();
    assert_eq!(hsum, rsum);
    let mut hit = h.clone().into_iter();
    let mut rit = r.clone().into_iter();
    assert_eq!(hit.next(), rit.next());
    assert_eq!(hit.next_back(), rit.next_back());
    assert_eq!(hit.as_slice(), rit.as_slice());
    assert_eq!(format!("{:?}", hit), format!("{:?}", rit));
    let mut hd = h.drain(1..3);
    let mut rd = r.drain(1..3);
    assert_eq!(format!("{:?}", hd), format!("{:?}", rd));
    assert_eq!(hd.next_back(), rd.next_back());
    drop(hd);
    drop(rd);
    check_same!(h, r, "after drain");
    let mut hc: hs::SmallVec<[i32; 4]> = hs::SmallVec::new();
    hc.clone_from(&h);
    assert_eq!(hc.as_slice(), h.as_slice());
    use std::hash::{Hash, Hasher};
    let mut a = std::collections::hash_map::DefaultHasher::new();
    let mut b = std::collections::hash_map::DefaultHasher::new();
    h.hash(&mut a);
    r.hash(&mut b);
    assert_eq!(a.finish(), b.finish());
    let hb = h.clone().into_boxed_slice();
    let rb = r.clone().into_boxed_slice();
    assert_eq!(hb, rb);
}

#[test]
fn drops_once() {
    struct D(Rc<Cell<u32>>);
    impl Drop for D {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let c = Rc::new(Cell::new(0));
    {
        let mut v: hs::SmallVec<[D; 2]> = hs::SmallVec::new();
        for _ in 0..10 {
            v.push(D(c.clone()));
        }
        v.truncate(7);
        assert_eq!(c.get(), 3);
        v.retain(|_| false);
        assert_eq!(c.get(), 10);
        for _ in 0..5 {
            v.push(D(c.clone()));
        }
        let mut it = v.into_iter();
        it.next();
        assert_eq!(c.get(), 11);
    }
    assert_eq!(c.get(), 15);
    let mut v: hs::SmallVec<[D; 2]> = hs::SmallVec::new();
    v.push(D(c.clone()));
    v.shrink_to_fit();
    drop(v);
    assert_eq!(c.get(), 16);
}

fn panic_message<F: FnOnce() + std::panic::UnwindSafe>(f: F) -> String {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(f);
    std::panic::set_hook(prev);
    match r {
        Ok(()) => "no panic".to_string(),
        Err(e) => {
            if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "?".to_string()
            }
        }
    }
}

#[test]
fn panics_match() {
    let cases: Vec<(&str, String, String)> = vec![
        (
            "insert",
            panic_message(|| { let mut v: hs::SmallVec<[u8; 2]> = hs::SmallVec::new(); v.insert(1, 0); }),
            panic_message(|| { let mut v: rs::SmallVec<[u8; 2]> = rs::SmallVec::new(); v.insert(1, 0); }),
        ),
        (
            "remove",
            panic_message(|| { let mut v: hs::SmallVec<[u8; 2]> = hs::SmallVec::new(); v.remove(0); }),
            panic_message(|| { let mut v: rs::SmallVec<[u8; 2]> = rs::SmallVec::new(); v.remove(0); }),
        ),
        (
            "drain",
            panic_message(|| { let mut v: hs::SmallVec<[u8; 2]> = hs::SmallVec::new(); v.drain(0..3); }),
            panic_message(|| { let mut v: rs::SmallVec<[u8; 2]> = rs::SmallVec::new(); v.drain(0..3); }),
        ),
        (
            "index",
            panic_message(|| { let v: hs::SmallVec<[u8; 2]> = hs::SmallVec::new(); let _ = v[3]; }),
            panic_message(|| { let v: rs::SmallVec<[u8; 2]> = rs::SmallVec::new(); let _ = v[3]; }),
        ),
        (
            "grow",
            panic_message(|| { let mut v: hs::SmallVec<[u8; 2]> = hs::SmallVec::from_slice(&[1, 2, 3]); v.grow(1); }),
            panic_message(|| { let mut v: rs::SmallVec<[u8; 2]> = rs::SmallVec::from_slice(&[1, 2, 3]); v.grow(1); }),
        ),
    ];
    for (name, h, r) in cases {
        assert_eq!(h, r, "{}", name);
    }
}
