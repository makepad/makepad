// The bounds of a range are coercion sites, as the fields of the struct literal rustc lowers it to
// (`a..=b` is `RangeInclusive::new(a, b)`): an expected range type gives the bounds their type
// (zerovec: `shift_bytes(index_range.end..prev_element_p.start, ..)` passes `*mut u8` bounds for a
// `Range<*const u8>` parameter), and without one the second bound coerces to the first's type.
use std::ops::{Range, RangeFrom, RangeInclusive, RangeTo, RangeToInclusive};

unsafe fn span(block: Range<*const u8>) -> usize {
    block.end.offset_from(block.start) as usize
}
fn total(r: Range<&[u8]>) -> usize {
    r.start.len() + r.end.len()
}
fn first(r: RangeInclusive<&dyn Fn() -> u8>) -> u8 {
    (r.start())() + (r.end())()
}

#[test]
fn mutable_pointers_for_a_range_of_const_pointers() {
    let mut buf = [0u8; 8];
    let p: *mut u8 = buf.as_mut_ptr();
    let q: *mut u8 = unsafe { p.add(5) };
    assert_eq!(unsafe { span(p..q) }, 5);
    let r: Range<*const u8> = p..q;
    assert_eq!(unsafe { span(r) }, 5);
    let from: RangeFrom<*const u8> = q..;
    let to: RangeTo<*const u8> = ..q;
    let to_incl: RangeToInclusive<*const u8> = ..=q;
    let incl: RangeInclusive<*const u8> = p..=q;
    assert_eq!(from.start, to.end);
    assert_eq!(to_incl.end, *incl.end());
    assert_eq!(*incl.start(), p as *const u8);
}

#[test]
fn the_second_bound_coerces_to_the_first() {
    let mut buf = [0u8; 4];
    let c: *const u8 = buf.as_ptr();
    let m: *mut u8 = buf.as_mut_ptr();
    let r = c..unsafe { m.add(3) };
    assert_eq!(unsafe { span(r) }, 3);
}

#[test]
fn bounds_unsize_to_the_expected_element() {
    let (a, b) = ([1u8, 2, 3], [4u8]);
    assert_eq!(total(&a..&b), 4);
    let (x, y) = (7u8, 5u8);
    assert_eq!(first(&(move || x)..=&(move || y)), 12);
    let n: Range<u64> = 1..40;
    assert_eq!(n.end - n.start, 39);
}
