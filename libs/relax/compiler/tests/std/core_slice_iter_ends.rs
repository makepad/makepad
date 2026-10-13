// slice::Iter / IterMut as a pointer pair: both ends, nth from either end, exhaustion, the rest as a
// slice, and zero-sized elements (whose iterator counts down without moving its pointer)
#[derive(Debug, PartialEq, Clone, Copy)]
struct Z;

#[test]
fn both_ends() {
    let v = [1u32, 2, 3, 4, 5, 6, 7];
    let mut it = v.iter();
    assert_eq!(it.len(), 7);
    assert_eq!(it.next(), Some(&1));
    assert_eq!(it.next_back(), Some(&7));
    assert_eq!(it.nth(1), Some(&3));
    assert_eq!(it.as_slice(), &[4, 5, 6]);
    assert_eq!(it.nth_back(1), Some(&5));
    assert_eq!(it.size_hint(), (1, Some(1)));
    assert_eq!(it.clone().count(), 1);
    assert_eq!(it.nth(3), None);
    assert_eq!(it.next(), None);
    assert_eq!(it.next_back(), None);
    let s: u32 = v.iter().rev().step_by(2).sum();
    assert_eq!(s, 7 + 5 + 3 + 1);
    let e: [u8; 0] = [];
    assert_eq!(e.iter().next(), None);
    assert_eq!(e.iter().len(), 0);
}

#[test]
fn mutable() {
    let mut v = [1i64, 2, 3, 4];
    for x in v.iter_mut() {
        *x *= 10;
    }
    let mut it = v.iter_mut();
    *it.next_back().unwrap() += 1;
    *it.nth(1).unwrap() += 2;
    assert_eq!(it.as_slice(), &[30]);
    assert_eq!(it.into_slice().len(), 1);
    assert_eq!(v, [10, 22, 30, 41]);
}

#[test]
fn zero_sized() {
    let v = [Z; 5];
    let mut it = v.iter();
    assert_eq!(it.len(), 5);
    assert_eq!(it.next(), Some(&Z));
    assert_eq!(it.next_back(), Some(&Z));
    assert_eq!(it.len(), 3);
    assert_eq!(it.as_slice().len(), 3);
    assert_eq!(it.nth(1), Some(&Z));
    assert_eq!(it.len(), 1);
    assert_eq!(it.nth(5), None);
    assert_eq!(it.next(), None);
    assert_eq!(v.iter().count(), 5);
    assert_eq!(v.iter().rev().count(), 5);
    let mut w = [(), (), ()];
    assert_eq!(w.iter_mut().count(), 3);
    let mut m = w.iter_mut();
    assert!(m.next_back().is_some());
    assert_eq!(m.nth(0), Some(&mut ()));
    assert_eq!(m.len(), 1);
    assert_eq!(m.nth(3), None);
    assert_eq!(m.next(), None);
}
