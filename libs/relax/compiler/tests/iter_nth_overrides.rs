// Core's iterators that have an `nth` or `advance_by` of their own hand a skip to the inner
// iterator in one call: `Skip`, `Fuse`, `Chain`, `Peekable` and `StepBy` (iter/adapters), the
// forwarding adapters' advance_by, the slice and VecDeque iterators (O(1)) and array::IntoIter
// and vec::IntoIter (which drop the skipped items). Relax's std lacked them, so the default `nth`
// stepped `n` times through `next`: `step_by` over a `skip` of a slice, which takes every
// `interval`th sample of a window from each start, cost O(len) per element, quadratic in all.
// The tests count the calls an adapter makes into an iterator; under rustc --test they pass on real
// std, so the counts are core's. From relax PR 161.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Counts 0..end and the `next` and `nth` calls made into it.
struct Counting {
    at: u32,
    end: u32,
    nexts: Rc<Cell<u32>>,
    nths: Rc<Cell<u32>>,
}

fn counting(end: u32) -> (Counting, Rc<Cell<u32>>, Rc<Cell<u32>>) {
    let (nexts, nths) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    (Counting { at: 0, end, nexts: nexts.clone(), nths: nths.clone() }, nexts, nths)
}

impl Iterator for Counting {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        self.nexts.set(self.nexts.get() + 1);
        if self.at < self.end {
            self.at += 1;
            Some(self.at - 1)
        } else {
            None
        }
    }
    fn nth(&mut self, n: usize) -> Option<u32> {
        self.nths.set(self.nths.get() + 1);
        let target = (self.at as usize).saturating_add(n);
        if target < self.end as usize {
            self.at = target as u32 + 1;
            Some(target as u32)
        } else {
            self.at = self.end;
            None
        }
    }
}

#[test]
fn skip_and_nth_are_one_inner_nth() {
    let (inner, nexts, nths) = counting(1000);
    let mut it = inner.skip(100);
    assert_eq!(it.nth(5), Some(105));
    assert_eq!((nexts.get(), nths.get()), (0, 1));
    // the skip is spent: the next nth is the inner one alone
    assert_eq!(it.nth(2), Some(108));
    assert_eq!((nexts.get(), nths.get()), (0, 2));
    assert_eq!(it.next(), Some(109));
    let v = [1, 2, 3];
    assert_eq!(v.iter().skip(1).nth(1), Some(&3));
    assert_eq!(v.iter().skip(1).nth(2), None);
    assert_eq!(v.iter().skip(5).nth(0), None);
    // the sum of the skip and n overflows usize
    assert_eq!(v.iter().skip(usize::MAX).nth(usize::MAX), None);
    assert_eq!(v.iter().skip(2).nth(usize::MAX), None);
    let mut it = v.iter().skip(2);
    assert_eq!(it.nth(0), Some(&3));
    assert_eq!(it.next(), None);
}

/// `step_by` over a `skip` of a million items: one inner nth per item it yields (and one for the
/// end), never a `next`; the default `nth` took a `next` per item passed, a million here.
#[test]
fn step_by_over_skip_is_linear_in_what_it_yields() {
    let (inner, nexts, nths) = counting(1_000_000);
    let mut it = inner.skip(3).step_by(1000);
    let (mut yielded, mut last) = (0u32, 0u32);
    while let Some(x) = it.next() {
        yielded += 1;
        last = x;
    }
    assert_eq!((yielded, last), (1000, 999_003));
    assert_eq!((nexts.get(), nths.get()), (0, yielded + 1));
}

#[test]
fn fuse_nth_is_one_inner_nth() {
    let (inner, nexts, nths) = counting(100);
    let mut it = inner.fuse();
    assert_eq!(it.nth(40), Some(40));
    assert_eq!(it.nth(100), None);
    assert_eq!(it.nth(0), None);
    // fused: the exhausted inner iterator is not asked again
    assert_eq!((nexts.get(), nths.get()), (0, 2));
    let got: Vec<u32> = counting(50).0.fuse().step_by(7).collect();
    assert_eq!(got, [0, 7, 14, 21, 28, 35, 42, 49]);
}

#[test]
fn peekable_nth_counts_the_peeked_item() {
    let (inner, nexts, nths) = counting(100);
    let mut it = inner.peekable();
    assert_eq!(it.peek(), Some(&0));
    assert_eq!(it.nth(0), Some(0));
    assert_eq!(it.peek(), Some(&1));
    assert_eq!(it.nth(5), Some(6));
    assert_eq!(it.nth(3), Some(10));
    // two peeks, then two inner nths
    assert_eq!((nexts.get(), nths.get()), (2, 2));
    let mut it = [1, 2].iter().peekable();
    assert_eq!(it.nth(5), None);
    assert_eq!(it.peek(), None);
}

