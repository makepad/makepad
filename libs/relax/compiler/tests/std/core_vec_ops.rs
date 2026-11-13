// Vec mutation, capacity, ownership, sorting and slice helper observations.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::fmt::{Debug, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};

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
    fn obs<T: Debug + ?Sized>(&mut self, v: &T) {
        write!(self, "{:?}", v).unwrap();
        self.bytes(&[0xff]);
        self.1 += 1;
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}
impl Write for D {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

#[test]
fn a01_vec_random_ops() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut d = D::new();
    for _ in 0..30 {
        let mut v: Vec<i64> = Vec::new();
        for _ in 0..200 {
            let op = rng.below(16);
            let x = rng.below(50) as i64 - 10;
            match op {
                0 | 1 | 2 => v.push(x),
                3 => d.obs(&v.pop()),
                4 => {
                    let i = rng.below(v.len() as u64 + 1) as usize;
                    v.insert(i, x);
                }
                5 if !v.is_empty() => {
                    let i = rng.below(v.len() as u64) as usize;
                    d.obs(&v.remove(i));
                }
                6 if !v.is_empty() => {
                    let i = rng.below(v.len() as u64) as usize;
                    d.obs(&v.swap_remove(i));
                }
                7 => v.retain(|v| v % 3 != 0),
                8 => v.dedup(),
                9 => {
                    let a = rng.below(v.len() as u64 + 1) as usize;
                    let b = a + rng.below((v.len() - a) as u64 + 1) as usize;
                    d.obs(&v.drain(a..b).collect::<Vec<_>>());
                }
                10 => v.resize(rng.below(20) as usize, x),
                11 => {
                    let n = rng.below(v.len() as u64 + 1) as usize;
                    v.truncate(n);
                }
                12 => v.extend_from_slice(&[x, x + 1, x + 2]),
                13 => {
                    let at = rng.below(v.len() as u64 + 1) as usize;
                    d.obs(&v.split_off(at));
                }
                14 => v.dedup_by_key(|v| *v / 4),
                _ => {
                    let a = rng.below(v.len() as u64 + 1) as usize;
                    let b = a + rng.below((v.len() - a) as u64 + 1) as usize;
                    d.obs(&v.splice(a..b, [x, x * 2]).collect::<Vec<_>>());
                }
            }
            d.obs(&v.len());
            d.obs(v.as_slice());
        }
        d.obs(&v.clone().into_iter().rev().collect::<Vec<_>>());
    }
    d.line("vec random");
}

#[test]
fn a02_vec_capacity_growth_matches() {
    let mut d = D::new();
    let mut v: Vec<u8> = Vec::new();
    for i in 0..1000 {
        v.push(i as u8);
        d.obs(&v.capacity());
    }
    d.line("vec capacity u8");
    let mut d = D::new();
    let mut v: Vec<u64> = Vec::new();
    for i in 0..1000 {
        v.push(i);
        d.obs(&v.capacity());
    }
    d.line("vec capacity u64");
    let v: Vec<u32> = Vec::with_capacity(13);
    assert_eq!(v.capacity(), 13);
}

