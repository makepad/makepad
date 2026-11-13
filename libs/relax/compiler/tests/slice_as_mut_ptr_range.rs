// <[T]>::as_mut_ptr_range (stable 1.48): zerovec's VarZeroVecOwned calls it on a Vec<u8>
// (varzerovec/owned.rs `self.entire_slice.as_mut_ptr_range()`), which parked on E0599.

#[test]
fn as_mut_ptr_range_spans_the_slice() {
    // through Vec's deref, as zerovec calls it
    let mut bytes = vec![1u8, 2, 3, 4, 5];
    let range = bytes.as_mut_ptr_range();
    assert_eq!(unsafe { range.end.offset_from(range.start) }, 5);
    unsafe {
        *range.start = 10;
        *range.end.sub(1) = 50;
    }
    assert_eq!(range.start, bytes.as_mut_ptr());
    assert_eq!(bytes, [10, 2, 3, 4, 50]);
    // the end is one element size past the last element, whatever the size
    let mut wide = [7u32, 8, 9];
    let range = wide.as_mut_ptr_range();
    assert_eq!(range.end as usize - range.start as usize, 12);
    assert_eq!(range.start as *const u32, wide.as_ptr_range().start);
    assert_eq!(range.end as *const u32, wide.as_ptr_range().end);
    unsafe { range.start.add(1).write(80) };
    assert_eq!(wide, [7, 80, 9]);
    let mut empty: [u64; 0] = [];
    let range = empty.as_mut_ptr_range();
    assert_eq!(range.start, range.end);
    // a sub-slice
    let mut a = [0u16; 8];
    let range = a[2..5].as_mut_ptr_range();
    unsafe {
        let mut p = range.start;
        while p < range.end {
            *p = 0xffff;
            p = p.add(1);
        }
    }
    assert_eq!(a, [0, 0, 0xffff, 0xffff, 0xffff, 0, 0, 0]);
}
