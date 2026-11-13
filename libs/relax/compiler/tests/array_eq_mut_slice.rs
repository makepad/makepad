// core array/equality.rs: an array compares with `&mut [U]` on either side (aws-lc-rs:
// `debug_assert_eq!(in_out_iter.into_remainder(), [])`).
fn rest(buf: &mut [u8], at: usize) -> &mut [u8] {
    &mut buf[at..]
}

#[test]
fn a_mutable_slice_compares_with_an_array() {
    let mut buf = [1u8, 2, 3];
    assert_eq!(rest(&mut buf, 3), []);
    assert_eq!(rest(&mut buf, 1), [2, 3]);
    assert!(rest(&mut buf, 1) != [2, 4]);
    assert!(rest(&mut buf, 2) != [3, 3]);
}

#[test]
fn an_array_compares_with_a_mutable_slice() {
    let mut buf = [1u8, 2, 3];
    assert_eq!([2u8, 3], rest(&mut buf, 1));
    assert!([1u8, 2, 3] != rest(&mut buf, 1));
    let empty: [u8; 0] = [];
    assert_eq!(empty, rest(&mut buf, 3));
}