#[test]
fn a03_vec_drops_everything_once() {
    use std::cell::Cell;
    use std::rc::Rc;
    struct Dropped(Rc<Cell<u32>>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let c = Rc::new(Cell::new(0));
    {
        let mut v = Vec::new();
        for _ in 0..100 {
            v.push(Dropped(c.clone()));
        }
        v.truncate(90);
        assert_eq!(c.get(), 10);
        v.retain(|_| false);
        assert_eq!(c.get(), 100);
        for _ in 0..10 {
            v.push(Dropped(c.clone()));
        }
        let mut it = v.into_iter();
        let _ = it.next();
        assert_eq!(c.get(), 101);
        println!("| vec partial drops {}", c.get());
    }
    assert_eq!(c.get(), 110);
    let mut v = Vec::new();
    for _ in 0..10 {
        v.push(Dropped(c.clone()));
    }
    {
        let mut drain = v.drain(2..5);
        let _ = drain.next();
    }
    assert_eq!(c.get(), 113);
    assert_eq!(v.len(), 7);
    drop(v);
    assert_eq!(c.get(), 120);
    println!("| vec all drops {}", c.get());
}

fn panics(tag: &str, expected: &str, f: impl FnOnce()) {
    let outcome = catch_unwind(AssertUnwindSafe(f));
    let payload = outcome.expect_err("expected bounds panic");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("");
    println!("| vec panic {} {:?}", tag, message);
    assert!(message.contains(expected), "unexpected panic message: {}", message);
}

#[test]
fn a04_vec_insert_panic() {
    panics("insert", "insertion index (is 5) should be <= len (is 3)", || {
        let mut v = vec![1u8, 2, 3];
        v.insert(5, 0);
    });
}

#[test]
fn a05_vec_remove_panic() {
    panics("remove", "removal index (is 3) should be < len (is 3)", || {
        let mut v = vec![1u8, 2, 3];
        v.remove(3);
    });
}

#[test]
fn a06_vec_drain_panic() {
    panics("drain", "range end index 7 out of range for slice of length 3", || {
        let mut v = vec![1u8, 2, 3];
        let _ = v.drain(1..7);
    });
}

#[test]
fn a07_slice_order_panic() {
    panics("order", "slice index starts at 2 but ends at 1", || {
        let v = vec![1u8, 2, 3];
        let a = 2;
        let b = 1;
        let _ = &v[a..b];
    });
}

#[test]
fn a08_vec_index_panic() {
    panics("index", "index out of bounds: the len is 3 but the index is 3", || {
        let v = vec![1u8, 2, 3];
        let i = 3;
        let _ = v[i];
    });
}

#[test]
fn a09_stable_sort_matches() {
    let mut rng = Rng(77);
    let mut d = D::new();
    for n in [0usize, 1, 2, 3, 5, 19, 20, 21, 50, 100, 1000, 5000] {
        for _ in 0..5 {
            let mut v: Vec<_> = (0..n).map(|i| (rng.below(30) as u32, i as u32)).collect();
            v.sort_by_key(|x| x.0);
            d.obs(&v);
        }
    }
    d.line("vec stable sort");
}

#[test]
fn a10_unstable_sort_sorts() {
    let mut rng = Rng(99);
    let mut d = D::new();
    for n in [0usize, 1, 2, 3, 5, 19, 20, 21, 50, 100, 1000, 10000] {
        for kind in 0..4 {
            let mut v: Vec<u64> = (0..n)
                .map(|i| match kind {
                    0 => rng.next() % 1000,
                    1 => i as u64,
                    2 => (n - i) as u64,
                    _ => 7,
                })
                .collect();
            v.sort_unstable();
            d.obs(&v);
            v.reverse();
            // The former direct heapsort case uses public unstable-sort dispatch.
            v.sort_unstable();
            d.obs(&v);
        }
    }
    d.line("vec unstable sort");
}

#[test]
fn a11_sort_by_cached_key_matches() {
    let mut rng = Rng(5);
    let mut v: Vec<_> = (0..500).map(|i| (rng.below(20) as u32, i)).collect();
    v.sort_by_cached_key(|x| x.0);
    let mut d = D::new();
    d.obs(&v);
    d.line("vec cached key");
}

#[test]
fn a12_binary_search_matches_core() {
    let mut rng = Rng(3);
    let mut d = D::new();
    for n in 0..60usize {
        let mut v: Vec<u32> = (0..n).map(|_| rng.below(10) as u32).collect();
        v.sort();
        for x in 0..12u32 {
            d.obs(&v.binary_search_by(|p| p.cmp(&x)));
            d.obs(&v.partition_point(|p| *p < x));
        }
    }
    d.line("vec binary search");
}

#[test]
fn a13_slice_helpers_match() {
    let mut rng = Rng(11);
    let mut d = D::new();
    for n in 0..40usize {
        let v: Vec<u8> = (0..n).map(|_| rng.below(4) as u8).collect();
        for k in 0..=n {
            let mut a = v.clone();
            a.rotate_left(k);
            d.obs(&a);
            let mut a = v.clone();
            a.rotate_right(k);
            d.obs(&a);
        }
        let mut a = v.clone();
        a.reverse();
        d.obs(&a);
        d.obs(&v.contains(&2));
        d.obs(&v.starts_with(&[0, 1]));
        d.obs(&v.ends_with(&[3]));
        for size in 1..6 {
            d.obs(&v.chunks(size).collect::<Vec<_>>());
            d.obs(&v.chunks(size).rev().collect::<Vec<_>>());
            d.obs(&v.windows(size).collect::<Vec<_>>());
            d.obs(&v.chunks_exact(size).rev().collect::<Vec<_>>());
            d.obs(&v.rchunks(size).collect::<Vec<_>>());
        }
        d.obs(&v.split(|b| *b == 0).collect::<Vec<_>>());
        d.obs(&v.split(|b| *b == 0).rev().collect::<Vec<_>>());
        d.obs(&v.split_inclusive(|b| *b == 1).collect::<Vec<_>>());
        d.obs(&v.split_inclusive(|b| *b == 1).rev().collect::<Vec<_>>());
        for k in 0..4 {
            d.obs(&v.splitn(k, |b| *b == 2).collect::<Vec<_>>());
            d.obs(&v.rsplitn(k, |b| *b == 2).collect::<Vec<_>>());
        }
        assert_eq!(b"  ab c \n".trim_ascii(), b"ab c");
        let mut s = *b"HeLLo";
        s.make_ascii_lowercase();
        assert_eq!(&s, b"hello");
        assert!(b"HeLLo".eq_ignore_ascii_case(b"hello"));
    }
    d.line("vec slice helpers");
}

#[test]
fn a14_join_concat() {
    let parts = ["a", "bc", "", "d"];
    println!("| join strings {:?} {:?}", parts.join(", "), parts.concat());
    let vs = [vec![1, 2], vec![], vec![3]];
    println!("| join vectors {:?} {:?}", vs.join(&0), vs.concat());
}
