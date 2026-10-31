// Boot batch 4: `&if ..` against `&str` (B33), projection through `Box<HashMap<?K, ?V>>` (B35), a
// bound's method on `&R` (B36), `for x in &deque` (B37), u128/i128 `/` and `%` (B39). rustc: all pass.
use std::collections::{HashMap, VecDeque};
use std::ops::{Bound, RangeBounds};

fn takes(s: &str) -> usize {
    s.len()
}

#[test]
fn addr_of_value_with_unsized_hint() {
    let upper = true;
    let s = "ab";
    assert!(takes(&if upper { s.to_uppercase() } else { s.to_lowercase() }) == 2);
}

#[test]
fn deref_projection_with_inference_vars() {
    let mut map = Box::new(HashMap::with_capacity(4));
    map.insert(1u64, 2u32);
    let spill: Option<Box<HashMap<u64, u32>>> = Some(map);
    assert!(*spill.unwrap().get(&1).unwrap() == 2);
}

fn lo<R: RangeBounds<usize>>(r: &R) -> usize {
    match r.start_bound() {
        Bound::Included(s) => *s,
        Bound::Excluded(s) => *s + 1,
        Bound::Unbounded => 0,
    }
}

#[test]
fn bound_method_through_ref() {
    assert!(lo(&(3..5)) + lo(&(..)) + lo(&(2..=4)) == 5);
}

#[test]
fn for_over_ref_adt() {
    let mut d = VecDeque::new();
    d.push_back(1u32);
    d.push_back(2);
    let mut s = 0;
    for v in &d {
        s += *v;
    }
    assert!(s == 3);
}

fn divrem(a: u128, b: u128) -> u64 {
    ((a / b) as u64) + ((a % b) as u64)
}
fn sdivrem(a: i128, b: i128) -> i64 {
    ((a / b) as i64) * 10 + ((a % b) as i64)
}

#[test]
fn wide_division() {
    assert!(divrem(100_000_000_000_000_000_000, 7) == 14285714285714285714 + 2);
    assert!(sdivrem(-17, 5) == -32);
    let mut x: u128 = 1 << 100;
    x /= 1 << 90;
    assert!(x == 1024);
}
