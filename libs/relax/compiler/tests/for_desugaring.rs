// rustc_ast_lowering/src/expr.rs:1774-1784,1826-1838: both iterator calls
// determine behavior, even when the head resembles a built-in loop.
#[test]
fn ranges_preserve_state() {
    let mut sum = 0u32;
    for n in 2..5 { sum += n; }
    assert_eq!(sum, 9);
    let mut last = 0;
    for n in 254u8..=255 { last = n; }
    assert_eq!(last, 255);
    let mut exhausted = 7u8..=7;
    assert_eq!(exhausted.next(), Some(7));
    for _ in exhausted { panic!("exhausted inclusive range yielded again"); }
    for _ in 4..2 { panic!("empty range yielded"); }
    let mut count = 0;
    for ch in '\u{d7ff}'..='\u{e000}' {
        assert!(ch == '\u{d7ff}' || ch == '\u{e000}');
        count += 1;
    }
    assert_eq!(count, 2);
    for n in 3u32.. {
        if n == 5 { break; }
        sum += n;
    }
    assert_eq!(sum, 16);
}

#[test]
fn arrays_and_slices() {
    let mut array = [1u32, 2, 3];
    for n in &mut array { *n += 1; }
    let mut sum = 0;
    for n in &array { sum += *n; }
    assert_eq!(sum, 9);
    let slice: &mut [u32] = &mut array;
    for n in slice { *n += 1; }
    let slice: &[u32] = &array;
    for n in slice { sum += *n; }
    for n in array { sum += n; }
    assert_eq!(sum, 33);
    let empty: [u8; 0] = [];
    for _ in empty { panic!("empty array yielded"); }
    let mut count = 0;
    for _ in &[(), (), ()] { count += 1; }
    assert_eq!(count, 3);
}

struct Range<T>(T);
impl IntoIterator for Range<u32> {
    type Item = u32;
    type IntoIter = std::ops::Range<u32>;
    fn into_iter(self) -> Self::IntoIter { 10..self.0 }
}

struct SliceHead([u32; 2]);
impl std::ops::Deref for SliceHead {
    type Target = [u32];
    fn deref(&self) -> &[u32] { &self.0 }
}
impl<'a> IntoIterator for &'a SliceHead {
    type Item = u32;
    type IntoIter = std::ops::Range<u32>;
    fn into_iter(self) -> Self::IntoIter { 20..22 }
}

#[test]
fn into_iterator_controls_the_head() {
    let mut sum = 0;
    for n in Range(13u32) { sum += n; }
    assert_eq!(sum, 33);
    for n in &SliceHead([100, 200]) { sum += n; }
    assert_eq!(sum, 74);
}

fn generic_sum<I: IntoIterator<Item = u32>>(iter: I) -> u32 {
    let mut sum = 0;
    for n in iter { sum += n; }
    sum
}

#[test]
fn generic_and_borrowed_iterators() {
    assert_eq!(generic_sum([2, 3, 4]), 9);
    let mut iter = 3..6;
    let mut sum = 0;
    for n in &mut iter {
        sum += n;
        break;
    }
    assert_eq!(sum, 3);
    assert_eq!(iter.next(), Some(4));
}

fn opaque_iter() -> impl Iterator<Item = u32> { 3..6 }

#[test]
fn opaque_dyn_and_collection_iterators() {
    assert_eq!(generic_sum(opaque_iter()), 12);
    let mut iter = 3u32..6;
    let dyn_iter: &mut dyn Iterator<Item = u32> = &mut iter;
    let mut sum = 0;
    for n in dyn_iter { sum += n; }
    assert_eq!(sum, 12);
    let mut values = vec![1u32, 2, 3];
    for n in &mut values { *n += 1; }
    for n in &values { sum += *n; }
    for n in values { sum += n; }
    assert_eq!(sum, 30);
}

struct DropItem<'a>(&'a std::cell::Cell<u32>);
impl<'a> Drop for DropItem<'a> {
    fn drop(&mut self) { self.0.set(self.0.get() + 1); }
}

#[test]
fn array_remaining_elements_drop_on_break() {
    let drops = std::cell::Cell::new(0);
    for _n in [DropItem(&drops), DropItem(&drops), DropItem(&drops)] { break; }
    assert_eq!(drops.get(), 3);
}