#[test]
fn step_by_nth_is_one_inner_nth() {
    let (inner, nexts, nths) = counting(1000);
    let mut it = inner.step_by(10);
    assert_eq!(it.nth(3), Some(30));
    assert_eq!((nexts.get(), nths.get()), (1, 1));
    assert_eq!(it.nth(2), Some(60));
    assert_eq!((nexts.get(), nths.get()), (1, 2));
    assert_eq!(it.next(), Some(70));
    assert_eq!(it.nth(100), None);
    let v: Vec<u32> = (0..100).collect();
    assert_eq!(v.iter().step_by(9).nth(0), Some(&0));
    assert_eq!(v.iter().step_by(9).nth(11), Some(&99));
    assert_eq!(v.iter().step_by(9).nth(12), None);
    assert_eq!(v.iter().step_by(usize::MAX).nth(1), None);
    assert_eq!(v.iter().step_by(2).nth(usize::MAX), None);
    let mut it = v.iter().step_by(3);
    it.next();
    assert_eq!(it.nth(usize::MAX), None);
}

#[test]
fn chain_nth_goes_to_the_second_in_one_nth() {
    let (b, nexts, nths) = counting(100);
    let a = [100u32, 101, 102];
    let mut it = a.iter().copied().chain(b);
    assert_eq!(it.nth(1), Some(101));
    assert_eq!(it.nth(3), Some(2));
    assert_eq!(it.nth(4), Some(7));
    assert_eq!((nexts.get(), nths.get()), (0, 2));
    let x: Vec<u32> = (0..10).collect();
    let y: Vec<u32> = (10..25).collect();
    for n in 0..30 {
        let want = if n < 25 { Some(n as u32) } else { None };
        assert_eq!(x.iter().chain(y.iter()).nth(n).copied(), want);
    }
    let got: Vec<u32> = x.iter().chain(y.iter()).step_by(4).copied().collect();
    assert_eq!(got, [0, 4, 8, 12, 16, 20, 24]);
    let mut it = x.iter().chain(y.iter());
    assert_eq!(it.nth(12), Some(&12));
    assert_eq!(it.next(), Some(&13));
}

#[test]
fn vec_deque_nth_across_both_halves() {
    let mut d: VecDeque<u32> = VecDeque::with_capacity(8);
    for i in 0..6 {
        d.push_back(i);
    }
    for _ in 0..4 {
        d.pop_front();
    }
    for i in 6..12 {
        d.push_back(i);
    }
    // the deque wraps: 4..12 lies in two slices
    assert!(!d.as_slices().1.is_empty());
    for n in 0..10 {
        let want = if n < 8 { Some(4 + n as u32) } else { None };
        assert_eq!(d.iter().nth(n).copied(), want);
    }
    let got: Vec<u32> = d.iter().step_by(3).copied().collect();
    assert_eq!(got, [4, 7, 10]);
    let mut it = d.iter_mut();
    *it.nth(5).unwrap() += 100;
    assert_eq!(it.next(), Some(&mut 10));
    assert_eq!(d[5], 109);
}

#[test]
fn array_into_iter_nth_drops_the_skipped() {
    let drops = Rc::new(Cell::new(0));
    struct D(u32, Rc<Cell<u32>>);
    impl Drop for D {
        fn drop(&mut self) {
            self.1.set(self.1.get() + 1);
        }
    }
    let arr = [0, 1, 2, 3, 4, 5].map(|i| D(i, drops.clone()));
    let mut it = arr.into_iter();
    let third = it.nth(2).unwrap();
    assert_eq!(third.0, 2);
    assert_eq!(drops.get(), 2);
    drop(third);
    assert_eq!(drops.get(), 3);
    assert!(it.nth(5).is_none());
    assert_eq!(drops.get(), 6);
    let got: Vec<u32> = [0u32, 1, 2, 3, 4, 5, 6].into_iter().step_by(3).collect();
    assert_eq!(got, [0, 3, 6]);
}

thread_local!(static DROPS: Cell<u32> = Cell::new(0));

/// Counts its drops; the one numbered 0 panics in its drop.
struct Bomb(u32);
impl Drop for Bomb {
    fn drop(&mut self) {
        DROPS.with(|d| d.set(d.get() + 1));
        if self.0 == 0 {
            panic!("bomb");
        }
    }
}

fn drops() -> u32 {
    DROPS.with(|d| d.replace(0))
}

#[test]
fn skipped_items_are_all_dropped_when_one_panics() {
    drops();
    // array::IntoIter: a skip past a panicking drop still drops the other skipped items
    let mut it = [Bomb(0), Bomb(1), Bomb(2), Bomb(3)].into_iter();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| it.nth(3).map(|b| b.0)));
    assert!(r.is_err());
    assert_eq!(drops(), 3);
    drop(it);
    assert_eq!(drops(), 1);
    // and so does its drop of the items left
    let it = [Bomb(5), Bomb(0), Bomb(6), Bomb(7)].into_iter();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(it))).is_err());
    assert_eq!(drops(), 4);
    // vec::IntoIter's nth
    let mut it = vec![Bomb(0), Bomb(1), Bomb(2), Bomb(3)].into_iter();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| it.nth(3).map(|b| b.0)));
    assert!(r.is_err());
    assert_eq!(drops(), 3);
    drop(it);
    assert_eq!(drops(), 1);
    // plain data: nothing to drop, the same items
    assert_eq!([7u8; 4096].into_iter().nth(4000), Some(7));
    assert_eq!(vec![1u32, 2, 3, 4, 5].into_iter().step_by(2).collect::<Vec<_>>(), [1, 3, 5]);
}

