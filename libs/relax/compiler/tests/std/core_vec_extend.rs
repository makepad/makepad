// Vec::extend paths: exact-size iterators (one reserve, local length), slice iterators of Copy
// items (one memcpy), iterators whose size_hint lies (short / long), collect, drops.
use std::cell::Cell;

thread_local! { static DROPS: Cell<u32> = Cell::new(0); }
#[derive(Clone, Debug)]
struct D(u32);
impl Drop for D {
    fn drop(&mut self) { DROPS.with(|d| d.set(d.get() + 1)); }
}

/// Reports `hint` items exactly but yields `real` of them.
struct Liar { hint: usize, real: usize, i: usize }
impl Iterator for Liar {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.i < self.real { self.i += 1; Some(self.i as u32) } else { None }
    }
    fn size_hint(&self) -> (usize, Option<usize>) { (self.hint, Some(self.hint)) }
}

#[test]
fn a01_exact_and_slices() {
    let a: Vec<u32> = (0..1000).collect();
    let mut v: Vec<u32> = vec![7, 8];
    v.extend(a.iter());
    v.extend(&a[10..20]);
    v.extend(a.iter().copied().map(|x| x * 2).take(5));
    v.extend(a.iter().rev().take(3));
    let mut it = a.iter();
    it.next();
    v.extend(it);
    println!("| len {} sum {} head {:?} tail {:?}", v.len(), v.iter().map(|x| *x as u64).sum::<u64>(), &v[..4], &v[v.len() - 3..]);
    let s: Vec<(u8, u16)> = vec![(1, 2), (3, 4)];
    let mut w: Vec<(u8, u16)> = Vec::new();
    w.extend(s.iter());
    w.extend([(5u8, 6u16)].iter());
    println!("| tuples {:?}", w);
    let c: Vec<String> = ["a", "bb", "ccc"].iter().map(|x| x.to_string()).collect();
    println!("| collect {:?}", c);
    let mut e: Vec<u32> = Vec::new();
    e.extend(std::iter::empty::<&u32>());
    e.extend([0u32; 0].iter());
    println!("| empty {}", e.len());
}

#[test]
fn a02_lying_hints() {
    let mut v: Vec<u32> = Vec::new();
    v.extend(Liar { hint: 10, real: 4, i: 0 });
    println!("| short {:?}", v);
    v.extend(Liar { hint: 2, real: 6, i: 0 });
    println!("| long {:?}", v);
    let refs: Vec<u32> = vec![9, 9, 9];
    let mut r: Vec<u32> = Vec::new();
    r.extend(refs.iter().chain(refs.iter()).filter(|x| **x == 9));
    println!("| filtered {:?} cap ok {}", r, r.capacity() >= r.len());
}

#[test]
fn a03_drops() {
    let before = DROPS.with(|d| d.get());
    let src = vec![D(1), D(2), D(3)];
    let mut v: Vec<D> = Vec::new();
    v.extend(src.into_iter());
    v.extend(vec![D(4); 2]);
    println!("| moved {:?} drops {}", v, DROPS.with(|d| d.get()) - before);
    drop(v);
    println!("| all {}", DROPS.with(|d| d.get()) - before);
}
