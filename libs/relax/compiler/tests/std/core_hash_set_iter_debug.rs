// Debug for HashSet's IntoIter and Drain and its set operations Intersection, Difference,
// SymmetricDifference and Union (std collections/hash/set.rs, stable 1.16-1.17): each prints the
// items it would yield, as a list. `#[derive(Debug)]` over a field of one of them was E0277 here.
// (Sets of one item: a larger set's order is its hasher's.)
use std::collections::hash_set;
use std::collections::HashSet;

#[derive(Debug)]
struct Draining<'a> {
    inner: hash_set::Drain<'a, u32>,
}

#[derive(Debug)]
struct Owned {
    inner: hash_set::IntoIter<&'static str>,
}

#[test]
fn a01_drain_and_into_iter() {
    let mut s = HashSet::from([7u32]);
    let mut d = Draining { inner: s.drain() };
    println!("| {:?}", d);
    println!("| {:?} {:?}", d.inner.next(), d);
    drop(d);
    let o = Owned { inner: HashSet::from(["x"]).into_iter() };
    println!("| {:?} {} {}", o, o.inner.len(), s.len());
    let mut it = HashSet::from([1u8]).into_iter();
    it.next();
    println!("| {:?}", it);
}

#[test]
fn a02_set_operations() {
    let a = HashSet::from([3u8]);
    let b = HashSet::from([4u8]);
    println!("| {:?} {:?}", a.difference(&b), a.intersection(&b));
    println!("| {:?} {:?}", a.intersection(&a), a.symmetric_difference(&a));
    println!("| {:?} {:?}", a.union(&a), b.difference(&b));
    println!("| {:#?}", a.union(&a));
}
