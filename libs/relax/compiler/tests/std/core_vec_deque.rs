// VecDeque random mutations, searches, wrapped storage, draining and zero-sized values.
// Differential file: `| ` lines equal under rustc --test and makepad test.

use std::cell::Cell;
use std::collections::VecDeque;
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

fn panic_message<R>(f: impl FnOnce() -> R) -> Option<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).err().map(|error| {
        if let Some(s) = error.downcast_ref::<String>() {
            s.clone()
        } else {
            error.downcast_ref::<&str>().unwrap().to_string()
        }
    })
}

fn observe(a: &VecDeque<i64>, d: &mut D) {
    d.add(&a.len());
    d.add(&a.iter().copied().collect::<Vec<_>>());
    d.add(&a.iter().rev().copied().collect::<Vec<_>>());
    d.add(&a.front());
    d.add(&a.back());
}

#[test]
fn a01_random_ops() {
    for seed in 1..8u64 {
        let mut rng = Rng(seed.wrapping_mul(0x1234_5678_9abc_def1));
        let mut d = D::new();
        let mut a: VecDeque<i64> = VecDeque::new();
        for _ in 0..2_000 {
            let v = rng.next() as i64 % 1000;
            let len = a.len();
            match rng.below(20) {
                0..=2 => a.push_back(v),
                3..=5 => a.push_front(v),
                6 | 7 => d.add(&a.pop_back()),
                8 | 9 => d.add(&a.pop_front()),
                10 => a.insert(rng.below(len as u64 + 1) as usize, v),
                11 => d.add(&a.remove(rng.below(len as u64 + 2) as usize)),
                12 => {
                    if len > 0 {
                        let i = rng.below(len as u64) as usize;
                        let j = rng.below(len as u64) as usize;
                        a.swap(i, j);
                        d.add(&a[i]);
                        a[j] += 1;
                    }
                }
                13 => {
                    d.add(&a.swap_remove_back(rng.below(len as u64 + 2) as usize));
                    d.add(&a.swap_remove_front(rng.below(len as u64 + 2) as usize));
                }
                14 => {
                    if len > 0 {
                        let start = rng.below(len as u64 + 1) as usize;
                        let end = start + rng.below((len - start) as u64 + 1) as usize;
                        d.add(&a.drain(start..end).collect::<Vec<_>>());
                    }
                }
                15 => {
                    let n = rng.below(len as u64 + 1) as usize;
                    if rng.below(2) == 0 {
                        a.rotate_left(n);
                    } else {
                        a.rotate_right(n);
                    }
                }
                16 => {
                    let m = rng.below(5) as i64;
                    a.retain(|x| x % 5 != m);
                }
                17 => {
                    let mut tail = a.split_off(rng.below(len as u64 + 1) as usize);
                    observe(&tail, &mut d);
                    a.append(&mut tail);
                }
                18 => {
                    let (front, back) = a.as_slices();
                    d.add(&front.iter().chain(back.iter()).copied().collect::<Vec<_>>());
                    if rng.below(8) == 0 {
                        d.add(a.make_contiguous());
                    }
                }
                _ => {
                    let n = rng.below(40) as usize;
                    a.resize(n, v);
                    a.truncate(n / 2 + 3);
                }
            }
            assert!(a.capacity() >= a.len());
            observe(&a, &mut d);
            d.add(&a.contains(&v));
        }
        let len = a.len();
        if len > 2 {
            d.add(&a.range(1..len - 1).copied().collect::<Vec<_>>());
            for x in a.range_mut(..2) {
                *x *= 7;
            }
        }
        for x in a.iter_mut() {
            *x += 1;
        }
        observe(&a, &mut d);
        d.add(&a.clone().into_iter().rev().collect::<Vec<_>>());
        d.add(&Vec::from(a.clone()));
        assert!(a == a.clone());
        d.line(&format!("deque random seed {seed}"));
    }
}

#[test]
fn a02_search_and_misc() {
    let mut a = VecDeque::new();
    for i in 0..50 {
        a.push_back(i / 3 * 2);
    }
    for _ in 0..10 {
        let x = a.pop_back().unwrap();
        a.push_front(x - 100);
    }
    let mut sorted: Vec<_> = a.iter().copied().collect();
    sorted.sort();
    let mut a: VecDeque<_> = sorted.into_iter().collect();
    let mut d = D::new();
    for target in -120..120 {
        d.add(&a.binary_search(&target).is_ok());
        d.add(&a.partition_point(|x| *x < target));
    }
    a.clear();
    assert!(a.is_empty());
    std::panic::set_hook(Box::new(|_| {}));
    let index = panic_message(|| {
        let a: VecDeque<u8> = VecDeque::new();
        let _ = a[3];
    });
    let insert = panic_message(|| {
        let mut a: VecDeque<u8> = VecDeque::new();
        a.insert(2, 1);
    });
    let drain = panic_message(|| {
        let mut a = VecDeque::from([1u8, 2, 3]);
        let _ = a.drain(1..5);
    });
    // the messages are compared with real std's through the digest
    assert!(index.is_some());
    assert_eq!(insert.as_deref(), Some("index out of bounds"));
    assert_eq!(drain.as_deref(), Some("range end index 5 out of range for slice of length 3"));
    d.add(&(index.as_deref(), insert.as_deref(), drain.as_deref()));
    println!("| deque index panic {:?}", index);
    assert_eq!(format!("{:?}", VecDeque::from([1u32, 2, 3])), "[1, 2, 3]");
    let from_vec = VecDeque::from(vec![4u32, 5]);
    assert_eq!(from_vec.len(), 2);
    let mut c: VecDeque<u8> = VecDeque::with_capacity(0);
    c.push_back(1);
    d.add(&c.capacity());
    d.line("deque search misc");
}

#[test]
fn a03_drops() {
    let drops = Rc::new(Cell::new(0usize));
    let mut made = 0;
    {
        let mut a = VecDeque::new();
        for i in 0..300u32 {
            made += 1;
            let value = Counted { v: i, drops: drops.clone() };
            if i % 2 == 0 {
                a.push_back(value);
            } else {
                a.push_front(value);
            }
        }
        a.truncate(250);
        a.retain(|c| c.v % 3 != 0);
        {
            let mut drain = a.drain(10..100);
            let _ = drain.next();
            let _ = drain.next_back();
        }
        let _ = a.remove(3);
        let mut it = a.into_iter();
        let _ = it.next();
    }
    assert_eq!(drops.get(), made);
    println!("| deque drops {}", drops.get());
}

#[test]
fn a04_zst() {
    let mut a = VecDeque::new();
    for _ in 0..100 {
        a.push_back(());
        a.push_front(());
    }
    assert_eq!(a.len(), 200);
    assert_eq!(a.iter().count(), 200);
    drop(a.drain(10..20));
    assert_eq!(a.len(), 190);
    println!("| deque zst {}", a.len());
}