/// Every nth and nth_back of `make()` (front, back, through a chain on either side, and every
/// step) gives the items a plain collect gives.
fn same_as_collect<I, F>(what: &str, make: F)
where
    I: DoubleEndedIterator,
    I::Item: PartialEq + std::fmt::Debug + Clone,
    F: Fn() -> I,
{
    let all: Vec<I::Item> = make().collect();
    let len = all.len();
    for n in 0..len + 3 {
        assert_eq!(make().nth(n), all.get(n).cloned(), "{what}: nth({n})");
        let back = if n < len { Some(all[len - 1 - n].clone()) } else { None };
        assert_eq!(make().nth_back(n), back, "{what}: nth_back({n})");
        // through Chain::nth (the front's advance_by) and Chain::nth_back (the back's advance_back_by)
        let tail: Vec<I::Item> = all.iter().take(2).cloned().collect();
        let mut joined = all.clone();
        joined.extend(tail.iter().cloned());
        assert_eq!(make().chain(tail.iter().cloned()).nth(n), joined.get(n).cloned(), "{what}: chain nth({n})");
        let mut back_joined = tail.clone();
        back_joined.extend(all.iter().cloned());
        let bl = back_joined.len();
        let want = if n < bl { Some(back_joined[bl - 1 - n].clone()) } else { None };
        assert_eq!(tail.iter().cloned().chain(make()).nth_back(n), want, "{what}: chain nth_back({n})");
        // a chain's nth runs its front on, and then the item after it
        let mut it = make().chain(tail.iter().cloned());
        let first = it.nth(n);
        assert_eq!(first, joined.get(n).cloned());
        assert_eq!(it.next(), joined.get(n + 1).cloned(), "{what}: next after chain nth({n})");
    }
    for step in 1..len + 2 {
        let want: Vec<I::Item> = all.iter().step_by(step).cloned().collect();
        assert_eq!(make().step_by(step).collect::<Vec<_>>(), want, "{what}: step_by({step})");
        let want: Vec<I::Item> = all.iter().rev().step_by(step).cloned().collect();
        assert_eq!(make().rev().step_by(step).collect::<Vec<_>>(), want, "{what}: rev step_by({step})");
    }
}

#[test]
fn advance_by_and_nth_back_agree_with_a_collect() {
    let v: Vec<u32> = (0..23).collect();
    let w: Vec<u32> = (100..107).collect();
    let mut d: VecDeque<u32> = (0..10).collect();
    d.rotate_left(4);
    same_as_collect("slice", || v.iter());
    same_as_collect("copied", || v.iter().copied());
    same_as_collect("enumerate", || v.iter().enumerate());
    same_as_collect("take", || v.iter().take(17));
    same_as_collect("skip", || v.iter().skip(5));
    same_as_collect("fuse", || v.iter().fuse());
    same_as_collect("rev", || v.iter().rev());
    same_as_collect("chain", || v.iter().chain(w.iter()));
    same_as_collect("chain chain", || w.iter().chain(v.iter()).chain(w.iter()));
    same_as_collect("range", || 3u32..40);
    same_as_collect("range of chars", || 'a'..'q');
    same_as_collect("vec into_iter", || v.clone().into_iter());
    same_as_collect("array into_iter", || [1u8, 2, 3, 4, 5, 6, 7].into_iter());
    same_as_collect("vec_deque", || d.iter());
    same_as_collect("chain of enumerate skip", || v.iter().skip(3).enumerate().chain(w.iter().enumerate()));
    let mut m: Vec<u32> = v.clone();
    let mut ms: Vec<u32> = Vec::new();
    for n in 0..30 {
        ms.push(m.iter_mut().nth_back(n).map(|x| *x).unwrap_or(999));
    }
    let want: Vec<u32> = (0..30).map(|n| if n < 23 { 22 - n } else { 999 }).collect();
    assert_eq!(ms, want);
    let mut it = m.iter_mut();
    *it.nth_back(3).unwrap() += 100;
    assert_eq!(it.next_back(), Some(&mut 18));
    assert_eq!(m[19], 119);
    let mut dm = d.clone();
    *dm.iter_mut().rev().nth(7).unwrap() += 50;
    assert_eq!(dm[2], d[2] + 50);
}

#[test]
fn chain_nth_past_a_skip_that_runs_out() {
    // Chain::nth advances its front (a skip of 10 over 5 items, which runs out in its skip: the
    // inner iterator's own advance_by, 5 items and the None) and goes on in the back
    let (inner, nexts, nths) = counting(5);
    let (b, _, b_nths) = counting(3);
    let mut it = inner.skip(10).chain(b);
    assert_eq!(it.nth(1), Some(1));
    assert_eq!((nexts.get(), nths.get(), b_nths.get()), (6, 0, 1));
    assert_eq!(it.next(), Some(2));
}
