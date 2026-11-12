// `<[T]>::element_offset`: the index of an element of a slice by address, not by value (relax
// issue 77: E0599, missing).

#[test]
fn elements_are_found_by_address() {
    let v = [1u8, 2, 3, 2];
    assert_eq!(v.element_offset(&v[2]), Some(2));
    assert_eq!(v.element_offset(&v[3]), Some(3));
    let other = 2u8;
    assert_eq!(v.element_offset(&other), None);
    let w = [7u32, 8, 9];
    assert_eq!(w.element_offset(&w[1]), Some(1));
    let pairs = [(1u16, 2u8); 3];
    assert_eq!(pairs[1..].element_offset(&pairs[2]), Some(1));
    assert_eq!(pairs[1..].element_offset(&pairs[0]), None);
}
